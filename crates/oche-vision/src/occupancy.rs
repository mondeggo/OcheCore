//! Whether the board is empty, judged from one still frame per camera.
//!
//! Through a camera's calibration every pixel looks at a board point, so the
//! pixels of each scoring cell are known: the two bulls, and every segment's
//! part of the treble, the double, and each single area split radially. An
//! empty board is nearly uniform within a cell, while a dart's barrel, shaft,
//! flight, or shadow departs from the cell's median. Pixels near a wire are
//! skipped: the wires, their shadows, and calibration error lie there. Each
//! pixel is compared with its own cell's median in the same frame, so exposure,
//! white balance, and slow lighting changes shift both alike. That is what lets
//! the check judge a scene no earlier reference describes, such as the first
//! one after the cameras open.
//!
//! The defaults were chosen on the 2026-10-29/30 recordings (see
//! `docs/discovery.md`): every empty still scene measured 0 mm² in every
//! camera, every scene with a dart measured at least 158 mm² in its best
//! camera, and calibration errors up to 2 mm or 1° added at most 11 mm². The
//! limit sits between them; it is not validated on other boards, darts, or
//! lighting.

use oche_domain::board::BoardDimensions;

use crate::live::LineMapper;
use crate::mask::{GrayImage, dilate, erode};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OccupancySettings {
    /// Luma difference from the cell median above which a pixel stands out.
    pub threshold: u8,
    /// Pixels nearer than this to a ring or radial wire (centre line) are
    /// skipped.
    pub wire_margin_mm: f64,
    /// Radial cells per single area (1 to 5): a long cell spans the light's
    /// fall-off, a short one is mostly covered by a dart lying in it.
    pub single_bins: usize,
    /// Outlying lines narrower than `2 * speck_radius + 1` px are ignored:
    /// sisal fibres, holes, and noise.
    pub speck_radius: usize,
    /// A camera sees something on the board when its outlying pixels cover
    /// more than this board area.
    pub occupied_mm2: f64,
}

impl Default for OccupancySettings {
    fn default() -> Self {
        Self {
            threshold: 40,
            wire_margin_mm: 2.0,
            single_bins: 3,
            speck_radius: 1,
            occupied_mm2: 50.0,
        }
    }
}

/// The scoring cells of one camera's frames, from its calibration.
pub struct Cells {
    width: usize,
    height: usize,
    /// Bounding box of the cell pixels with a margin for the speck filter.
    left: usize,
    top: usize,
    columns: usize,
    rows: usize,
    /// Frame index, cell, and the board area (mm²) each cell pixel covers.
    pixels: Vec<(u32, u8, f32)>,
    count: usize,
}

impl Cells {
    pub fn new(
        mapper: &dyn LineMapper,
        board: &BoardDimensions,
        width: usize,
        height: usize,
        settings: &OccupancySettings,
    ) -> Self {
        let bins = settings.single_bins.clamp(1, 5);
        let point = |x: usize, y: usize| mapper.pixel_to_board((x as f64, y as f64));
        let mut pixels = Vec::new();
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..height {
            for x in 0..width {
                let Some(p) = point(x, y) else {
                    continue;
                };
                let Some(cell) = cell(board, settings.wire_margin_mm, bins, p) else {
                    continue;
                };
                // The board area this pixel covers, from its neighbours.
                let (Some(right), Some(below)) = (
                    point(x + 1, y).or_else(|| point(x.wrapping_sub(1), y)),
                    point(x, y + 1).or_else(|| point(x, y.wrapping_sub(1))),
                ) else {
                    continue;
                };
                let (ax, ay) = (right.0 - p.0, right.1 - p.1);
                let (bx, by) = (below.0 - p.0, below.1 - p.1);
                let area = (ax * by - ay * bx).abs();
                if !area.is_finite() {
                    continue;
                }
                pixels.push(((y * width + x) as u32, cell, area as f32));
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
        let pad = settings.speck_radius + 1;
        let (left, top) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
        let (columns, rows) = if pixels.is_empty() {
            (0, 0)
        } else {
            (
                (x1 + pad + 1).min(width) - left,
                (y1 + pad + 1).min(height) - top,
            )
        };
        Self {
            width,
            height,
            left,
            top,
            columns,
            rows,
            pixels,
            count: 2 + 2 * bins * 20 + 40,
        }
    }

    /// Whether these cells were built for frames of this size.
    pub fn fits(&self, image: &GrayImage) -> bool {
        self.width == image.width && self.height == image.height
    }

    /// Board area (mm²) of the pixels that stand out from their cell, or
    /// `None` for a frame of another size or a camera that sees no cell.
    pub fn outlying_mm2(&self, image: &GrayImage, settings: &OccupancySettings) -> Option<f64> {
        if !self.fits(image) || self.pixels.is_empty() {
            return None;
        }
        let mut histograms = vec![[0u32; 256]; self.count];
        for &(i, cell, _) in &self.pixels {
            histograms[cell as usize][image.data[i as usize] as usize] += 1;
        }
        let medians: Vec<u8> = histograms
            .iter()
            .map(|histogram| {
                let half = histogram.iter().sum::<u32>().div_ceil(2);
                let mut seen = 0;
                histogram
                    .iter()
                    .position(|n| {
                        seen += n;
                        seen >= half.max(1)
                    })
                    .unwrap_or(0) as u8
            })
            .collect();
        let local = |i: u32| {
            let (x, y) = (i as usize % self.width, i as usize / self.width);
            (y - self.top) * self.columns + (x - self.left)
        };
        let mut outlying = vec![false; self.columns * self.rows];
        for &(i, cell, _) in &self.pixels {
            if image.data[i as usize].abs_diff(medians[cell as usize]) > settings.threshold {
                outlying[local(i)] = true;
            }
        }
        // An opening never adds pixels, so every survivor is a cell pixel.
        let radius = settings.speck_radius;
        let kept = dilate(
            &erode(&outlying, self.columns, self.rows, radius),
            self.columns,
            self.rows,
            radius,
        );
        Some(
            self.pixels
                .iter()
                .filter(|(i, _, _)| kept[local(*i)])
                // From +0: an empty float sum is -0, which prints as "-0".
                .fold(0.0, |total, (_, _, area)| total + f64::from(*area)),
        )
    }
}

/// The scoring cell of a board point (mm, +y toward the 20), or `None` near a
/// wire or off the scoring area.
fn cell(board: &BoardDimensions, margin: f64, bins: usize, (x, y): (f64, f64)) -> Option<u8> {
    let r = x.hypot(y);
    if board.radii().iter().any(|wire| (r - wire).abs() < margin) {
        return None;
    }
    if r < board.inner_bull {
        return Some(0);
    }
    if r < board.outer_bull {
        return Some(1);
    }
    if r > board.double_outer {
        return None;
    }
    // Clockwise from the top; radial wires lie 9° either side of a segment's
    // middle.
    let degrees = x.atan2(y).to_degrees().rem_euclid(360.0);
    let from_wire = (degrees - 9.0).rem_euclid(18.0);
    if r * from_wire.min(18.0 - from_wire).to_radians().sin() < margin {
        return None;
    }
    let segment = (((degrees + 9.0).rem_euclid(360.0) / 18.0) as usize).min(19);
    let (first, inner, outer, split) = if r < board.treble_inner {
        (2, board.outer_bull, board.treble_inner, bins)
    } else if r < board.treble_outer {
        (2 + bins * 20, board.treble_inner, board.treble_outer, 1)
    } else if r < board.double_inner {
        (
            2 + bins * 20 + 20,
            board.treble_outer,
            board.double_inner,
            bins,
        )
    } else {
        (
            2 + 2 * bins * 20 + 20,
            board.double_inner,
            board.double_outer,
            1,
        )
    };
    let bin = (((r - inner) / (outer - inner) * split as f64) as usize).min(split - 1);
    Some((first + bin * 20 + segment) as u8)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use oche_domain::board::{BoardLine, BoardPoint};

    /// 2 mm per pixel, the bull at the centre of a 200x200 frame.
    pub(crate) struct Scaled;

    impl LineMapper for Scaled {
        fn line_to_board(&self, a: (f64, f64), b: (f64, f64)) -> Option<BoardLine> {
            let p = |q: (f64, f64)| BoardPoint::new((q.0 - 100.0) * 2.0, (100.0 - q.1) * 2.0);
            BoardLine::through(p(a), p(b))
        }

        fn pixel_to_board(&self, pixel: (f64, f64)) -> Option<(f64, f64)> {
            Some(((pixel.0 - 100.0) * 2.0, (100.0 - pixel.1) * 2.0))
        }
    }

    /// A board drawn with a different grey per cell kind, wires darker.
    pub(crate) fn board_frame(board: &BoardDimensions, gain: f64) -> GrayImage {
        let (width, height) = (200, 200);
        let mut data = vec![0u8; width * height];
        for y in 0..height {
            for x in 0..width {
                let (bx, by) = Scaled.pixel_to_board((x as f64, y as f64)).unwrap();
                let hit = board.hit(BoardPoint::new(bx, by));
                let dark = hit
                    .label()
                    .trim_start_matches(['S', 'D', 'T'])
                    .parse::<u32>()
                    .is_ok_and(|n| n % 2 == 0);
                let r = bx.hypot(by);
                let level = if r > board.double_outer {
                    30.0
                } else if (board.treble_inner..board.treble_outer).contains(&r)
                    || (board.double_inner..board.double_outer).contains(&r)
                {
                    if dark { 90.0 } else { 140.0 }
                } else if dark {
                    40.0
                } else {
                    200.0
                };
                data[y * width + x] = (level * gain).min(255.0) as u8;
            }
        }
        GrayImage {
            width,
            height,
            data,
        }
    }

    fn outlying(image: &GrayImage) -> f64 {
        let board = BoardDimensions::wdf();
        let settings = OccupancySettings::default();
        Cells::new(&Scaled, &board, image.width, image.height, &settings)
            .outlying_mm2(image, &settings)
            .unwrap()
    }

    #[test]
    fn an_empty_board_is_uniform_in_every_cell_at_any_exposure() {
        let board = BoardDimensions::wdf();
        for gain in [0.5, 1.0, 1.25] {
            assert_eq!(outlying(&board_frame(&board, gain)), 0.0, "gain {gain}");
        }
    }

    #[test]
    fn a_dart_across_the_cells_stands_out() {
        let board = BoardDimensions::wdf();
        let mut image = board_frame(&board, 1.0);
        // A 3 px (6 mm) wide barrel from the treble outward, mid grey: it
        // differs from both the dark and the light cells it crosses.
        for y in 40..70 {
            for x in 120..123 {
                image.data[y * image.width + x] = 120;
            }
        }
        let area = outlying(&image);
        assert!(
            area > OccupancySettings::default().occupied_mm2,
            "{area} mm²"
        );
    }

    #[test]
    fn specks_do_not_count() {
        let board = BoardDimensions::wdf();
        let mut image = board_frame(&board, 1.0);
        // Isolated single pixels, like holes or fibres.
        for (x, y) in [(70, 70), (130, 60), (90, 140), (150, 110)] {
            image.data[y * image.width + x] = 255;
        }
        assert_eq!(outlying(&image), 0.0);
    }

    #[test]
    fn cells_cover_the_scoring_area_away_from_the_wires() {
        let board = BoardDimensions::wdf();
        let at = |r: f64, degrees: f64| {
            let p = BoardPoint::polar_cw_from_top(r, degrees);
            cell(&board, 2.0, 3, (p.x, p.y))
        };
        assert_eq!(at(3.0, 0.0), Some(0));
        assert_eq!(at(11.0, 0.0), Some(1));
        assert_eq!(at(board.treble_inner, 0.0), None, "on a ring wire");
        assert_eq!(at(60.0, 9.0), None, "on a radial wire");
        assert_eq!(at(board.double_outer + 5.0, 0.0), None, "off the board");
        // Neighbouring segments and rings get different cells.
        let treble = (board.treble_inner + board.treble_outer) / 2.0;
        assert_ne!(at(treble, 0.0), at(treble, 18.0));
        assert_ne!(at(treble, 0.0), at(60.0, 0.0));
        // The last cell, the double of the segment before the 20, is in range.
        assert!(at(165.0, 342.0).is_some_and(|c| (c as usize) < 2 + 2 * 3 * 20 + 40));
    }
}
