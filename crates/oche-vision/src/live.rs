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

use crate::mask::{Axis, GrayImage, ShaftFit, dilate, label, principal_axis, shaft_axis};
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
    /// Positive when the new scene differs more from the empty board than the
    /// previous scene did, i.e. something arrived.
    pub arrival_score: f64,
    pub region: Option<(usize, Axis)>,
    pub shaft: Option<ShaftFit>,
    pub line: Option<LineObservation>,
    /// The change mask and new reference, kept only when requested.
    pub detail: Option<(Vec<bool>, GrayImage)>,
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
        let changes: Vec<CameraChange> = self
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
            let lines: Vec<LineObservation> = changes.iter().filter_map(|c| c.line).collect();
            let impact = fuse_lines(&lines, settings.limits);
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
    // Group nearby pieces of one dart (flight, shaft, barrel) that small gaps
    // in the mask split apart, but fit only changed pixels.
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
    let region = largest
        .and_then(|(label, count)| principal_axis(points_of(label)).map(|axis| (count, axis)));
    // The principal axis orients the slices. The line comes from the barrel and
    // shaft centre line, which the flight cannot pull; when too little shaft is
    // visible, the principal axis is used with a larger uncertainty so precise
    // views dominate the fusion.
    let elongated = region
        .filter(|(_, axis)| axis.spread.0 >= settings.min_elongation * axis.spread.1.max(0.5));
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
        let line = camera
            .mapper
            .as_ref()?
            .line_to_board((cx, cy), (cx + dx * 50.0, cy + dy * 50.0))?;
        Some(LineObservation {
            camera: camera.slot,
            line,
            sigma_mm,
        })
    });
    CameraChange {
        slot: camera.slot,
        changed_pixels,
        arrival_score,
        region,
        shaft,
        line,
        detail: keep_detail.then(|| (mask, after.clone())),
    }
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
}
