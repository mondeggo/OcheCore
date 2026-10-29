//! Exploratory offline dart analysis of a recorded session.
//!
//! Splits the session into still periods common to all cameras, compares each
//! still period with the previous one, and fits the barrel and shaft centre
//! line of the largest changed region per camera. Axes become board lines that are fused
//! into an impact. Parameters are exploratory starting points, not tuned or
//! validated values.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use oche_calibration::{Homography, Pixel};
use oche_domain::board::{BoardDimensions, BoardPoint};
use oche_vision::image::RgbImage;
use oche_vision::mask::{Axis, GrayImage, ShaftFit, dilate, label, principal_axis, shaft_axis};
use oche_vision::{FusionLimits, LineObservation, fuse_lines};

use crate::CameraRecording;

pub struct Settings {
    /// Analyse every n-th frame for motion.
    pub stride: usize,
    pub motion_factor: usize,
    pub motion_threshold: u8,
    /// Changed fraction of the downsampled frame below which a sample is still.
    pub still_fraction: f64,
    pub min_still_seconds: f64,
    pub change_threshold: u8,
    pub min_region: usize,
    /// Dilation radius used only to group mask pieces into one region.
    pub group_radius: usize,
    pub min_elongation: f64,
    /// Slices wider than this multiple of the narrow-quartile width are flight.
    pub max_width_ratio: f64,
    pub min_slices: usize,
    pub line_sigma_mm: f64,
    /// Uncertainty of a principal-axis line used when no shaft fit exists.
    pub fallback_sigma_mm: f64,
    pub limits: FusionLimits,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            stride: 3,
            motion_factor: 4,
            motion_threshold: 15,
            still_fraction: 0.0005,
            min_still_seconds: 1.0,
            change_threshold: 30,
            min_region: 80,
            group_radius: 4,
            min_elongation: 3.0,
            max_width_ratio: 2.0,
            min_slices: 20,
            line_sigma_mm: 2.0,
            fallback_sigma_mm: 4.0,
            limits: FusionLimits {
                min_angle_deg: 10.0,
                max_residual_sigma: 3.0,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Still {
    pub start: f64,
    pub end: f64,
}

pub struct CameraChange {
    pub camera: u8,
    pub changed_pixels: usize,
    /// Positive when the new still frame differs more from the first (empty)
    /// still frame than the previous one did, i.e. something arrived.
    pub arrival_score: f64,
    pub region: Option<(usize, Axis)>,
    pub shaft: Option<ShaftFit>,
    pub line: Option<LineObservation>,
    pub mask: Vec<bool>,
    pub after: GrayImage,
}

pub struct Event {
    pub before: Still,
    pub after: Still,
    pub cameras: Vec<CameraChange>,
}

fn decode_luma(camera: &CameraRecording, index: usize) -> Result<GrayImage> {
    let bytes = camera.read(&camera.frames[index])?;
    Ok(RgbImage::decode_jpeg(&bytes)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("cam{} frame {index}", camera.camera))?
        .luma())
}

/// Still flags at sampled frame indices, per camera.
fn motion(camera: &CameraRecording, settings: &Settings) -> Result<Vec<(f64, bool)>> {
    let mut previous: Option<GrayImage> = None;
    let mut samples = Vec::new();
    for index in (0..camera.frames.len()).step_by(settings.stride) {
        let small = decode_luma(camera, index)?.downsample(settings.motion_factor);
        let still = previous.as_ref().is_some_and(|p| {
            let changed = p
                .changed(&small, settings.motion_threshold)
                .iter()
                .filter(|c| **c)
                .count();
            (changed as f64) < settings.still_fraction * small.data.len() as f64
        });
        samples.push((camera.frames[index].timestamp, still));
        previous = Some(small);
    }
    Ok(samples)
}

/// Periods during which every camera is still for at least the minimum time.
pub fn stills(cameras: &[CameraRecording], settings: &Settings) -> Result<Vec<Still>> {
    let motions: Vec<_> = cameras
        .iter()
        .map(|c| motion(c, settings))
        .collect::<Result<_>>()?;
    let still_at = |samples: &[(f64, bool)], t: f64| {
        samples
            .iter()
            .min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()))
            .is_some_and(|s| s.1)
    };
    let mut periods = Vec::new();
    let mut start: Option<f64> = None;
    let mut last = 0.0;
    for &(t, _) in &motions[0] {
        if motions.iter().all(|m| still_at(m, t)) {
            start.get_or_insert(t);
            last = t;
        } else if let Some(s) = start.take()
            && last - s >= settings.min_still_seconds
        {
            periods.push(Still {
                start: s,
                end: last,
            });
        }
    }
    if let Some(s) = start
        && last - s >= settings.min_still_seconds
    {
        periods.push(Still {
            start: s,
            end: last,
        });
    }
    Ok(periods)
}

/// Mean of up to five frames from the middle of a still period.
fn reference(camera: &CameraRecording, still: Still) -> Result<GrayImage> {
    let indices: Vec<usize> = camera
        .frames
        .iter()
        .enumerate()
        .filter(|(_, f)| f.timestamp >= still.start && f.timestamp <= still.end)
        .map(|(i, _)| i)
        .collect();
    let middle = indices.len() / 2;
    let chosen: Vec<usize> = indices
        .iter()
        .skip(middle.saturating_sub(4))
        .step_by(2)
        .take(5)
        .copied()
        .collect();
    let frames: Vec<GrayImage> = chosen
        .iter()
        .map(|&i| decode_luma(camera, i))
        .collect::<Result<_>>()?;
    GrayImage::mean(&frames.iter().collect::<Vec<_>>()).context("no frames in still period")
}

pub fn analyse(
    cameras: &[CameraRecording],
    calibration: &BTreeMap<u8, Homography>,
    settings: &Settings,
) -> Result<(Vec<Still>, Vec<Event>)> {
    let periods = stills(cameras, settings)?;
    let mut references: Vec<Vec<GrayImage>> = Vec::new();
    for camera in cameras {
        references.push(
            periods
                .iter()
                .map(|&p| reference(camera, p))
                .collect::<Result<_>>()?,
        );
    }
    let mut events = Vec::new();
    for k in 1..periods.len() {
        let mut changes = Vec::new();
        for (c, camera) in cameras.iter().enumerate() {
            let (empty, before, after) =
                (&references[c][0], &references[c][k - 1], &references[c][k]);
            let mask = before.changed(after, settings.change_threshold);
            let changed_pixels = mask.iter().filter(|m| **m).count();
            let arrival_score = if changed_pixels == 0 {
                0.0
            } else {
                mask.iter()
                    .enumerate()
                    .filter(|(_, m)| **m)
                    .map(|(i, _)| {
                        f64::from(after.data[i].abs_diff(empty.data[i]))
                            - f64::from(before.data[i].abs_diff(empty.data[i]))
                    })
                    .sum::<f64>()
                    / changed_pixels as f64
            };
            // Group nearby pieces of one dart (flight, shaft, barrel) that small
            // gaps in the mask split apart, but fit only changed pixels.
            let grouped = dilate(&mask, after.width, after.height, settings.group_radius);
            let (labels, regions) = label(&grouped, after.width);
            let points_of = |label: u32| {
                labels
                    .iter()
                    .zip(&mask)
                    .enumerate()
                    .filter(move |(_, (l, m))| **l == label && **m)
                    .map(|(i, _)| ((i % after.width) as f64, (i / after.width) as f64))
            };
            let largest = regions
                .iter()
                .map(|r| (r.label, points_of(r.label).count()))
                .filter(|(_, count)| *count >= settings.min_region)
                .max_by_key(|(_, count)| *count);
            let region = largest.and_then(|(label, count)| {
                principal_axis(points_of(label)).map(|axis| (count, axis))
            });
            // The principal axis orients the slices. The line comes from the
            // barrel and shaft centre line, which the flight cannot pull; when
            // too little shaft is visible, the principal axis is used with a
            // larger uncertainty so precise views dominate the fusion.
            let elongated = region.filter(|(_, axis)| {
                axis.spread.0 >= settings.min_elongation * axis.spread.1.max(0.5)
            });
            let shaft = largest.zip(elongated).and_then(|((label, _), (_, axis))| {
                let points: Vec<_> = points_of(label).collect();
                shaft_axis(
                    &points,
                    &axis,
                    settings.max_width_ratio,
                    settings.min_slices,
                )
            });
            let fitted = match (&shaft, elongated) {
                (Some(shaft), _) => Some((shaft.point, shaft.direction, settings.line_sigma_mm)),
                (None, Some((_, axis))) => {
                    Some((axis.centroid, axis.direction, settings.fallback_sigma_mm))
                }
                (None, None) => None,
            };
            let line = fitted.and_then(|((cx, cy), (dx, dy), sigma_mm)| {
                let homography = calibration.get(&camera.camera)?;
                let line = homography.line_to_board(
                    Pixel::new(cx, cy),
                    Pixel::new(cx + dx * 50.0, cy + dy * 50.0),
                )?;
                Some(LineObservation {
                    camera: camera.camera,
                    line,
                    sigma_mm,
                })
            });
            changes.push(CameraChange {
                camera: camera.camera,
                changed_pixels,
                arrival_score,
                region,
                shaft,
                line,
                mask,
                after: after.clone(),
            });
        }
        events.push(Event {
            before: periods[k - 1],
            after: periods[k],
            cameras: changes,
        });
    }
    Ok((periods, events))
}

impl Event {
    pub fn is_arrival(&self) -> bool {
        let scores: Vec<f64> = self.cameras.iter().map(|c| c.arrival_score).collect();
        scores.iter().filter(|s| **s > 0.0).count() * 2 > scores.len()
    }

    pub fn lines(&self) -> Vec<LineObservation> {
        self.cameras.iter().filter_map(|c| c.line).collect()
    }
}

/// Grey frame with the change mask in red, shaft slice midpoints in yellow,
/// the fitted shaft line in cyan, and the fused impact (if any) in magenta.
pub fn overlay(
    change: &CameraChange,
    homography: Option<&Homography>,
    impact: Option<BoardPoint>,
) -> RgbImage {
    let gray = &change.after;
    let mut image = RgbImage::new(gray.width, gray.height);
    for (i, v) in gray.data.iter().enumerate() {
        let colour = if change.mask[i] {
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
    image
}

pub fn fuse(
    event: &Event,
    settings: &Settings,
) -> Result<oche_vision::Impact, oche_vision::Abstention> {
    fuse_lines(&event.lines(), settings.limits)
}

pub fn describe(point: BoardPoint) -> String {
    let hit = BoardDimensions::wdf().hit(point);
    format!(
        "{} ({:.1}, {:.1}) mm, {:.1} mm from a wire",
        hit.label(),
        point.x,
        point.y,
        hit.wire_distance_mm
    )
}
