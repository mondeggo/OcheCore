//! Camera capture: one thread per camera streaming MJPG over V4L2.
//!
//! Each thread dequeues every frame (cheap) but decodes only the frames due
//! for analysis, straight to luminance, and hands them over through a bounded
//! channel. A full channel drops the sample rather than blocking capture. The
//! latest JPEG is kept for calibration. Errors are reported in the camera
//! status and the device is reopened after a pause.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use oche_domain::{CameraState, CameraStatus};
use oche_vision::mask::GrayImage;

/// What to open and how often to analyse it.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraSpec {
    pub slot: u8,
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Manual exposure in 100 µs units; `None` keeps auto exposure.
    pub exposure: Option<u32>,
    pub analysis_hz: u32,
}

/// One analysed frame.
pub struct Sample {
    pub slot: u8,
    /// Kernel monotonic time at the start of exposure, in seconds.
    pub timestamp: f64,
    pub luma: GrayImage,
}

struct Shared {
    status: Mutex<CameraStatus>,
    latest_jpeg: Mutex<Option<Vec<u8>>>,
    stop: AtomicBool,
}

/// A running capture thread; stops and joins on drop.
pub struct Camera {
    spec: CameraSpec,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Camera {
    pub fn start(spec: CameraSpec, samples: SyncSender<Sample>) -> Self {
        let shared = Arc::new(Shared {
            status: Mutex::new(CameraStatus {
                slot: spec.slot,
                state: CameraState::Starting,
                fps: 0.0,
                error: None,
            }),
            latest_jpeg: Mutex::new(None),
            stop: AtomicBool::new(false),
        });
        let thread = {
            let (spec, shared) = (spec.clone(), shared.clone());
            std::thread::Builder::new()
                .name(format!("camera-{}", spec.slot))
                .spawn(move || run(&spec, &shared, &samples))
                .ok()
        };
        if thread.is_none() {
            set_error(&shared, "could not start the capture thread".into());
        }
        Self {
            spec,
            shared,
            thread,
        }
    }

    pub fn spec(&self) -> &CameraSpec {
        &self.spec
    }

    pub fn status(&self) -> CameraStatus {
        self.shared
            .status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(CameraStatus {
                slot: self.spec.slot,
                state: CameraState::Error,
                fps: 0.0,
                error: Some("status unavailable".into()),
            })
    }

    pub fn latest_jpeg(&self) -> Option<Vec<u8>> {
        self.shared.latest_jpeg.lock().ok()?.clone()
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn set_error(shared: &Shared, message: String) {
    if let Ok(mut status) = shared.status.lock() {
        status.state = CameraState::Error;
        status.fps = 0.0;
        status.error = Some(message);
    }
}

fn run(spec: &CameraSpec, shared: &Shared, samples: &SyncSender<Sample>) {
    while !shared.stop.load(Ordering::Relaxed) {
        if let Err(message) = stream(spec, shared, samples) {
            set_error(shared, message);
        }
        // Reopen after a pause; check the stop flag often so shutdown is quick.
        let resume = Instant::now() + Duration::from_secs(2);
        while Instant::now() < resume && !shared.stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    if let Ok(mut status) = shared.status.lock() {
        status.state = CameraState::Off;
        status.fps = 0.0;
    }
}

/// Decides which frames to analyse and tracks delivered FPS.
struct Pacer {
    period: f64,
    next_due: f64,
    window_start: Option<f64>,
    window_frames: u32,
}

impl Pacer {
    fn new(analysis_hz: u32) -> Self {
        Self {
            period: 1.0 / f64::from(analysis_hz.max(1)),
            next_due: f64::NEG_INFINITY,
            window_start: None,
            window_frames: 0,
        }
    }

    /// Whether the frame at `timestamp` should be analysed.
    fn due(&mut self, timestamp: f64) -> bool {
        if timestamp + 1e-3 < self.next_due {
            return false;
        }
        // Stay on the grid unless we fell behind by more than one period.
        self.next_due = if timestamp - self.next_due > self.period {
            timestamp + self.period
        } else {
            self.next_due + self.period
        };
        true
    }

    /// Delivered FPS once a two-second window completes.
    fn frame(&mut self, timestamp: f64) -> Option<f64> {
        let start = *self.window_start.get_or_insert(timestamp);
        self.window_frames += 1;
        let elapsed = timestamp - start;
        (elapsed >= 2.0).then(|| {
            let fps = f64::from(self.window_frames - 1) / elapsed;
            self.window_start = Some(timestamp);
            self.window_frames = 1;
            fps
        })
    }
}

#[cfg(not(target_os = "linux"))]
fn stream(_: &CameraSpec, _: &Shared, _: &SyncSender<Sample>) -> Result<(), String> {
    Err("camera capture requires Linux V4L2".into())
}

#[cfg(target_os = "linux")]
fn stream(spec: &CameraSpec, shared: &Shared, samples: &SyncSender<Sample>) -> Result<(), String> {
    use linux_video::Device;
    use linux_video::types::*;

    if spec.device.is_empty() {
        return Err("no device selected for this slot".into());
    }
    let device =
        Device::open(&spec.device).map_err(|e| format!("cannot open {}: {e}", spec.device))?;

    let mut format = device
        .format(BufferType::VideoCapture)
        .map_err(|e| format!("cannot read format: {e}"))?;
    {
        let pix = format
            .try_mut::<PixFormat>()
            .ok_or("device does not report a single-plane capture format")?;
        pix.set_width(spec.width);
        pix.set_height(spec.height);
        pix.set_pixel_format(FourCc::Mjpeg);
    }
    device
        .set_format(&mut format)
        .map_err(|e| format!("cannot set {}x{} MJPG: {e}", spec.width, spec.height))?;
    let pix = format.try_ref::<PixFormat>().ok_or("format changed type")?;
    if pix.width() != spec.width
        || pix.height() != spec.height
        || pix.pixel_format() != FourCc::Mjpeg
    {
        return Err(format!(
            "camera negotiated {}x{} {} instead of {}x{} MJPG",
            pix.width(),
            pix.height(),
            pix.pixel_format(),
            spec.width,
            spec.height
        ));
    }

    let mut param = device
        .param(BufferType::VideoCapture)
        .map_err(|e| format!("cannot read frame rate: {e}"))?;
    if let Some(capture) = param.try_mut::<CaptureParm>() {
        let mut interval = capture.time_per_frame();
        interval.set_numerator(1);
        interval.set_denominator(spec.fps);
        capture.set_time_per_frame(interval);
        device
            .set_param(&mut param)
            .map_err(|e| format!("cannot set {} FPS: {e}", spec.fps))?;
    }

    // Exposure and white balance are best effort: not every camera has them.
    let set = |id: CtrlId, apply: &dyn Fn(&mut Value<&linux_video::Control>)| {
        if let Ok(control) = device.control(id) {
            let mut value = Value::from(&control);
            if device.get_control(&mut value).is_ok() {
                apply(&mut value);
                let _ = device.set_control(&value);
            }
        }
    };
    match spec.exposure {
        Some(exposure) => {
            set(CtrlId::ExposureAuto, &|v| {
                if let Some(mode) = v.try_mut::<u32>() {
                    *mode = 1; // manual
                }
            });
            set(CtrlId::ExposureAbsolute, &|v| {
                if let Some(time) = v.try_mut::<i32>() {
                    *time = exposure as i32;
                }
            });
        }
        None => set(CtrlId::ExposureAuto, &|v| {
            if let Some(mode) = v.try_mut::<u32>() {
                *mode = 3; // aperture priority (auto)
            }
        }),
    }
    set(CtrlId::AutoWhiteBalance, &|v| {
        if let Some(on) = v.try_mut::<bool>() {
            *on = true;
        }
    });

    let stream = device
        .stream::<In, Mmap>(ContentType::Video, 4)
        .map_err(|e| format!("cannot start streaming: {e}"))?;
    let mut pacer = Pacer::new(spec.analysis_hz.min(spec.fps));
    if let Ok(mut status) = shared.status.lock() {
        status.state = CameraState::Starting;
        status.error = None;
    }
    while !shared.stop.load(Ordering::Relaxed) {
        let buffer = stream.next().map_err(|e| format!("capture stopped: {e}"))?;
        let buffer = buffer.lock();
        let timestamp = buffer.timestamp::<Duration>().as_secs_f64();
        let jpeg: &[u8] = buffer.as_ref();
        if let Some(fps) = pacer.frame(timestamp)
            && let Ok(mut status) = shared.status.lock()
        {
            status.state = CameraState::Streaming;
            status.fps = fps;
            status.error = None;
        }
        if let Ok(mut latest) = shared.latest_jpeg.lock() {
            let latest = latest.get_or_insert_with(Vec::new);
            latest.clear();
            latest.extend_from_slice(jpeg);
        }
        if pacer.due(timestamp) {
            // A corrupt frame is skipped; the next one is due soon.
            if let Ok(luma) = GrayImage::decode_jpeg(jpeg) {
                match samples.try_send(Sample {
                    slot: spec.slot,
                    timestamp,
                    luma,
                }) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => return Ok(()),
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacer_selects_frames_at_the_analysis_rate() {
        let mut pacer = Pacer::new(10);
        let chosen = (0..90).filter(|i| pacer.due(f64::from(*i) / 30.0)).count();
        assert_eq!(chosen, 30, "3 s at 30 FPS analysed at 10 Hz");
    }

    #[test]
    fn pacer_recovers_after_a_gap_without_a_burst() {
        let mut pacer = Pacer::new(10);
        assert!(pacer.due(0.0));
        assert!(pacer.due(5.0), "first frame after a long gap");
        assert!(!pacer.due(5.033));
        assert!(pacer.due(5.1));
    }

    #[test]
    fn pacer_reports_delivered_fps_every_two_seconds() {
        let mut pacer = Pacer::new(10);
        let reports: Vec<f64> = (0..=150)
            .filter_map(|i| pacer.frame(f64::from(i) / 30.0))
            .collect();
        assert_eq!(reports.len(), 2);
        assert!((reports[0] - 30.0).abs() < 0.01);
    }

    #[test]
    fn missing_device_reports_an_error_status() {
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        let camera = Camera::start(
            CameraSpec {
                slot: 2,
                device: String::new(),
                width: 640,
                height: 480,
                fps: 30,
                exposure: None,
                analysis_hz: 10,
            },
            tx,
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while camera.status().state != CameraState::Error && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = camera.status();
        assert_eq!(status.state, CameraState::Error);
        assert!(status.error.unwrap().contains("no device"));
    }
}
