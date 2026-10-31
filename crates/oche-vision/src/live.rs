//! Streaming dart detection over analysed frames from every camera.
//!
//! A camera is still when its downsampled frame barely changes from the
//! previous one. Once every camera has been still for a minimum time, the most
//! recent still frames are averaged into a reference and compared with the
//! previous reference. The largest changed region per camera gives a shaft
//! line on the board, the lines are fused, and the change is reported as an
//! arrival or a removal. The first still scene is taken as the empty board.
//!
//! The thresholds are the ones validated on the 2026-10-29 recordings at
//! 10 analyses per second; they are starting values, not tuned limits.

use std::collections::VecDeque;

use oche_domain::board::BoardLine;

use crate::compensate::{CompensateOptions, DartMove, compensate};
use crate::mask::{
    Axis, GrayImage, RansacOptions, ShaftFit, dilate, flight_and_tip, join_aligned, label,
    narrow_points, principal_axis, ransac_lines, remove_thin, shaft_axis,
};
use crate::{Abstention, FusionLimits, Impact, LineObservation, fuse_lines};

/// Maps an image line through two pixels onto the board plane.
pub trait LineMapper: Send {
    fn line_to_board(&self, a: (f64, f64), b: (f64, f64)) -> Option<BoardLine>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub motion_factor: usize,
    pub motion_threshold: u8,
    /// Changed fraction of the downsampled frame below which a camera is still.
    pub still_fraction: f64,
    pub min_still_seconds: f64,
    /// Still frames averaged into a reference.
    pub reference_frames: usize,
    pub change_threshold: u8,
    pub min_region: usize,
    /// Move the darts already on the board to where the new scene shows them
    /// before looking for the new dart (see [`crate::compensate`]): a nudged
    /// dart then leaves no outline.
    pub compensate_moves: bool,
    /// Largest translation (px) and rotation (degrees) searched per old dart.
    pub move_search_px: i64,
    pub move_max_deg: f64,
    /// Lines narrower than `2 * radius + 1` px are removed from the fresh mask
    /// (see [`crate::mask::remove_thin`]); 0 disables it.
    pub fresh_open_radius: usize,
    /// Removed pixels within this distance of a solid shape are kept, so thin
    /// junctions of the new dart (shaft to flight) are not cut.
    pub fresh_reconnect_px: usize,
    /// Choose the dart's pieces by a line instead of by size: the line covering
    /// the most length over all fresh pixels is found first, and every group it
    /// passes through with at least `line_select_min_px` pixels is kept.
    /// Otherwise, or when no line is found, the largest group is kept (plus
    /// `join_aligned`).
    pub select_by_line: bool,
    pub line_select_min_px: usize,
    /// Also take groups that continue the chosen one along the dart's line
    /// (see [`crate::mask::join_aligned`]).
    pub join_aligned: bool,
    pub join_max_gap_px: f64,
    pub join_max_offset_px: f64,
    /// Candidate lines per camera from sequential RANSAC on the chosen region
    /// without its flight. When the cameras' main lines disagree, the fusion
    /// tries these and keeps the combination that agrees best. 0: off.
    pub line_candidates: usize,
    /// A shaft fit whose midpoints scatter more than this (RMS, px) likely
    /// mixes two things (the dart and a stray piece); the alternatives are then
    /// tried even when the cameras' main lines agree.
    pub poor_fit_rms_px: f64,
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
    /// A camera sees an empty board when the pixels still differing from the
    /// last empty board are at most this share of what the visit's arrivals
    /// changed. Darts leave holes, so an exact match is never expected. The
    /// board is empty when most cameras agree.
    pub empty_ratio: f64,
    /// Changed fraction above which a change is a disturbance (lighting, a
    /// moved camera, a large object) rather than darts.
    pub disturbance_fraction: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            motion_factor: 4,
            motion_threshold: 15,
            still_fraction: 0.0005,
            min_still_seconds: 1.0,
            reference_frames: 5,
            change_threshold: 30,
            min_region: 80,
            compensate_moves: true,
            move_search_px: 5,
            move_max_deg: 3.0,
            fresh_open_radius: 1,
            fresh_reconnect_px: 3,
            select_by_line: true,
            line_select_min_px: 20,
            join_aligned: true,
            join_max_gap_px: 40.0,
            join_max_offset_px: 6.0,
            line_candidates: 2,
            poor_fit_rms_px: 3.0,
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
            empty_ratio: 0.25,
            disturbance_fraction: 0.05,
        }
    }
}

/// What one camera saw change between two still scenes.
pub struct CameraChange {
    pub slot: u8,
    pub changed_pixels: usize,
    /// Of those, the pixels newly covered since the last scene: what the dart
    /// line is fitted to.
    pub fresh_pixels: usize,
    /// Positive when the new scene differs more from the empty board than the
    /// previous scene did, i.e. something arrived.
    pub arrival_score: f64,
    pub region: Option<(usize, Axis)>,
    pub shaft: Option<ShaftFit>,
    pub line: Option<LineObservation>,
    /// Two pixels on the fitted dart line, and the tip when a flight settled it,
    /// for drawing the detection over the frame.
    pub image_line: Option<[(f64, f64); 2]>,
    pub tip: Option<(f64, f64)>,
    /// How `line` was obtained.
    pub method: Option<LineMethod>,
    /// Other candidate lines, each with two image pixels on it, for the fusion
    /// to try when the cameras' main lines disagree.
    pub alternatives: Vec<([(f64, f64); 2], LineObservation)>,
    /// How the darts already on the board moved (see [`crate::compensate`]).
    pub moves: Vec<DartMove>,
    /// The change mask and new reference, kept only when requested.
    pub detail: Option<(Vec<bool>, GrayImage)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMethod {
    /// Centre line of the barrel and shaft.
    Shaft,
    /// From a visible flight to the farthest changed pixel.
    Flight,
    /// Principal axis of the changed pixels, when nothing better exists.
    PrincipalAxis,
    /// A sequential RANSAC alternative the fusion preferred.
    Ransac,
}

pub enum Event {
    /// The first still scene, taken as the empty board.
    Baseline { timestamp: f64 },
    Arrival {
        timestamp: f64,
        changes: Vec<CameraChange>,
        impact: Result<Impact, Abstention>,
    },
    Removal {
        timestamp: f64,
        changes: Vec<CameraChange>,
        /// Every camera now matches the empty board.
        empty: bool,
        /// Pixels per camera that still differ from the last empty board.
        remaining: Vec<usize>,
        /// Pixels per camera that this visit's arrivals changed.
        added: Vec<usize>,
    },
    /// A large change that is not darts; the new scene is accepted silently.
    Disturbance { timestamp: f64 },
}

struct Camera {
    slot: u8,
    mapper: Option<Box<dyn LineMapper>>,
    previous_small: Option<GrayImage>,
    still_since: Option<f64>,
    latest: f64,
    recent: VecDeque<GrayImage>,
}

pub struct Detector {
    settings: Settings,
    cameras: Vec<Camera>,
    settled: bool,
    reference: Option<Vec<GrayImage>>,
    empty: Option<Vec<GrayImage>>,
    /// The settled scene is the empty board.
    scene_empty: bool,
    /// Pixels changed per camera by arrivals since the board was last empty.
    visit_pixels: Vec<usize>,
    keep_detail: bool,
}

impl Detector {
    /// One entry per camera slot; a camera without a mapper still contributes
    /// to stillness and occupancy but not to impact estimation.
    pub fn new(cameras: Vec<(u8, Option<Box<dyn LineMapper>>)>, settings: Settings) -> Self {
        Self {
            settings,
            cameras: cameras
                .into_iter()
                .map(|(slot, mapper)| Camera {
                    slot,
                    mapper,
                    previous_small: None,
                    still_since: None,
                    latest: f64::NEG_INFINITY,
                    recent: VecDeque::new(),
                })
                .collect(),
            settled: false,
            reference: None,
            empty: None,
            scene_empty: false,
            visit_pixels: Vec::new(),
            keep_detail: false,
        }
    }

    /// Keep masks and references in each change, for overlays.
    pub fn keep_detail(mut self) -> Self {
        self.keep_detail = true;
        self
    }

    /// No scene is settled: some camera still sees movement, or the cameras
    /// have not yet been still long enough.
    pub fn in_motion(&self) -> bool {
        !self.settled
    }

    /// Take the next still scene as the empty board.
    pub fn reset_empty(&mut self) {
        self.reference = None;
        self.empty = None;
        self.scene_empty = false;
        self.visit_pixels.clear();
        self.settled = false;
    }

    /// Replaces the settled references with each camera's latest still frames,
    /// so a change is measured against the scene just before it and slow
    /// lighting drift during long still periods does not count as change.
    fn refresh_references(&mut self) {
        let Some(reference) = self.reference.as_mut() else {
            return;
        };
        for (camera, image) in self.cameras.iter().zip(reference.iter_mut()) {
            if let Some(mean) = GrayImage::mean(&camera.recent.iter().collect::<Vec<_>>())
                && mean.width == image.width
                && mean.height == image.height
            {
                *image = mean;
            }
        }
        if self.scene_empty {
            self.empty = Some(reference.clone());
        }
    }

    pub fn push(&mut self, slot: u8, timestamp: f64, luma: GrayImage) -> Option<Event> {
        let settings = self.settings;
        let camera = self.cameras.iter_mut().find(|c| c.slot == slot)?;
        if timestamp <= camera.latest {
            return None;
        }
        camera.latest = timestamp;
        let small = luma.downsample(settings.motion_factor);
        let still = camera.previous_small.as_ref().is_some_and(|previous| {
            previous.width == small.width
                && previous.height == small.height
                && (previous
                    .changed(&small, settings.motion_threshold)
                    .iter()
                    .filter(|c| **c)
                    .count() as f64)
                    < settings.still_fraction * small.data.len() as f64
        });
        camera.previous_small = Some(small);
        if still {
            camera.still_since.get_or_insert(timestamp);
            camera.recent.push_back(luma);
            if camera.recent.len() > settings.reference_frames {
                camera.recent.pop_front();
            }
        } else {
            // The first motion after a settled scene: remember that scene as it
            // looked just now, before this camera's still frames are discarded.
            if self.settled {
                self.refresh_references();
                self.settled = false;
            }
            let camera = self.cameras.iter_mut().find(|c| c.slot == slot)?;
            camera.still_since = None;
            camera.recent.clear();
            return None;
        }
        if self.settled
            || !self.cameras.iter().all(|c| {
                c.still_since
                    .is_some_and(|since| c.latest - since >= settings.min_still_seconds)
            })
        {
            return None;
        }
        self.settled = true;
        let reference: Vec<GrayImage> = self
            .cameras
            .iter()
            .map(|c| GrayImage::mean(&c.recent.iter().collect::<Vec<_>>()))
            .collect::<Option<_>>()?;
        let timestamp = self
            .cameras
            .iter()
            .map(|c| c.latest)
            .fold(f64::NEG_INFINITY, f64::max);
        let Some(before) = self.reference.replace(reference.clone()) else {
            self.empty = Some(reference);
            self.scene_empty = true;
            self.visit_pixels = vec![0; self.cameras.len()];
            return Some(Event::Baseline { timestamp });
        };
        let empty = self.empty.as_ref()?;
        if before
            .iter()
            .zip(&reference)
            .any(|(a, b)| a.width != b.width || a.height != b.height)
        {
            self.empty = Some(reference);
            self.scene_empty = true;
            self.visit_pixels = vec![0; self.cameras.len()];
            return Some(Event::Disturbance { timestamp });
        }
        let mut changes: Vec<CameraChange> = self
            .cameras
            .iter()
            .zip(before.iter().zip(&reference).zip(empty))
            .map(|(camera, ((before, after), empty))| {
                change(camera, before, after, empty, &settings, self.keep_detail)
            })
            .collect();
        if changes
            .iter()
            .all(|c| c.changed_pixels < settings.min_region)
        {
            return None;
        }
        if changes.iter().zip(&reference).any(|(c, r)| {
            c.changed_pixels as f64 > settings.disturbance_fraction * r.data.len() as f64
        }) {
            return Some(Event::Disturbance { timestamp });
        }
        let arrivals = changes.iter().filter(|c| c.arrival_score > 0.0).count();
        if arrivals * 2 > changes.len() {
            let impact = fuse_changes(&mut changes, &settings);
            self.scene_empty = false;
            for (total, change) in self.visit_pixels.iter_mut().zip(&changes) {
                *total += change.changed_pixels;
            }
            return Some(Event::Arrival {
                timestamp,
                changes,
                impact,
            });
        }
        let remaining: Vec<usize> = reference
            .iter()
            .zip(empty)
            .map(|(r, e)| {
                r.changed(e, settings.change_threshold)
                    .iter()
                    .filter(|c| **c)
                    .count()
            })
            .collect();
        let added = self.visit_pixels.clone();
        // A majority decides: a camera that saw the visit's darts only small is
        // easily swayed by a shadow, while a dart left behind shows in most views.
        let empty_views = remaining
            .iter()
            .zip(&added)
            .filter(|(left, added)| **left as f64 <= settings.empty_ratio * **added as f64)
            .count();
        let now_empty = empty_views * 2 > remaining.len();
        if now_empty {
            // Restart from this board: earlier holes and lighting drift no
            // longer count against the next visit.
            self.empty = Some(reference);
            self.visit_pixels.iter_mut().for_each(|p| *p = 0);
        }
        self.scene_empty = now_empty;
        Some(Event::Removal {
            timestamp,
            changes,
            empty: now_empty,
            remaining,
            added,
        })
    }
}

fn change(
    camera: &Camera,
    before: &GrayImage,
    after: &GrayImage,
    empty: &GrayImage,
    settings: &Settings,
    keep_detail: bool,
) -> CameraChange {
    let s = settings;
    let (width, height) = (after.width, after.height);
    let threshold = s.change_threshold;
    let mask = before.changed(after, threshold);
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
    // Darts already on the board shift a little when a new one lands, and their
    // printed flights then change everywhere. Only pixels that now differ from
    // the empty board but did not in the reference scene belong to the new dart.
    let fresh_against = |reference: &GrayImage| -> Vec<bool> {
        (0..mask.len())
            .map(|i| {
                reference.data[i].abs_diff(after.data[i]) > threshold
                    && after.data[i].abs_diff(empty.data[i]) > threshold
                    && reference.data[i].abs_diff(empty.data[i]) <= threshold
            })
            .collect()
    };
    // Measure against the old scene with its darts moved to where they are
    // now, so their shift is not mistaken for the new dart.
    let mut moves = Vec::new();
    let mut raw_fresh = None;
    if s.compensate_moves && arrival_score > 0.0 {
        let (corrected, found) = compensate(
            before,
            after,
            empty,
            threshold,
            CompensateOptions {
                search_px: s.move_search_px,
                max_deg: s.move_max_deg,
                ..CompensateOptions::default()
            },
        );
        if !found.is_empty() {
            raw_fresh = Some(fresh_against(&corrected));
            moves = found;
        }
    }
    let raw_fresh = raw_fresh.unwrap_or_else(|| fresh_against(before));
    // A nudged dart only uncovers or covers a sliver along its edges, while the
    // new dart is a solid silhouette: keep the second.
    let fresh = remove_thin(
        &raw_fresh,
        width,
        height,
        s.fresh_open_radius,
        s.fresh_reconnect_px,
    );
    let fresh_pixels = fresh.iter().filter(|m| **m).count();
    // Group nearby pieces of one dart (flight, shaft, barrel) that small gaps
    // in the mask split apart, but fit only changed pixels.
    let grouped = dilate(&fresh, width, height, s.group_radius);
    let (labels, regions) = label(&grouped, width);
    let mut by_label: Vec<Vec<(f64, f64)>> = vec![Vec::new(); regions.len() + 1];
    let mut fresh_points: Vec<(u32, (f64, f64))> = Vec::with_capacity(fresh_pixels);
    for (i, _) in fresh.iter().enumerate().filter(|(_, f)| **f) {
        let point = ((i % width) as f64, (i / width) as f64);
        by_label[labels[i] as usize].push(point);
        fresh_points.push((labels[i], point));
    }
    let mut out = CameraChange {
        slot: camera.slot,
        changed_pixels,
        fresh_pixels,
        arrival_score,
        region: None,
        shaft: None,
        line: None,
        image_line: None,
        tip: None,
        method: None,
        alternatives: Vec::new(),
        moves,
        detail: keep_detail.then(|| (fresh.clone(), after.clone())),
    };
    let mut chosen = regions
        .iter()
        .map(|r| (r.label, by_label[r.label as usize].len()))
        .filter(|(_, count)| *count >= s.min_region)
        .max_by_key(|(_, count)| *count)
        .map(|(label, _)| label);
    // The line covering the most length over all fresh pixels picks the dart's
    // pieces: a short fat flight seen edge-on would win by size alone and leave
    // the shaft and barrel out.
    let mut joined = None;
    let mut select_line = None;
    if s.select_by_line && fresh_pixels > 0 {
        let points: Vec<(f64, f64)> = fresh_points.iter().map(|(_, p)| *p).collect();
        let options = RansacOptions {
            by_length: true,
            min_inliers: s.min_region,
            ..RansacOptions::default()
        };
        if let Some(line) = ransac_lines(&points, 1, options).into_iter().next() {
            let (c, d) = (line.centre, line.direction);
            select_line = Some((c, d));
            let mut hits = vec![0usize; regions.len() + 1];
            for (label, p) in &fresh_points {
                if ((p.0 - c.0) * d.1 - (p.1 - c.1) * d.0).abs() <= 4.0 {
                    hits[*label as usize] += 1;
                }
            }
            let keep: Vec<u32> = (1..hits.len())
                .filter(|&g| hits[g] >= s.line_select_min_px)
                .map(|g| g as u32)
                .collect();
            let total: usize = keep.iter().map(|&g| by_label[g as usize].len()).sum();
            if !keep.is_empty() && total >= s.min_region {
                // The first group with the most pixels on the line leads.
                let main = keep.iter().copied().fold(keep[0], |best, g| {
                    if hits[g as usize] > hits[best as usize] {
                        g
                    } else {
                        best
                    }
                });
                chosen = Some(main);
                joined = Some(keep.into_iter().filter(|&g| g != main).collect::<Vec<_>>());
            }
        }
    }
    let Some(main) = chosen else {
        return out;
    };
    let joined = joined.unwrap_or_else(|| {
        if !s.join_aligned {
            return Vec::new();
        }
        let groups: Vec<(u32, Vec<(f64, f64)>)> = regions
            .iter()
            .filter(|r| r.label != main)
            .map(|r| (r.label, by_label[r.label as usize].clone()))
            .collect();
        join_aligned(
            &groups,
            &by_label[main as usize],
            s.join_max_gap_px,
            s.join_max_offset_px,
            25.0,
        )
    });
    let in_region = |label: u32| label == main || joined.contains(&label);
    let points: Vec<(f64, f64)> = fresh_points
        .iter()
        .filter(|(label, _)| in_region(*label))
        .map(|(_, p)| *p)
        .collect();
    let axis = principal_axis(points.iter().copied());
    out.region = axis.map(|axis| (points.len(), axis));
    // The line comes from the barrel and shaft centre line, which the flight
    // cannot pull; when too little shaft is visible, the principal axis is used
    // with a larger uncertainty so precise views dominate the fusion. Slices are
    // cut across the selection line when there is one: the pixels' own axis is
    // pulled by a flight, which tilts the slices.
    let elongated = |axis: &Axis| axis.spread.0 >= s.min_elongation * axis.spread.1.max(0.5);
    let long_axis = axis.filter(elongated);
    let orient = match (select_line, long_axis) {
        (Some((centroid, direction)), Some(axis)) => Some(Axis {
            centroid,
            direction,
            spread: axis.spread,
        }),
        _ => long_axis,
    };
    let mut shaft =
        orient.and_then(|axis| shaft_axis(&points, &axis, s.max_width_ratio, s.min_slices));
    // A flight in view (face-on, or overlapping other darts) makes the whole
    // silhouette fat, but the line from the flight to the farthest changed pixel
    // still follows the dart. It beats the principal axis, which the flight pulls.
    let grouped_pixels: Vec<(usize, usize)> = labels
        .iter()
        .enumerate()
        .filter(|(_, l)| **l != 0 && in_region(**l))
        .map(|(i, _)| (i % width, i / width))
        .collect();
    let flight = flight_and_tip(&points, &grouped_pixels);
    let outside_flight = |points: &[(f64, f64)]| -> Vec<(f64, f64)> {
        match &flight {
            Some((flight, _)) => points
                .iter()
                .filter(|p| !flight.covers(p.0, p.1, 3))
                .copied()
                .collect(),
            None => points.to_vec(),
        }
    };
    if shaft.is_none() && flight.is_some() {
        // The flight's pixels outnumber the shaft's, so the region as a whole
        // is not elongated; the shaft is still there once the flight is set aside.
        let rest = outside_flight(&points);
        if let Some(rest_axis) = principal_axis(rest.iter().copied()).filter(elongated) {
            shaft = shaft_axis(&rest, &rest_axis, s.max_width_ratio, s.min_slices);
        }
    }
    let flight_line = flight.as_ref().map(|(flight, tip)| {
        let (dx, dy) = (tip.0 - flight.centre.0, tip.1 - flight.centre.1);
        let length = dx.hypot(dy).max(1.0);
        (*tip, (dx / length, dy / length), s.fallback_sigma_mm)
    });
    let fitted = match (&shaft, flight_line, long_axis) {
        (Some(shaft), _, _) => Some((
            shaft.point,
            shaft.direction,
            s.line_sigma_mm,
            LineMethod::Shaft,
        )),
        (None, Some((point, direction, sigma)), _) => {
            Some((point, direction, sigma, LineMethod::Flight))
        }
        (None, None, Some(axis)) => Some((
            axis.centroid,
            axis.direction,
            s.fallback_sigma_mm,
            LineMethod::PrincipalAxis,
        )),
        (None, None, None) => None,
    };
    let mapper = camera.mapper.as_ref();
    if let Some(((cx, cy), (dx, dy), sigma_mm, method)) = fitted {
        out.image_line = Some([
            (cx - dx * 150.0, cy - dy * 150.0),
            (cx + dx * 150.0, cy + dy * 150.0),
        ]);
        out.method = Some(method);
        out.line = mapper
            .and_then(|m| m.line_to_board((cx, cy), (cx + dx * 50.0, cy + dy * 50.0)))
            .map(|line| LineObservation {
                camera: camera.slot,
                line,
                sigma_mm,
            });
    }
    // Alternatives: sequential RANSAC on the shaft and barrel alone (wide slices,
    // the flight even edge-on, left out as the shaft fit does), so two parallel
    // strips in one region give two lines instead of one fitted between them.
    if s.line_candidates > 0
        && let Some(mapper) = mapper
    {
        let mut rest = outside_flight(&points);
        let orient = match select_line {
            Some((centroid, direction)) => Some(Axis {
                centroid,
                direction,
                spread: (1.0, 0.0),
            }),
            None => principal_axis(rest.iter().copied()),
        };
        if let Some(axis) = orient {
            rest = narrow_points(&rest, &axis, s.max_width_ratio);
        }
        let options = RansacOptions {
            by_length: true,
            ..RansacOptions::default()
        };
        for line in ransac_lines(&rest, s.line_candidates, options) {
            let ((cx, cy), (dx, dy)) = (line.centre, line.direction);
            if let Some(board) = mapper.line_to_board((cx, cy), (cx + dx * 50.0, cy + dy * 50.0)) {
                out.alternatives.push((
                    [
                        (cx - dx * 150.0, cy - dy * 150.0),
                        (cx + dx * 150.0, cy + dy * 150.0),
                    ],
                    LineObservation {
                        camera: camera.slot,
                        line: board,
                        sigma_mm: s.line_sigma_mm,
                    },
                ));
            }
        }
    }
    out.tip = flight.map(|(_, tip)| tip);
    out.shaft = shaft;
    out
}

/// Fuses the cameras' lines. When the main lines do not all agree, or a
/// camera's shaft fit is poor (`Settings::poor_fit_rms_px`), every
/// combination of main and alternative lines is tried; a combination is only
/// taken when at least three cameras then agree (two lines always cross, so
/// they prove nothing), the one with the smallest residuals winning. The
/// cameras whose alternative was used get it as their line.
pub fn fuse_changes(
    changes: &mut [CameraChange],
    settings: &Settings,
) -> Result<Impact, Abstention> {
    let limits = settings.limits;
    let lines: Vec<LineObservation> = changes.iter().filter_map(|c| c.line).collect();
    let impact = fuse_lines(&lines, limits);
    let agrees = |i: &Impact| i.consistency_checked && i.rejected_cameras.is_empty();
    let poor_fit = changes.iter().any(|c| {
        c.method == Some(LineMethod::Shaft)
            && c.shaft
                .as_ref()
                .is_some_and(|s| s.rms_px > settings.poor_fit_rms_px)
    });
    if impact.as_ref().is_ok_and(agrees) && !poor_fit {
        return impact;
    }
    type Choice = (Option<usize>, Option<LineObservation>);
    let options: Vec<Vec<Choice>> = changes
        .iter()
        .map(|c| {
            let mut choices: Vec<Choice> = c.line.map(|l| (None, Some(l))).into_iter().collect();
            choices.extend(
                c.alternatives
                    .iter()
                    .enumerate()
                    .map(|(i, (_, obs))| (Some(i), Some(*obs))),
            );
            if choices.is_empty() {
                choices.push((None, None));
            }
            choices
        })
        .collect();
    if options.iter().filter(|o| o[0].1.is_some()).count() < 3
        || options.iter().all(|o| o.len() == 1)
    {
        return impact;
    }
    let mut best: Option<(f64, Impact, Vec<usize>)> = None;
    let mut pick = vec![0usize; options.len()];
    'combinations: loop {
        let used: Vec<LineObservation> = pick
            .iter()
            .zip(&options)
            .filter_map(|(&k, o)| o[k].1)
            .collect();
        if let Ok(result) = fuse_lines(&used, limits)
            && agrees(&result)
        {
            let chi2: f64 = used
                .iter()
                .map(|o| (o.line.distance(result.point) / o.sigma_mm).powi(2))
                .sum();
            if best.as_ref().is_none_or(|(b, _, _)| chi2 < *b) {
                best = Some((chi2, result, pick.clone()));
            }
        }
        // Next combination, the last camera varying fastest.
        let mut k = options.len();
        loop {
            if k == 0 {
                break 'combinations;
            }
            k -= 1;
            pick[k] += 1;
            if pick[k] < options[k].len() {
                break;
            }
            pick[k] = 0;
        }
    }
    let Some((_, result, pick)) = best else {
        return impact;
    };
    for (change, (&k, choices)) in changes.iter_mut().zip(pick.iter().zip(&options)) {
        if let Some(i) = choices[k].0 {
            let (image_line, observation) = change.alternatives[i];
            change.line = Some(observation);
            change.image_line = Some(image_line);
            change.method = Some(LineMethod::Ransac);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oche_domain::board::BoardPoint;

    /// Image pixel (x, y) is board point (x - 100, 100 - y) mm.
    struct Offset;

    impl LineMapper for Offset {
        fn line_to_board(&self, a: (f64, f64), b: (f64, f64)) -> Option<BoardLine> {
            BoardLine::through(
                BoardPoint::new(a.0 - 100.0, 100.0 - a.1),
                BoardPoint::new(b.0 - 100.0, 100.0 - b.1),
            )
        }
    }

    type Segment = ((f64, f64), (f64, f64));

    fn frame(darts: &[Segment]) -> GrayImage {
        let (width, height) = (200, 200);
        let mut data = vec![120u8; width * height];
        for &((x0, y0), (x1, y1)) in darts {
            for step in 0..=400 {
                let t = f64::from(step) / 400.0;
                let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        let (px, py) = (
                            (x.round() as i64 + dx) as usize,
                            (y.round() as i64 + dy) as usize,
                        );
                        data[py * width + px] = 20;
                    }
                }
            }
        }
        GrayImage {
            width,
            height,
            data,
        }
    }

    fn feed(detector: &mut Detector, t: &mut f64, image: &GrayImage, seconds: f64) -> Vec<Event> {
        let mut events = Vec::new();
        let end = *t + seconds;
        while *t < end {
            for slot in [1, 2] {
                if let Some(event) = detector.push(slot, *t, image.clone()) {
                    events.push(event);
                }
            }
            *t += 0.1;
        }
        events
    }

    /// A synthetic dart: a thin barrel from `tip` towards `flight`, ending in a
    /// solid square flight, as a flight seen face-on looks.
    fn flighted_frame(tip: (f64, f64), flight: (f64, f64)) -> GrayImage {
        let mut image = frame(&[(tip, flight)]);
        for y in (flight.1 as usize - 18)..(flight.1 as usize + 18) {
            for x in (flight.0 as usize - 18)..(flight.0 as usize + 18) {
                image.data[y * image.width + x] = 20;
            }
        }
        image
    }

    /// A small striped patch like a printed flight, `shift` px to the right.
    fn striped_flight(image: &mut GrayImage, left: usize, top: usize, shift: usize) {
        for y in top..top + 18 {
            for x in left..left + 18 {
                let stripe = ((x + shift) / 2).is_multiple_of(2);
                image.data[y * image.width + x] = if stripe { 20 } else { 235 };
            }
        }
    }

    #[test]
    fn a_neighbouring_dart_whose_flight_shifted_does_not_skew_the_new_dart() {
        let mut detector = Detector::new(
            vec![(1, Some(Box::new(Offset))), (2, Some(Box::new(Offset)))],
            Settings::default(),
        );
        let mut t = 0.0;
        let empty = frame(&[]);
        let _ = feed(&mut detector, &mut t, &empty, 2.0);
        // First dart: a striped flight on camera 1 where the next dart's tip will
        // land, and a plain dart far away on camera 2.
        let mut first_1 = empty.clone();
        striped_flight(&mut first_1, 88, 103, 0);
        let first_2 = frame(&[((30.0, 160.0), (60.0, 190.0))]);
        for _ in 0..20 {
            let _ = detector.push(1, t, first_1.clone());
            let _ = detector.push(2, t, first_2.clone());
            t += 0.1;
        }
        // Second dart: the first one's flight shifts one pixel on camera 1.
        let mut second_1 = flighted_frame((100.0, 100.0), (150.0, 40.0));
        striped_flight(&mut second_1, 88, 103, 1);
        let mut second_2 = flighted_frame((100.0, 100.0), (40.0, 30.0));
        for (x, y) in [(30, 160), (45, 175), (60, 190)] {
            second_2.data[y * 200 + x] = 20;
        }
        let mut events = Vec::new();
        let end = t + 2.0;
        while t < end {
            events.extend(detector.push(1, t, second_1.clone()));
            events.extend(detector.push(2, t, second_2.clone()));
            t += 0.1;
        }
        let [
            Event::Arrival {
                impact, changes, ..
            },
        ] = events.as_slice()
        else {
            panic!("expected one arrival, got {}", events.len());
        };
        // The tip is the new dart's, not the neighbour's far corner at (88, 121).
        let tip = changes[0].tip.expect("a flight is in view");
        assert!(
            (tip.0 - 100.0).abs() < 3.0 && (tip.1 - 100.0).abs() < 3.0,
            "{tip:?}"
        );
        let point = impact.as_ref().expect("two lines through the bull").point;
        assert!(point.x.abs() < 1.5 && point.y.abs() < 1.5, "{point:?}");
    }

    #[test]
    fn a_face_on_flight_still_gives_a_dart_line_through_the_tip() {
        let mut detector = Detector::new(
            vec![(1, Some(Box::new(Offset))), (2, Some(Box::new(Offset)))],
            Settings::default(),
        );
        let mut t = 0.0;
        let empty = frame(&[]);
        let _ = feed(&mut detector, &mut t, &empty, 2.0);
        // Both darts end in the bull at (100, 100); each has a 36 px flight.
        let dart_1 = flighted_frame((100.0, 100.0), (160.0, 40.0));
        let dart_2 = flighted_frame((100.0, 100.0), (40.0, 30.0));
        let mut events = Vec::new();
        let end = t + 2.0;
        while t < end {
            events.extend(detector.push(1, t, dart_1.clone()));
            events.extend(detector.push(2, t, dart_2.clone()));
            t += 0.1;
        }
        let [
            Event::Arrival {
                impact, changes, ..
            },
        ] = events.as_slice()
        else {
            panic!("expected one arrival");
        };
        assert!(changes.iter().all(|c| c.line.is_some() && c.tip.is_some()));
        let point = impact.as_ref().expect("two lines through the bull").point;
        assert!(point.x.abs() < 3.0 && point.y.abs() < 3.0, "{point:?}");
    }

    #[test]
    fn baseline_arrival_and_takeout_follow_the_scene() {
        let mut detector = Detector::new(
            vec![(1, Some(Box::new(Offset))), (2, Some(Box::new(Offset)))],
            Settings::default(),
        );
        let mut t = 0.0;
        let empty = frame(&[]);
        let events = feed(&mut detector, &mut t, &empty, 2.0);
        assert!(matches!(events.as_slice(), [Event::Baseline { .. }]));
        assert!(
            feed(&mut detector, &mut t, &empty, 2.0).is_empty(),
            "no change, no event"
        );

        // Both cameras see the same synthetic dart through (100, 100) = bull,
        // but along different directions, so their lines cross at the bull.
        let mut detector2 = Detector::new(
            vec![(1, Some(Box::new(Offset))), (2, Some(Box::new(Offset)))],
            Settings::default(),
        );
        let mut t2 = 0.0;
        let _ = feed(&mut detector2, &mut t2, &empty, 2.0);
        let dart_1 = frame(&[((100.0, 100.0), (160.0, 40.0))]);
        let dart_2 = frame(&[((100.0, 100.0), (40.0, 30.0))]);
        let mut events = Vec::new();
        let end = t2 + 2.0;
        while t2 < end {
            events.extend(detector2.push(1, t2, dart_1.clone()));
            events.extend(detector2.push(2, t2, dart_2.clone()));
            t2 += 0.1;
        }
        let [Event::Arrival { impact, .. }] = events.as_slice() else {
            panic!("expected one arrival");
        };
        let point = impact.as_ref().expect("two crossing lines").point;
        assert!(point.radius() < 2.0, "{point:?}");

        let mut events = Vec::new();
        let end = t2 + 2.0;
        while t2 < end {
            events.extend(detector2.push(1, t2, empty.clone()));
            events.extend(detector2.push(2, t2, empty.clone()));
            t2 += 0.1;
        }
        assert!(matches!(
            events.as_slice(),
            [Event::Removal { empty: true, .. }]
        ));
    }

    #[test]
    fn motion_without_a_lasting_change_produces_nothing() {
        let mut detector = Detector::new(vec![(1, None)], Settings::default());
        let mut t = 0.0;
        let empty = frame(&[]);
        assert_eq!(feed(&mut detector, &mut t, &empty, 2.0).len(), 1);
        // A hand passes for a few frames, then the scene is unchanged.
        let hand = GrayImage {
            width: 200,
            height: 200,
            data: vec![60; 200 * 200],
        };
        let _ = feed(&mut detector, &mut t, &hand, 0.3);
        assert!(feed(&mut detector, &mut t, &empty, 2.0).is_empty());
    }

    #[test]
    fn a_large_lasting_change_is_a_disturbance() {
        let mut detector = Detector::new(vec![(1, None)], Settings::default());
        let mut t = 0.0;
        let _ = feed(&mut detector, &mut t, &frame(&[]), 2.0);
        let darker = GrayImage {
            width: 200,
            height: 200,
            data: vec![60; 200 * 200],
        };
        let events = feed(&mut detector, &mut t, &darker, 2.0);
        assert!(matches!(events.as_slice(), [Event::Disturbance { .. }]));
    }

    fn through(camera: u8, p: (f64, f64), q: (f64, f64)) -> LineObservation {
        LineObservation {
            camera,
            line: BoardLine::through(BoardPoint::new(p.0, p.1), BoardPoint::new(q.0, q.1)).unwrap(),
            sigma_mm: 2.0,
        }
    }

    fn observed(line: LineObservation, alternatives: &[LineObservation]) -> CameraChange {
        CameraChange {
            slot: line.camera,
            changed_pixels: 1000,
            fresh_pixels: 1000,
            arrival_score: 10.0,
            region: None,
            shaft: None,
            line: Some(line),
            image_line: None,
            tip: None,
            method: Some(LineMethod::Shaft),
            alternatives: alternatives
                .iter()
                .map(|a| ([(0.0, 0.0), (1.0, 1.0)], *a))
                .collect(),
            moves: Vec::new(),
            detail: None,
        }
    }

    #[test]
    fn an_alternative_line_is_used_when_it_makes_three_cameras_agree() {
        let hit = (10.0, 20.0);
        let wrong = through(3, (40.0, 20.0), (140.0, 120.0)); // misses by ~21 mm
        let right = through(3, hit, (110.0, 120.0));
        let mut changes = vec![
            observed(through(1, hit, (110.0, 20.0)), &[]),
            observed(through(2, hit, (10.0, 120.0)), &[]),
            observed(wrong, &[right]),
        ];
        let impact =
            fuse_changes(&mut changes, &Settings::default()).expect("three agreeing lines");
        assert!(impact.consistency_checked && impact.rejected_cameras.is_empty());
        assert!((impact.point.x - hit.0).abs() < 0.5 && (impact.point.y - hit.1).abs() < 0.5);
        assert_eq!(changes[2].method, Some(LineMethod::Ransac));
        assert_eq!(changes[2].line, Some(right));
    }

    #[test]
    fn two_cameras_keep_their_main_lines() {
        let hit = (10.0, 20.0);
        let other = through(2, (50.0, 20.0), (50.0, 120.0));
        let mut changes = vec![
            observed(through(1, hit, (110.0, 20.0)), &[]),
            observed(through(2, hit, (10.0, 120.0)), &[other]),
        ];
        let impact = fuse_changes(&mut changes, &Settings::default()).unwrap();
        assert!((impact.point.x - hit.0).abs() < 0.5);
        assert_eq!(changes[1].method, Some(LineMethod::Shaft));
    }
}
