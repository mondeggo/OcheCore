//! Read-only, bounded V4L2 discovery. Never configures devices or starts a stream.
use oche_domain::CameraInventory;

#[cfg(not(target_os = "linux"))]
pub fn discover() -> CameraInventory {
    CameraInventory {
        devices: vec![],
        warnings: vec!["Camera discovery requires Linux V4L2".into()],
    }
}

#[cfg(target_os = "linux")]
pub fn discover() -> CameraInventory {
    use linux_video::{
        Device,
        types::{BufferType, CapabilityFlag, Fract, FrmSizeDiscrete},
    };
    use oche_domain::{CameraDevice, CameraMode};
    use std::{
        fs,
        path::Path,
        time::{Duration, Instant},
    };
    let mut result = CameraInventory::default();
    let started = Instant::now();
    let mut paths = match fs::read_dir("/dev") {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("video"))
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
            })
            .collect::<Vec<_>>(),
        Err(error) => {
            result.warnings.push(error.to_string());
            return result;
        }
    };
    paths.sort();
    if paths.len() > 32 {
        result
            .warnings
            .push("Device discovery limited to 32 nodes".into());
        paths.truncate(32);
    }
    for path in paths {
        if started.elapsed() > Duration::from_secs(2) {
            result
                .warnings
                .push("Camera scan time budget reached; results are partial".into());
            break;
        }
        let mut aliases = vec![path.to_string_lossy().into_owned()];
        for directory in ["/dev/v4l/by-path", "/dev/v4l/by-id"] {
            if let Ok(entries) = fs::read_dir(directory) {
                let mut links = entries
                    .filter_map(Result::ok)
                    .take(128)
                    .map(|entry| entry.path())
                    .filter(|alias| {
                        fs::canonicalize(alias).ok().as_deref() == Some(Path::new(&path))
                    })
                    .collect::<Vec<_>>();
                links.sort();
                aliases.extend(links.iter().map(|p| p.to_string_lossy().into_owned()));
            }
        }
        let mut camera = CameraDevice {
            path: aliases
                .get(1)
                .cloned()
                .unwrap_or_else(|| aliases[0].clone()),
            aliases,
            name: path.to_string_lossy().into_owned(),
            modes: vec![],
            error: None,
        };
        let probe = (|| -> std::io::Result<bool> {
            let dev = Device::open(&path)?;
            let caps = dev.capabilities()?;
            let flags = if caps.capabilities().contains(CapabilityFlag::DeviceCaps) {
                caps.device_capabilities()
            } else {
                caps.capabilities()
            };
            if !BufferType::VideoCapture.is_supported(flags)
                && !BufferType::VideoCaptureMplane.is_supported(flags)
            {
                return Ok(false);
            }
            camera.name = caps.card().to_owned();
            for kind in [BufferType::VideoCapture, BufferType::VideoCaptureMplane] {
                if !kind.is_supported(flags) {
                    continue;
                }
                let mut formats = dev.formats(kind);
                for fi in 0..16 {
                    let Some(format) = formats.fetch_next()? else {
                        break;
                    };
                    if fi == 15 {
                        camera.error = Some("Format list may be truncated".into());
                    }
                    let mut sizes = dev.sizes(format.pixel_format());
                    for si in 0..64 {
                        if started.elapsed() > Duration::from_secs(2) {
                            camera.error = Some("Scan time budget reached".into());
                            return Ok(true);
                        }
                        let Some(size) = sizes.fetch_next()? else {
                            break;
                        };
                        let Some(size) = size.try_ref::<FrmSizeDiscrete>() else {
                            camera.error =
                                Some("Continuous/stepwise sizes are not selectable yet".into());
                            continue;
                        };
                        if si == 63 {
                            camera.error = Some("Size list may be truncated".into());
                        }
                        let mut mode = CameraMode {
                            width: size.width(),
                            height: size.height(),
                            pixel_format: format.pixel_format().to_string(),
                            fps: vec![],
                        };
                        let mut intervals =
                            dev.intervals(format.pixel_format(), mode.width, mode.height);
                        for ii in 0..32 {
                            let Some(interval) = intervals.fetch_next()? else {
                                break;
                            };
                            if ii == 31 {
                                camera.error = Some("Frame-rate list may be truncated".into());
                            }
                            if let Some(value) = interval.try_ref::<Fract>() {
                                if let Some(fps) =
                                    integer_fps(value.numerator(), value.denominator())
                                {
                                    mode.fps.push(fps);
                                } else {
                                    camera.error = Some(
                                        "Fractional/out-of-range FPS is not selectable yet".into(),
                                    );
                                }
                            } else {
                                camera.error =
                                    Some("Continuous/stepwise FPS is not selectable yet".into());
                            }
                        }
                        mode.fps.sort_unstable();
                        mode.fps.dedup();
                        camera.modes.push(mode);
                        if camera.modes.len() >= 256 {
                            camera.error = Some("Mode list limited to 256 entries".into());
                            return Ok(true);
                        }
                    }
                }
            }
            Ok(true)
        })();
        match probe {
            Ok(false) => continue,
            Ok(true) => {}
            Err(error) => camera.error = Some(error.to_string()),
        }
        result.devices.push(camera);
    }
    result
}

#[cfg(any(target_os = "linux", test))]
fn integer_fps(numerator: u32, denominator: u32) -> Option<u32> {
    if numerator == 0 || !denominator.is_multiple_of(numerator) {
        return None;
    }
    let fps = denominator / numerator;
    (1..=240).contains(&fps).then_some(fps)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_intervals_are_not_rounded_to_invented_rates() {
        assert_eq!(integer_fps(1, 30), Some(30));
        assert_eq!(integer_fps(2, 120), Some(60));
        assert_eq!(integer_fps(1001, 30000), None);
        assert_eq!(integer_fps(0, 30), None);
        assert_eq!(integer_fps(1, 0), None);
    }
}
