//! Undo the small moves of darts already on the board.
//!
//! A new dart often nudges the darts already there. Comparing the new scene
//! with the old one then shows the old darts' displaced edges as if they were
//! new, and hides the new dart wherever it crosses in front of an old one. The
//! old darts are the pixels of `before` that differ from the empty board. Each
//! connected group of them (a dart, or touching darts) is moved as one rigid
//! body, by the small rotation and translation that best explains `after`, and
//! `before` is rebuilt with it at its new place. Comparing `after` with that
//! corrected scene leaves only the new dart.
//!
//! The model is layered: in the corrected scene a moved dart's pixels are
//! first erased (replaced by the empty board, which is what they uncovered)
//! and then pasted at their new position. A rigid motion has to explain the
//! whole dart, so it cannot slide part of an old dart onto a new dart lying
//! next to it.

use crate::mask::{GrayImage, dilate, label};

/// How one old dart (or group of touching darts) moved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DartMove {
    /// Bounding box `(x0, y0, x1, y1)` of the dart in `before`.
    pub bounds: (usize, usize, usize, usize),
    /// Rotation about `centre`, in degrees (image coordinates, y down).
    pub angle_deg: f64,
    pub dx: i64,
    pub dy: i64,
    pub centre: (f64, f64),
}

/// Options of [`compensate`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompensateOptions {
    /// Largest translation searched, in pixels.
    pub search_px: i64,
    /// Largest rotation searched, in degrees, and its step.
    pub max_deg: f64,
    pub step_deg: f64,
    /// Dilation radius grouping old-dart pixels into darts.
    pub group_radius: usize,
    /// Old darts smaller than this are left in place.
    pub min_pixels: usize,
    /// Cost per pixel per pixel of displacement of the dart's farthest pixel,
    /// in grey levels: prefers the smallest motion among near ties.
    pub penalty: f64,
}

impl Default for CompensateOptions {
    fn default() -> Self {
        Self {
            search_px: 5,
            max_deg: 3.0,
            step_deg: 0.5,
            group_radius: 2,
            min_pixels: 40,
            penalty: 0.5,
        }
    }
}

/// `(y, x)` of `(x, y)` rotated by `angle` about `centre` and shifted, rounded
/// half to even.
fn moved(
    x: f64,
    y: f64,
    centre: (f64, f64),
    (cos, sin): (f64, f64),
    dx: f64,
    dy: f64,
) -> (i64, i64) {
    let (rx, ry) = (x - centre.0, y - centre.1);
    (
        (centre.1 + sin * rx + cos * ry + dy).round_ties_even() as i64,
        (centre.0 + cos * rx - sin * ry + dx).round_ties_even() as i64,
    )
}

/// `before` with its old darts moved to match `after`, and how each moved.
///
/// Every old dart near a change tries each rotation and translation within
/// the options and keeps the motion with the lowest truncated absolute
/// difference to `after`. Truncation makes pixels a new dart now covers cost
/// the same for every motion, so they cannot drag the estimate.
pub fn compensate(
    before: &GrayImage,
    after: &GrayImage,
    empty: &GrayImage,
    threshold: u8,
    options: CompensateOptions,
) -> (GrayImage, Vec<DartMove>) {
    let o = options;
    let (w, h) = (before.width, before.height);
    let occupied = before.changed(empty, threshold);
    let changed = before.changed(after, threshold);
    let near_change = dilate(&changed, w, h, o.search_px as usize + 2);
    let cap = 2 * i32::from(threshold);
    let (labels, regions) = label(&dilate(&occupied, w, h, o.group_radius), w);
    let steps = (2.0 * o.max_deg / o.step_deg).round() as i64;
    let angles: Vec<f64> = (0..=steps)
        .map(|k| (-o.max_deg + k as f64 * o.step_deg).to_radians())
        .collect();
    let level =
        |image: &GrayImage, x: i64, y: i64| i32::from(image.data[y as usize * w + x as usize]);
    let mut found = Vec::new();
    for region in &regions {
        let own: Vec<(usize, usize)> = (region.min.1..=region.max.1)
            .flat_map(|y| (region.min.0..=region.max.0).map(move |x| (x, y)))
            .filter(|&(x, y)| occupied[y * w + x] && labels[y * w + x] == region.label)
            .collect();
        if own.len() < o.min_pixels || !own.iter().any(|&(x, y)| near_change[y * w + x]) {
            continue;
        }
        let n = own.len() as f64;
        let centre = (
            own.iter().map(|p| p.0 as f64).sum::<f64>() / n,
            own.iter().map(|p| p.1 as f64).sum::<f64>() / n,
        );
        let radius = own
            .iter()
            .map(|p| (p.0 as f64 - centre.0).hypot(p.1 as f64 - centre.1))
            .fold(0.0, f64::max);
        let is_own = |x: i64, y: i64| {
            occupied[y as usize * w + x as usize]
                && labels[y as usize * w + x as usize] == region.label
        };
        let mut best = (0.0, 0i64, 0i64);
        let mut best_cost: Option<f64> = None;
        for &angle in &angles {
            let rotation = (angle.cos(), angle.sin());
            let rotated: Vec<(i64, i64)> = own
                .iter()
                .map(|&(x, y)| moved(x as f64, y as f64, centre, rotation, 0.0, 0.0))
                .collect();
            for dy in -o.search_px..=o.search_px {
                for dx in -o.search_px..=o.search_px {
                    let motion = (dx as f64).hypot(dy as f64) + angle.abs() * radius;
                    let mut cost = 0i64;
                    for (&(x, y), &(ry, rx)) in own.iter().zip(&rotated) {
                        let (ty, tx) = (ry + dy, rx + dx);
                        if ty < 0 || tx < 0 || ty >= h as i64 || tx >= w as i64 {
                            continue;
                        }
                        let value = i32::from(before.data[y * w + x]);
                        // The scene at the destination without the dart pasted
                        // there: the empty board where the dart itself was erased.
                        let base = if is_own(tx, ty) {
                            level(empty, tx, ty)
                        } else {
                            level(before, tx, ty)
                        };
                        let target = level(after, tx, ty);
                        // Relative to the scene with the dart erased, pasting
                        // changes the cost only at the destination pixels.
                        cost += i64::from(
                            (target - value).abs().min(cap) - (target - base).abs().min(cap),
                        );
                    }
                    let cost = cost as f64 + o.penalty * n * motion;
                    if best_cost.is_none_or(|b| cost < b) {
                        best = (angle, dx, dy);
                        best_cost = Some(cost);
                    }
                }
            }
        }
        if best != (0.0, 0, 0) {
            found.push((own, centre, best, region));
        }
    }
    let mut corrected = before.clone();
    // Erase every moved dart first, then paste, so a dart moving onto
    // another's old place is not overwritten by that one's erasure.
    for (own, ..) in &found {
        for &(x, y) in own {
            corrected.data[y * w + x] = empty.data[y * w + x];
        }
    }
    let mut moves = Vec::new();
    for (own, centre, (angle, dx, dy), region) in &found {
        let rotation = (angle.cos(), angle.sin());
        for &(x, y) in own {
            let (ty, tx) = moved(
                x as f64, y as f64, *centre, rotation, *dx as f64, *dy as f64,
            );
            if ty >= 0 && tx >= 0 && ty < h as i64 && tx < w as i64 {
                corrected.data[ty as usize * w + tx as usize] = before.data[y * w + x];
            }
        }
        moves.push(DartMove {
            bounds: (region.min.0, region.min.1, region.max.0, region.max.1),
            angle_deg: angle.to_degrees(),
            dx: *dx,
            dy: *dy,
            centre: *centre,
        });
    }
    (corrected, moves)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic board texture so a wrong shift of the background would cost.
    fn scene() -> GrayImage {
        let (width, height) = (200, 160);
        let mut data = Vec::with_capacity(width * height);
        let mut noise = 12345u32;
        for _ in 0..height {
            for x in 0..width {
                noise = noise.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let stripe = if (x as f64 / 7.0).sin() >= 0.0 {
                    40
                } else {
                    -40
                };
                data.push((120 + stripe + (noise >> 16) as i32 % 7 - 3) as u8);
            }
        }
        GrayImage {
            width,
            height,
            data,
        }
    }

    fn put_dart(image: &GrayImage, x: usize, y0: usize, y1: usize, value: u8) -> GrayImage {
        let mut out = image.clone();
        for y in y0..y1 {
            for xx in x..x + 6 {
                out.data[y * out.width + xx] = value;
            }
        }
        out
    }

    fn residual(
        after: &GrayImage,
        corrected: &GrayImage,
        x0: usize,
        x1: usize,
        y0: usize,
        y1: usize,
    ) -> usize {
        (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                after.data[y * after.width + x].abs_diff(corrected.data[y * after.width + x]) > 30
            })
            .count()
    }

    #[test]
    fn a_moved_dart_is_put_where_after_shows_it() {
        let empty = scene();
        let before = put_dart(&empty, 60, 20, 120, 20);
        let after = put_dart(&put_dart(&empty, 63, 20, 120, 20), 140, 30, 130, 230);
        let (corrected, moves) =
            compensate(&before, &after, &empty, 30, CompensateOptions::default());
        assert_eq!(moves.len(), 1);
        assert_eq!((moves[0].dx, moves[0].dy, moves[0].angle_deg), (3, 0, 0.0));
        // Only the new dart is left.
        assert_eq!(residual(&after, &corrected, 0, 120, 0, 160), 0);
        assert_eq!(residual(&after, &corrected, 140, 146, 30, 130), 600);
    }

    #[test]
    fn an_old_dart_next_to_a_new_one_is_not_slid_onto_it() {
        let empty = scene();
        let before = put_dart(&empty, 60, 20, 120, 20);
        // The old dart stays; a new one lands touching it on its right.
        let after = put_dart(&put_dart(&empty, 60, 20, 120, 20), 66, 20, 120, 20);
        let (corrected, moves) =
            compensate(&before, &after, &empty, 30, CompensateOptions::default());
        assert!(moves.is_empty());
        assert_eq!(residual(&after, &corrected, 66, 72, 20, 120), 600);
    }
}
