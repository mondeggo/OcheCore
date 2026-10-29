//! Automatic anchor detection from an empty-board frame.
//!
//! The double and treble rings are bands of alternating red and green segments.
//! Rays cast from the projected bull centre are images of board radii, because a
//! homography maps lines through the centre to lines through its image. Along
//! each ray the coloured bands give the four ring edges, and around each ring the
//! red/green changes give the radial wires. Each wire yields one anchor at the
//! middle of the double band and one at the middle of the treble band. Only the
//! orientation needs a hint: a pixel near the 20 segment, because the colour
//! pattern repeats every two segments.
//!
//! Colour thresholds were chosen from the 2026-10-29 `empty-calib-autowb`
//! recording under the board's light ring with auto white balance. They are not
//! validated for other lighting. Band middles are used instead of band edges:
//! on the recorded frames the classified bands were only about half their
//! nominal 8 mm width, consistent with JPEG chroma subsampling eroding colour at
//! the edges. Symmetric erosion cancels at the middle; asymmetric erosion and
//! deviations of the real board from the nominal radii remain in the residuals.

use std::f64::consts::TAU;

use oche_domain::board::{BoardDimensions, BoardPoint, SEGMENTS};
use oche_vision::image::RgbImage;

use crate::{Correspondence, Fit, FitError, Homography, Pixel, Residuals};

const RAYS: usize = 1440;
const RAY_STEP: f64 = 0.5;
/// Dilation radius that bridges radial wires between coloured segments.
const BRIDGE: i64 = 2;
/// Rays in a colour run shorter than this are treated as noise.
const MIN_ARC_RAYS: usize = 4;
const ITERATIONS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Colour {
    Red,
    Green,
}

fn classify([r, g, b]: [u8; 3]) -> Option<Colour> {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    if r >= 90 && r - g.max(b) >= 60 {
        Some(Colour::Red)
    } else if g >= 70 && g - r >= 50 && g >= b {
        Some(Colour::Green)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetectedAnchor {
    pub name: String,
    pub correspondence: Correspondence,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub centre: Pixel,
    pub anchors: Vec<DetectedAnchor>,
    pub fit: Fit,
    /// Treble anchors checked against a fit made from double anchors only.
    pub treble_holdout: Residuals,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DetectError {
    RingsNotFound(&'static str),
    Boundaries { ring: &'static str, found: usize },
    HintNotNearRing,
    Fit(FitError),
}

impl std::fmt::Display for DetectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RingsNotFound(what) => write!(f, "could not find the {what}"),
            Self::Boundaries { ring, found } => {
                write!(
                    f,
                    "found {found} colour boundaries on the {ring} ring, expected 20"
                )
            }
            Self::HintNotNearRing => f.write_str("no red double segment near the 20 hint"),
            Self::Fit(error) => write!(f, "fit failed: {error}"),
        }
    }
}

impl std::error::Error for DetectError {}

pub fn detect(
    image: &RgbImage,
    hint_20: Pixel,
    board: &BoardDimensions,
) -> Result<Detection, DetectError> {
    let colours: Vec<Option<Colour>> = image
        .data
        .chunks_exact(3)
        .map(|p| classify([p[0], p[1], p[2]]))
        .collect();
    let components = Components::label(image.width, image.height, &colours);
    let (double, treble, mut centre) = components.rings()?;

    let mut result = None;
    for _ in 0..ITERATIONS {
        let rays = cast(image, &colours, &components, centre, double, treble);
        let anchors = anchors_from_rays(&rays, centre, hint_20, board)?;
        let points: Vec<_> = anchors.iter().map(|a| a.correspondence).collect();
        let fit = Homography::fit(&points).map_err(DetectError::Fit)?;
        let next = fit
            .homography
            .to_image(BoardPoint::new(0.0, 0.0))
            .ok_or(DetectError::Fit(FitError::Degenerate))?;
        let moved = (next.x - centre.x).hypot(next.y - centre.y);
        result = Some((anchors, fit));
        centre = next;
        if moved < 0.05 {
            break;
        }
    }
    let (anchors, fit) = result.expect("at least one iteration");
    let doubles: Vec<_> = anchors
        .iter()
        .filter(|a| a.name.starts_with("double"))
        .map(|a| a.correspondence)
        .collect();
    let trebles: Vec<_> = anchors
        .iter()
        .filter(|a| a.name.starts_with("treble"))
        .map(|a| a.correspondence)
        .collect();
    let treble_holdout = Homography::fit(&doubles)
        .map_err(DetectError::Fit)?
        .homography
        .residuals(&trebles);
    Ok(Detection {
        centre,
        anchors,
        fit,
        treble_holdout,
    })
}

struct Components {
    width: usize,
    labels: Vec<u32>,
    stats: Vec<Stats>,
}

#[derive(Debug, Clone, Copy)]
struct Stats {
    count: usize,
    min: (usize, usize),
    max: (usize, usize),
    sum: (f64, f64),
}

impl Stats {
    fn bbox_area(&self) -> usize {
        (self.max.0 - self.min.0 + 1) * (self.max.1 - self.min.1 + 1)
    }

    fn inside(&self, outer: &Stats) -> bool {
        self.min.0 > outer.min.0
            && self.min.1 > outer.min.1
            && self.max.0 < outer.max.0
            && self.max.1 < outer.max.1
    }

    fn centroid(&self) -> Pixel {
        Pixel::new(
            self.sum.0 / self.count as f64,
            self.sum.1 / self.count as f64,
        )
    }
}

impl Components {
    /// Labels 4-connected regions of the dilated colour mask. Label 0 is empty.
    fn label(width: usize, height: usize, colours: &[Option<Colour>]) -> Self {
        let mut mask = vec![false; colours.len()];
        for y in 0..height as i64 {
            for x in 0..width as i64 {
                if colours[y as usize * width + x as usize].is_none() {
                    continue;
                }
                for dy in -BRIDGE..=BRIDGE {
                    for dx in -BRIDGE..=BRIDGE {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < width && (ny as usize) < height {
                            mask[ny as usize * width + nx as usize] = true;
                        }
                    }
                }
            }
        }
        let mut labels = vec![0u32; colours.len()];
        let mut stats = Vec::new();
        let mut stack = Vec::new();
        for start in 0..mask.len() {
            if !mask[start] || labels[start] != 0 {
                continue;
            }
            let label = stats.len() as u32 + 1;
            let mut s = Stats {
                count: 0,
                min: (usize::MAX, usize::MAX),
                max: (0, 0),
                sum: (0.0, 0.0),
            };
            labels[start] = label;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let (x, y) = (i % width, i / width);
                s.count += 1;
                s.min = (s.min.0.min(x), s.min.1.min(y));
                s.max = (s.max.0.max(x), s.max.1.max(y));
                s.sum = (s.sum.0 + x as f64, s.sum.1 + y as f64);
                let neighbours = [
                    (x > 0).then(|| i - 1),
                    (x + 1 < width).then(|| i + 1),
                    (y > 0).then(|| i - width),
                    (y + 1 < height).then(|| i + width),
                ];
                for n in neighbours.into_iter().flatten() {
                    if mask[n] && labels[n] == 0 {
                        labels[n] = label;
                        stack.push(n);
                    }
                }
            }
            stats.push(s);
        }
        Self {
            width,
            labels,
            stats,
        }
    }

    fn at(&self, x: usize, y: usize) -> u32 {
        self.labels[y * self.width + x]
    }

    /// Double ring, treble ring, and the bull centroid.
    fn rings(&self) -> Result<(u32, u32, Pixel), DetectError> {
        let index = |i: usize| i as u32 + 1;
        let (double_i, double) = self
            .stats
            .iter()
            .enumerate()
            .max_by_key(|(_, s)| s.bbox_area())
            .ok_or(DetectError::RingsNotFound("double ring"))?;
        let (treble_i, treble) = self
            .stats
            .iter()
            .enumerate()
            .filter(|(i, s)| *i != double_i && s.inside(double))
            .max_by_key(|(_, s)| s.count)
            .ok_or(DetectError::RingsNotFound("treble ring"))?;
        let target = treble.centroid();
        let (_, bull) = self
            .stats
            .iter()
            .enumerate()
            .filter(|(i, s)| *i != treble_i && s.inside(treble))
            .min_by(|(_, a), (_, b)| {
                let da = (a.centroid().x - target.x).hypot(a.centroid().y - target.y);
                let db = (b.centroid().x - target.x).hypot(b.centroid().y - target.y);
                da.total_cmp(&db)
            })
            .ok_or(DetectError::RingsNotFound("bull"))?;
        Ok((index(double_i), index(treble_i), bull.centroid()))
    }
}

/// Inner and outer edge distance along a ray and the band's majority colour.
#[derive(Debug, Clone, Copy)]
struct Band {
    inner: f64,
    outer: f64,
    colour: Colour,
}

struct Ray {
    angle: f64,
    double: Option<Band>,
    treble: Option<Band>,
}

fn cast(
    image: &RgbImage,
    colours: &[Option<Colour>],
    components: &Components,
    centre: Pixel,
    double: u32,
    treble: u32,
) -> Vec<Ray> {
    let reach = (image.width as f64).hypot(image.height as f64);
    (0..RAYS)
        .map(|i| {
            let angle = TAU * i as f64 / RAYS as f64;
            let (sin, cos) = angle.sin_cos();
            let mut bands = [(double, None::<(f64, f64, i32)>), (treble, None)];
            let mut s = 0.0;
            while s < reach {
                let (x, y) = ((centre.x + cos * s).round(), (centre.y + sin * s).round());
                if x < 0.0 || y < 0.0 || x >= image.width as f64 || y >= image.height as f64 {
                    break;
                }
                let (x, y) = (x as usize, y as usize);
                let label = components.at(x, y);
                if let Some(colour) = colours[y * image.width + x] {
                    for (target, band) in &mut bands {
                        if label == *target {
                            let vote = if colour == Colour::Red { 1 } else { -1 };
                            let entry = band.get_or_insert((s, s, 0));
                            entry.1 = s;
                            entry.2 += vote;
                        }
                    }
                }
                s += RAY_STEP;
            }
            let finish = |band: Option<(f64, f64, i32)>| {
                band.filter(|b| b.2 != 0).map(|(inner, outer, votes)| Band {
                    // Edges lie halfway between the last outside and first inside sample.
                    inner: inner - RAY_STEP / 2.0,
                    outer: outer + RAY_STEP / 2.0,
                    colour: if votes > 0 {
                        Colour::Red
                    } else {
                        Colour::Green
                    },
                })
            };
            Ray {
                angle,
                double: finish(bands[0].1),
                treble: finish(bands[1].1),
            }
        })
        .collect()
}

/// Colour boundary between consecutive rays `before` and `after` of one ring.
#[derive(Debug, Clone, Copy)]
struct Boundary {
    angle: f64,
    inner: f64,
    outer: f64,
    /// Colour of the arc that starts here and runs clockwise.
    next: Colour,
}

fn boundaries(rays: &[Ray], pick: impl Fn(&Ray) -> Option<Band>) -> Vec<Boundary> {
    let valid: Vec<(f64, Band)> = rays
        .iter()
        .filter_map(|r| pick(r).map(|b| (r.angle, b)))
        .collect();
    if valid.is_empty() {
        return Vec::new();
    }
    // Merge short runs into their neighbours so isolated misclassifications do
    // not create boundaries.
    let mut colours: Vec<Colour> = valid.iter().map(|(_, b)| b.colour).collect();
    let n = colours.len();
    for _ in 0..3 {
        let runs = runs(&colours);
        if runs.len() < 2 {
            break;
        }
        for (start, len) in &runs {
            if *len < MIN_ARC_RAYS {
                let before = colours[(start + n - 1) % n];
                for k in 0..*len {
                    colours[(start + k) % n] = before;
                }
            }
        }
    }
    runs(&colours)
        .into_iter()
        .map(|(start, _)| {
            let (a0, b0) = valid[(start + n - 1) % n];
            let (a1, b1) = valid[start];
            let span = (a1 - a0).rem_euclid(TAU);
            Boundary {
                angle: (a0 + span / 2.0).rem_euclid(TAU),
                inner: (b0.inner + b1.inner) / 2.0,
                outer: (b0.outer + b1.outer) / 2.0,
                next: colours[start],
            }
        })
        .collect()
}

/// Circular runs as (start, length); a run starts where the colour changes.
fn runs(colours: &[Colour]) -> Vec<(usize, usize)> {
    let n = colours.len();
    let starts: Vec<usize> = (0..n)
        .filter(|&i| colours[i] != colours[(i + n - 1) % n])
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(k, &start)| {
            let end = starts[(k + 1) % starts.len()];
            (start, (end + n - start - 1) % n + 1)
        })
        .collect()
}

fn angle_distance(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(TAU);
    d.min(TAU - d)
}

fn anchors_from_rays(
    rays: &[Ray],
    centre: Pixel,
    hint: Pixel,
    board: &BoardDimensions,
) -> Result<Vec<DetectedAnchor>, DetectError> {
    let doubles = boundaries(rays, |r| r.double);
    let trebles = boundaries(rays, |r| r.treble);
    for (ring, found) in [("double", doubles.len()), ("treble", trebles.len())] {
        if found != 20 {
            return Err(DetectError::Boundaries { ring, found });
        }
    }
    // The 20 arc is the red double arc whose middle is nearest the hint.
    let arc_middle = |list: &[Boundary], k: usize| {
        let (a, b) = (list[k], list[(k + 1) % 20]);
        let span = (b.angle - a.angle).rem_euclid(TAU);
        (a.angle + span / 2.0).rem_euclid(TAU)
    };
    let twenty = (0..20)
        .filter(|&k| doubles[k].next == Colour::Red)
        .min_by(|&a, &b| {
            let point = |k: usize| {
                let angle = arc_middle(&doubles, k);
                let s = (doubles[k].inner + doubles[k].outer) / 2.0;
                (centre.x + angle.cos() * s - hint.x).hypot(centre.y + angle.sin() * s - hint.y)
            };
            point(a).total_cmp(&point(b))
        })
        .ok_or(DetectError::HintNotNearRing)?;
    let twenty_angle = arc_middle(&doubles, twenty);
    let treble_twenty = (0..20)
        .filter(|&k| trebles[k].next == Colour::Red)
        .min_by(|&a, &b| {
            angle_distance(arc_middle(&trebles, a), twenty_angle)
                .total_cmp(&angle_distance(arc_middle(&trebles, b), twenty_angle))
        })
        .ok_or(DetectError::RingsNotFound("treble 20"))?;

    let mut anchors = Vec::with_capacity(40);
    for (ring, list, first, radius) in [
        (
            "double",
            &doubles,
            twenty,
            (board.double_inner + board.double_outer) / 2.0,
        ),
        (
            "treble",
            &trebles,
            treble_twenty,
            (board.treble_inner + board.treble_outer) / 2.0,
        ),
    ] {
        for k in 0..20 {
            // Arc `first` runs from boundary 5/20 to 20/1, so boundary `k`
            // (between SEGMENTS[k] and SEGMENTS[k + 1]) starts arc first + k + 1.
            let boundary = list[(first + k + 1) % 20];
            let (sin, cos) = boundary.angle.sin_cos();
            let s = (boundary.inner + boundary.outer) / 2.0;
            anchors.push(DetectedAnchor {
                name: format!("{ring}_mid:{}/{}", SEGMENTS[k], SEGMENTS[(k + 1) % 20]),
                correspondence: Correspondence {
                    image: Pixel::new(centre.x + cos * s, centre.y + sin * s),
                    board: BoardPoint::polar_cw_from_top(radius, 18.0 * (k as f64 + 0.5)),
                },
            });
        }
    }
    Ok(anchors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oche_domain::board::Ring;

    /// Renders a synthetic board through a perspective mapping, colouring only
    /// the treble/double bands and bulls as the real board does.
    fn render(image_to_board: &Homography, rotation_deg: f64) -> RgbImage {
        let board = BoardDimensions::wdf();
        let mut image = RgbImage::new(640, 400);
        for y in 0..image.height {
            for x in 0..image.width {
                let Some(p) = image_to_board.to_board(Pixel::new(x as f64, y as f64)) else {
                    continue;
                };
                // Rotate the board within the image to exercise the hint.
                let (sin, cos) = rotation_deg.to_radians().sin_cos();
                let p = BoardPoint::new(p.x * cos - p.y * sin, p.x * sin + p.y * cos);
                let hit = board.hit(p);
                let index = hit
                    .segment
                    .and_then(|s| SEGMENTS.iter().position(|&v| v == s));
                let colour = match (hit.ring, index) {
                    (Ring::InnerBull, _) => [200, 20, 40],
                    (Ring::OuterBull, _) => [10, 150, 110],
                    (Ring::Treble | Ring::Double, Some(i)) if i % 2 == 0 => [200, 20, 40],
                    (Ring::Treble | Ring::Double, Some(_)) => [10, 150, 110],
                    (Ring::InnerSingle | Ring::OuterSingle, Some(i)) if i % 2 == 0 => [20, 20, 25],
                    (Ring::InnerSingle | Ring::OuterSingle, Some(_)) => [185, 190, 190],
                    _ => [30, 30, 30],
                };
                image.data[(y * image.width + x) * 3..][..3].copy_from_slice(&colour);
            }
        }
        image
    }

    fn camera() -> Homography {
        // Image → board for a mildly oblique view centred at (320, 200).
        let points: Vec<_> = [
            ((320.0, 200.0), (0.0, 0.0)),
            ((520.0, 205.0), (180.0, 0.0)),
            ((315.0, 60.0), (0.0, 180.0)),
            ((125.0, 195.0), (-180.0, 0.0)),
            ((325.0, 360.0), (0.0, -180.0)),
            ((470.0, 90.0), (127.0, 127.0)),
        ]
        .iter()
        .map(|&((ix, iy), (bx, by))| Correspondence {
            image: Pixel::new(ix, iy),
            board: BoardPoint::new(bx, by),
        })
        .collect();
        Homography::fit(&points).unwrap().homography
    }

    #[test]
    fn detects_all_anchors_on_a_synthetic_perspective_board() {
        let mapping = camera();
        let image = render(&mapping, 0.0);
        let hint = mapping.to_image(BoardPoint::new(0.0, 190.0)).unwrap();
        let detection = detect(&image, hint, &BoardDimensions::wdf()).unwrap();
        assert_eq!(detection.anchors.len(), 40);
        // Pixel quantization limits accuracy; 1 px is about 1 mm here.
        assert!(
            detection.fit.residuals.rms_mm < 1.5,
            "{:?}",
            detection.fit.residuals.rms_mm
        );
        assert!(
            detection.treble_holdout.rms_mm < 2.5,
            "{:?}",
            detection.treble_holdout.rms_mm
        );
        let bull = mapping.to_image(BoardPoint::new(0.0, 0.0)).unwrap();
        assert!((detection.centre.x - bull.x).hypot(detection.centre.y - bull.y) < 1.5);
        let twenty = detection
            .fit
            .homography
            .to_board(mapping.to_image(BoardPoint::new(0.0, 140.0)).unwrap())
            .unwrap();
        assert_eq!(BoardDimensions::wdf().hit(twenty).label(), "S20");
    }

    #[test]
    fn hint_selects_orientation_among_repeating_colours() {
        let mapping = camera();
        // Rotate the printed board by two segments; 20 now sits where 18 was.
        let image = render(&mapping, 36.0);
        let hint = mapping
            .to_image(BoardPoint::polar_cw_from_top(190.0, 36.0))
            .unwrap();
        let detection = detect(&image, hint, &BoardDimensions::wdf()).unwrap();
        let probe = mapping
            .to_image(BoardPoint::polar_cw_from_top(140.0, 36.0))
            .unwrap();
        let mapped = detection.fit.homography.to_board(probe).unwrap();
        assert_eq!(BoardDimensions::wdf().hit(mapped).label(), "S20");
    }

    #[test]
    fn missing_rings_are_reported() {
        let blank = RgbImage::new(64, 64);
        assert!(matches!(
            detect(&blank, Pixel::new(0.0, 0.0), &BoardDimensions::wdf()),
            Err(DetectError::RingsNotFound(_))
        ));
    }
}
