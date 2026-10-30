//! Live camera pipeline for the daemon: capture, detection, and calibration.
//!
//! A supervisor thread follows the engine. While the board is running in live
//! mode it keeps one capture thread per assigned camera and one detector
//! thread; stopping, standby, or any change to camera settings, detection
//! speed, or calibration stops them and, if still wanted, starts them afresh.
//! The engine lock is only held briefly, never during capture, decoding, or
//! calibration.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oche_calibration::detect::detect;
use oche_calibration::{Homography, Pixel, refine};
use oche_capture::{Camera, CameraSpec};
use oche_domain::board::{BoardDimensions, BoardPoint};
use oche_domain::*;
use oche_vision::image::RgbImage;
use oche_vision::live::{Detector, Event, LineMapper, Settings};
use oche_vision::mask::dart_ends;

use crate::{Engine, Observation, error};

/// A calibration is rejected when any camera's wire points fit worse than this.
const MAX_WIRE_RMS_MM: f64 = 1.0;
const MIN_WIRE_POINTS: usize = 300;
/// Analysed frames waiting for the detector; older samples are dropped.
const SAMPLE_QUEUE: usize = 6;

#[derive(Debug, Clone, PartialEq)]
struct Plan {
    specs: Vec<CameraSpec>,
    calibration: Option<u64>,
}

struct Running {
    plan: Plan,
    cameras: Vec<Camera>,
    detector: Option<JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        // Stopping the cameras drops every sender, which ends the detector.
        self.cameras.clear();
        if let Some(detector) = self.detector.take() {
            let _ = detector.join();
        }
    }
}

pub struct Pipeline {
    engine: Arc<Mutex<Engine>>,
    running: Mutex<Option<Running>>,
    calibrating: Mutex<()>,
    /// Empty-board frames taken when orientation began, one per camera.
    orienting: Mutex<Option<Vec<(u8, RgbImage)>>>,
    stop: AtomicBool,
    supervisor: Mutex<Option<JoinHandle<()>>>,
}

impl Pipeline {
    pub fn spawn(engine: Arc<Mutex<Engine>>) -> Arc<Self> {
        let pipeline = Arc::new(Self {
            engine,
            running: Mutex::new(None),
            calibrating: Mutex::new(()),
            orienting: Mutex::new(None),
            stop: AtomicBool::new(false),
            supervisor: Mutex::new(None),
        });
        let worker = pipeline.clone();
        let handle = std::thread::Builder::new()
            .name("pipeline".into())
            .spawn(move || worker.supervise())
            .ok();
        *pipeline.supervisor.lock().unwrap() = handle;
        pipeline
    }

    /// Stops the supervisor and closes every camera.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.supervisor.lock().unwrap().take() {
            let _ = handle.join();
        }
        *self.running.lock().unwrap() = None;
    }

    /// The most recent JPEG from a streaming camera.
    pub fn latest_jpeg(&self, slot: u8) -> Option<Vec<u8>> {
        let running = self.running.lock().ok()?;
        running
            .as_ref()?
            .cameras
            .iter()
            .find(|c| c.spec().slot == slot)?
            .latest_jpeg()
    }

    fn supervise(&self) {
        while !self.stop.load(Ordering::Relaxed) {
            let (plan, board, mappers, configured) = {
                let Ok(engine) = self.engine.lock() else {
                    return;
                };
                plan_for(&engine)
            };
            let statuses = {
                let mut running = self.running.lock().unwrap();
                match (&plan, running.as_ref()) {
                    (Some(plan), Some(current)) if current.plan == *plan => {}
                    (Some(plan), _) => {
                        *running = None;
                        *running = Some(start(plan.clone(), board, mappers, self.engine.clone()));
                    }
                    (None, Some(_)) => *running = None,
                    (None, None) => {}
                }
                configured
                    .iter()
                    .map(|&slot| {
                        running
                            .as_ref()
                            .and_then(|r| r.cameras.iter().find(|c| c.spec().slot == slot))
                            .map(Camera::status)
                            .unwrap_or(CameraStatus {
                                slot,
                                state: CameraState::Off,
                                fps: 0.0,
                                error: None,
                            })
                    })
                    .collect()
            };
            if let Ok(mut engine) = self.engine.lock() {
                engine.set_camera_status(statuses);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn assigned_cameras(&self) -> Result<Vec<CameraConfig>, ApiError> {
        let engine = self
            .engine
            .lock()
            .map_err(|_| error("internal_error", "command authority unavailable"))?;
        if engine.state() != ServiceState::Running {
            return Err(error(
                "conflict",
                "start the board first; cameras capture only while running",
            ));
        }
        let cameras: Vec<_> = engine
            .config()
            .cameras
            .iter()
            .filter(|c| !c.device.is_empty())
            .cloned()
            .collect();
        if cameras.is_empty() {
            return Err(error(
                "invalid_request",
                "assign a device to a camera first",
            ));
        }
        Ok(cameras)
    }

    fn frame(&self, slot: u8) -> Result<RgbImage, ApiError> {
        let jpeg = self.wait_for_frame(slot).ok_or_else(|| {
            error(
                "conflict",
                format!("camera {slot} is not streaming; check its device and status"),
            )
        })?;
        RgbImage::decode_jpeg(&jpeg)
            .map_err(|e| error("internal_error", format!("camera {slot}: {e}")))
    }

    /// Step one of finding the 20: remember the empty board. Run with every
    /// dart removed; the next step needs one dart thrown at the 20.
    pub fn orient_begin(&self) -> Result<(), ApiError> {
        let cameras = self.assigned_cameras()?;
        let mut empty = Vec::new();
        for camera in &cameras {
            empty.push((camera.slot, self.frame(camera.slot)?));
        }
        *self.orienting.lock().unwrap() = Some(empty);
        Ok(())
    }

    /// Step two: locate the dart thrown at the 20 in every camera and return
    /// each camera's hint. A hint is only returned when the dart tip, mapped
    /// onto the board with that hint, lands in segment 20.
    pub fn orient_finish(&self) -> Result<Vec<(u8, [f64; 2])>, ApiError> {
        let empty =
            self.orienting.lock().unwrap().take().ok_or_else(|| {
                error("conflict", "start orientation first, with the board empty")
            })?;
        let wanted: Vec<u8> = self.assigned_cameras()?.iter().map(|c| c.slot).collect();
        let mut hints = Vec::new();
        for (slot, before) in &empty {
            if !wanted.contains(slot) {
                continue;
            }
            let after = self.frame(*slot)?;
            let found = dart_ends(&before.luma(), &after.luma(), 30, 80, 4).map_err(|reason| {
                error(
                    "calibration_failed",
                    format!(
                        "camera {slot}: no single dart found: {reason}. Throw one dart at the 20, let it settle, and keep the rest of the board empty"
                    ),
                )
            })?;
            let board = BoardDimensions::wdf();
            let failed = |what: String| {
                error(
                    "calibration_failed",
                    format!("camera {slot}: {what}. No hint was changed"),
                )
            };
            // The colours repeat every two segments, so any red double arc fits
            // the board equally well; use the dart's end to pick a tentative one,
            // then measure where both ends fall relative to the board's geometry.
            let tentative = Pixel::new(found.ends[0].0.round(), found.ends[0].1.round());
            let detection = detect(before, tentative, &board)
                .map_err(|e| failed(format!("{e}; make sure the board was empty and lit")))?;
            let homography = &detection.fit.homography;
            // Red wedges sit at even positions around the board, 36° apart.
            let red_wedge = |(x, y): (f64, f64)| {
                let p = homography.to_board(Pixel::new(x, y))?;
                let angle = p.x.atan2(p.y).to_degrees().rem_euclid(360.0);
                Some(((angle / 36.0).round() as usize % 10) * 2)
            };
            let wedges = found.ends.map(red_wedge);
            // The steel tip is black on a black board and the flight is often out
            // of frame, so only the barrel is visible. Its end nearer the board
            // centre is the tip side; both ends are used when they agree.
            let distance =
                |(x, y): (f64, f64)| (x - detection.centre.x).hypot(y - detection.centre.y);
            let tip_side = found
                .tip
                .unwrap_or_else(|| usize::from(distance(found.ends[1]) < distance(found.ends[0])));
            let wedge = match wedges {
                // A flight in view says which end is the tip, so use that end alone.
                [Some(a), Some(b)] if a == b && found.tip.is_none() => a,
                wedges => {
                    wedges[tip_side].ok_or_else(|| failed("the dart is off the board".into()))?
                }
            };
            let target = homography
                .to_image(BoardPoint::polar_cw_from_top(
                    (board.double_inner + board.double_outer) / 2.0,
                    18.0 * wedge as f64,
                ))
                .ok_or_else(|| failed("the board geometry is degenerate".into()))?;
            let hint = Pixel::new(target.x.round(), target.y.round());
            // The hint must now yield a labelling whose double arc there is 20.
            let check = detect(before, hint, &board).map_err(|e| failed(e.to_string()))?;
            let label = check
                .fit
                .homography
                .to_board(hint)
                .map(|p| board.hit(p).label())
                .unwrap_or_default();
            if label != "D20" {
                return Err(failed(format!(
                    "the stored hint would label as {label}, not D20"
                )));
            }
            hints.push((*slot, [hint.x, hint.y]));
        }
        if hints.len() != wanted.len() {
            return Err(error(
                "conflict",
                "cameras changed during orientation; start again. No hint was changed",
            ));
        }
        Ok(hints)
    }

    /// Measures the board and fits every selected camera from its latest
    /// frame. The board must be empty and the service running.
    pub fn calibrate(&self, camera: Option<u8>) -> Result<CalibrationRecord, ApiError> {
        let _only_one = self
            .calibrating
            .try_lock()
            .map_err(|_| error("busy", "a calibration is already running"))?;
        let (config, previous, state) = {
            let engine = self
                .engine
                .lock()
                .map_err(|_| error("internal_error", "command authority unavailable"))?;
            (
                engine.config().clone(),
                engine.calibration().cloned(),
                engine.state(),
            )
        };
        if state != ServiceState::Running {
            return Err(error(
                "conflict",
                "start the board first; cameras capture only while running",
            ));
        }
        let selected: Vec<&CameraConfig> = config
            .cameras
            .iter()
            .filter(|c| !c.device.is_empty() && camera.is_none_or(|slot| c.slot == slot))
            .collect();
        if selected.is_empty() {
            return Err(error(
                "invalid_request",
                "assign a device to the camera before calibrating",
            ));
        }

        let mut views = Vec::new();
        for camera in &selected {
            let jpeg = self.wait_for_frame(camera.slot).ok_or_else(|| {
                error(
                    "conflict",
                    format!(
                        "camera {} is not streaming; check its device and status",
                        camera.slot
                    ),
                )
            })?;
            let image = RgbImage::decode_jpeg(&jpeg)
                .map_err(|e| error("internal_error", format!("camera {}: {e}", camera.slot)))?;
            let hint = hint_for(camera, previous.as_ref()).ok_or_else(|| {
                error(
                    "invalid_request",
                    format!(
                        "camera {0} needs an orientation hint: set cameras.{0}.hint to the pixel x,y of the 20 segment (`oche-core cameras snapshot --camera {0}` saves a frame to look at)",
                        camera.slot
                    ),
                )
            })?;
            let detection = detect(&image, hint, &BoardDimensions::wdf()).map_err(|e| {
                error(
                    "calibration_failed",
                    format!(
                        "camera {}: {e}; make sure the board is empty and lit",
                        camera.slot
                    ),
                )
            })?;
            views.push((camera, image.luma(), detection.fit.homography));
        }

        let mut settings = refine::Settings::default();
        let start = match (camera, previous.as_ref()) {
            (Some(_), Some(previous)) => {
                // One camera against the board the others already measured.
                settings.fit_radii = false;
                previous.board
            }
            _ => BoardDimensions::wdf(),
        };
        let inputs: Vec<_> = views.iter().map(|(_, gray, h)| (gray, *h)).collect();
        let result = refine::refine(&inputs, &start, &settings)
            .map_err(|e| error("calibration_failed", e))?;
        let mut cameras = Vec::new();
        for (i, (camera, _, _)) in views.iter().enumerate() {
            let fit = &result.views[i];
            let points = fit.ring_points.iter().sum::<usize>() + fit.radial_points;
            if fit.rms_mm.is_nan() || fit.rms_mm > MAX_WIRE_RMS_MM || points < MIN_WIRE_POINTS {
                return Err(error(
                    "calibration_failed",
                    format!(
                        "camera {} fits the board poorly ({:.2} mm over {points} wire points); check focus, lighting, and the hint. No calibration was changed",
                        camera.slot, fit.rms_mm
                    ),
                ));
            }
            cameras.push(CameraCalibration {
                slot: camera.slot,
                device: camera.device.clone(),
                width: camera.width,
                height: camera.height,
                rotation: camera.rotation,
                to_board: result.homographies[i].matrix(),
                wire_rms_mm: fit.rms_mm,
                wire_points: points,
            });
        }
        // Keep other cameras' calibrations that still match their settings.
        if camera.is_some()
            && let Some(previous) = &previous
        {
            for other in &config.cameras {
                if cameras.iter().all(|c| c.slot != other.slot)
                    && let Some(kept) = previous.for_camera(other)
                {
                    cameras.push(kept.clone());
                }
            }
        }
        cameras.sort_by_key(|c| c.slot);
        let record = CalibrationRecord {
            schema_version: CALIBRATION_VERSION,
            created_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            board: result.board,
            cameras,
        };
        record
            .validate()
            .map_err(|e| error("calibration_failed", e))?;
        Ok(record)
    }

    fn wait_for_frame(&self, slot: u8) -> Option<Vec<u8>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(jpeg) = self.latest_jpeg(slot) {
                return Some(jpeg);
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// The configured hint, or the 20 segment as the previous calibration saw it.
fn hint_for(camera: &CameraConfig, previous: Option<&CalibrationRecord>) -> Option<Pixel> {
    if let Some([x, y]) = camera.hint {
        return Some(Pixel::new(x, y));
    }
    let matrix = previous?.for_camera(camera)?.to_board;
    Homography::from_matrix(matrix)?.to_image(BoardPoint::new(0.0, 140.0))
}

type Mappers = Vec<(u8, Option<Box<dyn LineMapper>>)>;

/// What should run now, the board to score on, mappers, and configured slots.
fn plan_for(engine: &Engine) -> (Option<Plan>, BoardDimensions, Mappers, Vec<u8>) {
    let config = engine.config();
    let configured = config.cameras.iter().map(|c| c.slot).collect();
    let calibration = engine.calibration();
    let board = calibration.map_or_else(BoardDimensions::wdf, |c| c.board);
    let wanted = engine.mode() == Mode::Live && engine.state() == ServiceState::Running;
    let cameras: Vec<&CameraConfig> = config
        .cameras
        .iter()
        .filter(|c| !c.device.is_empty())
        .collect();
    if !wanted || cameras.is_empty() {
        return (None, board, Vec::new(), configured);
    }
    let analysis_hz = config.detection.speed.analysis_hz();
    let specs = cameras
        .iter()
        .map(|c| CameraSpec {
            slot: c.slot,
            device: c.device.clone(),
            width: c.width,
            height: c.height,
            fps: c.fps,
            exposure: c.exposure,
            analysis_hz,
        })
        .collect();
    let mappers = cameras
        .iter()
        .map(|c| {
            let mapper = calibration
                .and_then(|record| record.for_camera(c))
                .and_then(|cal| Homography::from_matrix(cal.to_board))
                .map(|h| Box::new(h) as Box<dyn LineMapper>);
            (c.slot, mapper)
        })
        .collect();
    let plan = Plan {
        specs,
        calibration: calibration.map(|c| c.created_unix_ms),
    };
    (Some(plan), board, mappers, configured)
}

fn start(
    plan: Plan,
    board: BoardDimensions,
    mappers: Mappers,
    engine: Arc<Mutex<Engine>>,
) -> Running {
    let (samples, receiver) = mpsc::sync_channel(SAMPLE_QUEUE);
    let cameras = plan
        .specs
        .iter()
        .map(|spec| Camera::start(spec.clone(), samples.clone()))
        .collect();
    drop(samples);
    let detector = std::thread::Builder::new()
        .name("detector".into())
        .spawn(move || {
            let mut detector = Detector::new(mappers, Settings::default());
            let mut reported = false;
            while let Ok(sample) = receiver.recv() {
                let event = detector.push(sample.slot, sample.timestamp, sample.luma);
                let motion = detector.in_motion();
                if motion != reported {
                    reported = motion;
                    match engine.lock() {
                        Ok(mut engine) => engine.set_motion(motion),
                        Err(_) => return,
                    }
                }
                let Some(event) = event else {
                    continue;
                };
                let observation = match event {
                    Event::Baseline { .. } => Observation::Baseline,
                    Event::Disturbance { .. } => Observation::Disturbance,
                    Event::Removal { empty, .. } => Observation::Removal { empty },
                    Event::Arrival { impact, .. } => Observation::Arrival(impact.ok().map(|i| {
                        let hit = board.hit(i.point);
                        (hit.label(), hit.score())
                    })),
                };
                match engine.lock() {
                    Ok(mut engine) => engine.observe(observation),
                    Err(_) => return,
                }
            }
        })
        .ok();
    Running {
        plan,
        cameras,
        detector,
    }
}
