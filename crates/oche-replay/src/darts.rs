//! Replays a recorded session through the live detector.
//!
//! Frames are sampled per camera at the analysis rate the daemon would use,
//! decoded straight to luminance as live capture does, and pushed to
//! [`oche_vision::live::Detector`] in timestamp order. Recorded results
//! therefore check the same code that runs on the board.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use oche_calibration::Homography;
use oche_domain::board::{BoardDimensions, BoardPoint};
use oche_vision::image::RgbImage;
use oche_vision::live::{CameraChange, Detector, Event, LineMapper, Settings};
use oche_vision::mask::GrayImage;

use crate::CameraRecording;

/// Runs a session at `analysis_hz` per camera and returns events with their
/// timestamps relative to the first frame.
pub fn analyse(
    cameras: &[CameraRecording],
    calibration: &BTreeMap<u8, Homography>,
    analysis_hz: u32,
) -> Result<Vec<Event>> {
    let period = 1.0 / f64::from(analysis_hz.max(1));
    let mut schedule = Vec::new();
    for (c, camera) in cameras.iter().enumerate() {
        let mut next_due = f64::NEG_INFINITY;
        for (i, frame) in camera.frames.iter().enumerate() {
            // Same rule as the capture pacer: stay on the grid unless behind.
            if frame.timestamp + 1e-3 >= next_due {
                next_due = if frame.timestamp - next_due > period {
                    frame.timestamp + period
                } else {
                    next_due + period
                };
                schedule.push((frame.timestamp, c, i));
            }
        }
    }
    schedule.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mappers = cameras
        .iter()
        .map(|c| {
            let mapper = calibration
                .get(&c.camera)
                .map(|h| Box::new(*h) as Box<dyn LineMapper>);
            (c.camera, mapper)
        })
        .collect();
    let mut detector = Detector::new(mappers, Settings::default()).keep_detail();
    let mut events = Vec::new();
    for (timestamp, c, i) in schedule {
        let camera = &cameras[c];
        let bytes = camera.read(&camera.frames[i])?;
        let luma = GrayImage::decode_jpeg(&bytes)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("cam{} frame {i}", camera.camera))?;
        events.extend(detector.push(camera.camera, timestamp, luma));
    }
    Ok(events)
}

/// Grey frame with the change mask in red, shaft slice midpoints in yellow,
/// the fitted shaft line in cyan, and the fused impact (if any) in magenta.
pub fn overlay(
    change: &CameraChange,
    homography: Option<&Homography>,
    impact: Option<BoardPoint>,
) -> Option<RgbImage> {
    let (mask, gray) = change.detail.as_ref()?;
    let mut image = RgbImage::new(gray.width, gray.height);
    for (i, v) in gray.data.iter().enumerate() {
        let colour = if mask[i] {
            [255, v / 3, v / 3]
        } else {
            [*v, *v, *v]
        };
        image.data[i * 3..i * 3 + 3].copy_from_slice(&colour);
    }
    if let Some(shaft) = &change.shaft {
        // Extend past both ends so the line visibly reaches the board.
        let [a, b] = shaft.ends;
        let length = (b.0 - a.0).hypot(b.1 - a.1);
        let mut t = -0.5 * length;
        while t <= 1.5 * length {
            let (x, y) = (a.0 + shaft.direction.0 * t, a.1 + shaft.direction.1 * t);
            image.set(x.round() as i64, y.round() as i64, [0, 255, 255]);
            t += 0.5;
        }
        for (x, y) in &shaft.midpoints {
            image.set(x.round() as i64, y.round() as i64, [255, 255, 0]);
        }
    }
    if let Some(p) = homography.zip(impact).and_then(|(h, p)| h.to_image(p)) {
        for d in -6..=6 {
            for w in -1..=1 {
                image.set(
                    p.x.round() as i64 + d,
                    p.y.round() as i64 + w,
                    [255, 0, 255],
                );
                image.set(
                    p.x.round() as i64 + w,
                    p.y.round() as i64 + d,
                    [255, 0, 255],
                );
            }
        }
    }
    Some(image)
}

pub fn describe(point: BoardPoint, board: &BoardDimensions) -> String {
    let hit = board.hit(point);
    format!(
        "{} ({:.1}, {:.1}) mm, {:.1} mm from a wire",
        hit.label(),
        point.x,
        point.y,
        hit.wire_distance_mm
    )
}

pub fn print_changes(changes: &[CameraChange]) {
    for c in changes {
        let region = c.region.map_or("no region".into(), |(count, axis)| {
            format!(
                "region {count} px, spread {:.1}/{:.1}",
                axis.spread.0, axis.spread.1
            )
        });
        let region = match &c.shaft {
            Some(s) => format!(
                "{region}, shaft {} slices rms {:.2} px",
                s.midpoints.len(),
                s.rms_px
            ),
            None => format!("{region}, no shaft"),
        };
        println!(
            "  cam{}: {} changed px, arrival {:+.1}, {region}, line {}",
            c.slot,
            c.changed_pixels,
            c.arrival_score,
            if c.line.is_some() { "yes" } else { "no" }
        );
    }
}
