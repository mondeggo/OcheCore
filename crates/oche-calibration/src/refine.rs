//! Board measurement and calibration refinement from wire centres.
//!
//! Starting from rough per-camera mappings (for example from
//! [`crate::detect`]), each view is resampled in board polar coordinates and the
//! metal wires are located as thin bright ridges in the luminance profile across
//! each expected wire. A ridge peak is the wire centre, which is the scoring
//! boundary regardless of wire width. Ring wires are only measured where the
//! adjacent singles are black, because next to a white single the wire does not
//! stand out as a ridge. Radial wires are measured inside the coloured bands.
//!
//! All views and the ring radii are then fitted together: every camera must map
//! its wire pixels onto circles of shared radii and onto the 20 radial lines.
//! One radius stays fixed to set the scale (by default the double's outer wire,
//! nominally 170 mm); the others are measured. Finding wires and fitting
//! alternate until the solution settles.

use oche_domain::board::{BoardDimensions, BoardPoint};
use oche_vision::mask::GrayImage;

use crate::{Homography, Matrix3, Pixel, apply, dehomogenize, invert, multiply};

/// Ring boundary order used throughout: inner bull, outer bull, treble inner,
/// treble outer, double inner, double outer.
pub const RINGS: [&str; 6] = [
    "inner_bull",
    "outer_bull",
    "treble_inner",
    "treble_outer",
    "double_inner",
    "double_outer",
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Half-width of the search window across an expected wire.
    pub window_mm: f64,
    pub step_mm: f64,
    /// Minimum ridge height above the brighter of the two sides, in grey levels.
    pub min_contrast: f64,
    pub angle_step_deg: f64,
    /// Ring samples closer than this to a radial wire are skipped.
    pub radial_clearance_deg: f64,
    /// Ring whose radius stays fixed to set the scale (index into [`RINGS`]).
    pub fixed_ring: usize,
    pub rounds: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window_mm: 2.5,
            step_mm: 0.05,
            min_contrast: 12.0,
            angle_step_deg: 0.5,
            radial_clearance_deg: 2.0,
            fixed_ring: 5,
            rounds: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    Ring(usize),
    /// Radial wire `k`, between `SEGMENTS[k]` and `SEGMENTS[k + 1]`.
    Radial(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WirePoint {
    pub view: usize,
    pub pixel: Pixel,
    pub wire: Wire,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewFit {
    pub ring_rms_mm: [f64; 6],
    pub ring_points: [usize; 6],
    pub radial_rms_mm: f64,
    pub radial_points: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Refinement {
    /// Measured wire-centre radii, scaled so the fixed ring keeps its value.
    pub board: BoardDimensions,
    pub homographies: Vec<Homography>,
    pub views: Vec<ViewFit>,
    /// Wire points used in the final fit.
    pub points: Vec<WirePoint>,
}

pub fn dimensions(radii: [f64; 6]) -> BoardDimensions {
    BoardDimensions {
        inner_bull: radii[0],
        outer_bull: radii[1],
        treble_inner: radii[2],
        treble_outer: radii[3],
        double_inner: radii[4],
        double_outer: radii[5],
    }
}

fn polar(radius: f64, degrees: f64) -> (f64, f64) {
    let (sin, cos) = degrees.to_radians().sin_cos();
    (radius * sin, radius * cos)
}

/// Clockwise angle of radial wire `k` from the top, in degrees.
fn radial_degrees(k: usize) -> f64 {
    18.0 * k as f64 + 9.0
}

/// Whether the single segment at `degrees` is black (20 and every other one).
fn black_single(degrees: f64) -> bool {
    let index = ((degrees + 9.0).rem_euclid(360.0) / 18.0) as usize % 20;
    index % 2 == 0
}

fn near_radial(degrees: f64, clearance: f64) -> bool {
    let offset = (degrees - 9.0).rem_euclid(18.0);
    offset.min(18.0 - offset) < clearance
}

/// Sub-sample position of a bright ridge strictly inside `profile`.
fn ridge(profile: &[f64], min_contrast: f64) -> Option<f64> {
    let (peak, &value) = profile
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    if peak < 2 || peak + 2 >= profile.len() {
        return None;
    }
    let left = profile[..peak]
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let right = profile[peak + 1..]
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    if value - left.max(right) < min_contrast {
        return None;
    }
    let (a, b, c) = (profile[peak - 1], value, profile[peak + 1]);
    let curvature = a - 2.0 * b + c;
    let offset = if curvature < 0.0 {
        0.5 * (a - c) / curvature
    } else {
        0.0
    };
    Some(peak as f64 + offset.clamp(-0.5, 0.5))
}

/// Locates wire centres in one view using the current mapping and radii.
pub fn find_wires(
    view: usize,
    image: &GrayImage,
    mapping: &Homography,
    radii: [f64; 6],
    settings: &Settings,
) -> Vec<WirePoint> {
    let steps = (2.0 * settings.window_mm / settings.step_mm).round() as usize + 1;
    let sample = |point: (f64, f64)| {
        mapping
            .to_image(BoardPoint::new(point.0, point.1))
            .and_then(|p| image.bilinear(p.x, p.y))
    };
    let mut points = Vec::new();
    let mut push = |board: (f64, f64), wire: Wire| {
        if let Some(pixel) = mapping.to_image(BoardPoint::new(board.0, board.1)) {
            points.push(WirePoint { view, pixel, wire });
        }
    };

    let angles = (360.0 / settings.angle_step_deg) as usize;
    for (ring, &radius) in radii.iter().enumerate() {
        // Keep windows inside the gap to the neighbouring wires.
        let gap = [
            ring.checked_sub(1).map(|i| radii[ring] - radii[i]),
            radii.get(ring + 1).map(|r| r - radii[ring]),
        ]
        .into_iter()
        .flatten()
        .fold(f64::INFINITY, f64::min);
        let window = settings.window_mm.min(gap * 0.45);
        let steps = (2.0 * window / settings.step_mm).round() as usize + 1;
        for a in 0..angles {
            let degrees = a as f64 * settings.angle_step_deg;
            // The bull wire separates red and green everywhere; the others are
            // only ridges where the single beside them is black.
            if ring > 0
                && (!black_single(degrees) || near_radial(degrees, settings.radial_clearance_deg))
            {
                continue;
            }
            let profile: Option<Vec<f64>> = (0..steps)
                .map(|i| {
                    sample(polar(
                        radius - window + i as f64 * settings.step_mm,
                        degrees,
                    ))
                })
                .collect();
            if let Some(at) = profile.and_then(|p| ridge(&p, settings.min_contrast)) {
                push(
                    polar(radius - window + at * settings.step_mm, degrees),
                    Wire::Ring(ring),
                );
            }
        }
    }

    for k in 0..20 {
        let degrees = radial_degrees(k);
        let (sin, cos) = degrees.to_radians().sin_cos();
        let tangent = (cos, -sin);
        for (inner, outer) in [(radii[2], radii[3]), (radii[4], radii[5])] {
            let mut radius = inner + 1.5;
            while radius <= outer - 1.5 {
                let centre = polar(radius, degrees);
                let at_offset = |d: f64| (centre.0 + tangent.0 * d, centre.1 + tangent.1 * d);
                let profile: Option<Vec<f64>> = (0..steps)
                    .map(|i| sample(at_offset(-settings.window_mm + i as f64 * settings.step_mm)))
                    .collect();
                if let Some(at) = profile.and_then(|p| ridge(&p, settings.min_contrast)) {
                    push(
                        at_offset(-settings.window_mm + at * settings.step_mm),
                        Wire::Radial(k),
                    );
                }
                radius += 0.5;
            }
        }
    }
    points
}

/// Per-view parameterization: `H = H0 · N⁻¹ · (I + Δ) · N` with `N`
/// normalizing pixels around the projected centre, so all 8 entries of `Δ` are
/// of similar scale.
struct ViewModel {
    start: Matrix3,
    normalize: Matrix3,
    denormalize: Matrix3,
}

impl ViewModel {
    fn new(start: &Homography) -> Option<Self> {
        let centre = start.to_image(BoardPoint::new(0.0, 0.0))?;
        let edge = start.to_image(BoardPoint::new(170.0, 0.0))?;
        let scale = 1.0 / (edge.x - centre.x).hypot(edge.y - centre.y).max(1.0);
        let normalize = [
            [scale, 0.0, -scale * centre.x],
            [0.0, scale, -scale * centre.y],
            [0.0, 0.0, 1.0],
        ];
        Some(Self {
            start: start.matrix(),
            denormalize: invert(&normalize)?,
            normalize,
        })
    }

    fn matrix(&self, delta: &[f64]) -> Matrix3 {
        let adjust = [
            [1.0 + delta[0], delta[1], delta[2]],
            [delta[3], 1.0 + delta[4], delta[5]],
            [delta[6], delta[7], 1.0],
        ];
        multiply(
            &multiply(&self.start, &self.denormalize),
            &multiply(&adjust, &self.normalize),
        )
    }
}

/// Parameters: 8 per view, then one per free ring.
struct Problem<'a> {
    views: Vec<ViewModel>,
    points: &'a [WirePoint],
    free: Vec<usize>,
    radii: [f64; 6],
}

impl Problem<'_> {
    fn radius(&self, params: &[f64], ring: usize) -> f64 {
        match self.free.iter().position(|&r| r == ring) {
            Some(i) => params[self.views.len() * 8 + i],
            None => self.radii[ring],
        }
    }

    fn residual(&self, params: &[f64], point: &WirePoint, matrix: &Matrix3) -> Option<f64> {
        let (x, y) = dehomogenize(apply(matrix, point.pixel.x, point.pixel.y))?;
        Some(match point.wire {
            Wire::Ring(ring) => x.hypot(y) - self.radius(params, ring),
            Wire::Radial(k) => {
                let (sin, cos) = radial_degrees(k).to_radians().sin_cos();
                x * cos - y * sin
            }
        })
    }

    /// Residuals and the sparse gradient of each with respect to its parameters.
    fn linearize(&self, params: &[f64], use_point: &[bool]) -> (f64, Vec<Vec<f64>>, Vec<f64>) {
        let n = params.len();
        let mut normal = vec![vec![0.0; n]; n];
        let mut gradient = vec![0.0; n];
        let mut cost = 0.0;
        let matrices: Vec<Matrix3> = (0..self.views.len())
            .map(|v| self.views[v].matrix(&params[v * 8..v * 8 + 8]))
            .collect();
        for (point, _) in self.points.iter().zip(use_point).filter(|(_, u)| **u) {
            let Some(r) = self.residual(params, point, &matrices[point.view]) else {
                continue;
            };
            let mut indices: Vec<usize> = (point.view * 8..point.view * 8 + 8).collect();
            if let Wire::Ring(ring) = point.wire
                && let Some(i) = self.free.iter().position(|&f| f == ring)
            {
                indices.push(self.views.len() * 8 + i);
            }
            let mut derivatives = Vec::with_capacity(indices.len());
            for &i in &indices {
                let step = if i < self.views.len() * 8 { 1e-7 } else { 1e-5 };
                let mut shifted = params.to_vec();
                shifted[i] += step;
                let matrix =
                    self.views[point.view].matrix(&shifted[point.view * 8..point.view * 8 + 8]);
                let r2 = self.residual(&shifted, point, &matrix).unwrap_or(r);
                derivatives.push((r2 - r) / step);
            }
            for (a, &i) in indices.iter().enumerate() {
                gradient[i] += derivatives[a] * r;
                for (b, &j) in indices.iter().enumerate() {
                    normal[i][j] += derivatives[a] * derivatives[b];
                }
            }
            cost += r * r;
        }
        (cost, normal, gradient)
    }

    fn cost(&self, params: &[f64], use_point: &[bool]) -> f64 {
        let matrices: Vec<Matrix3> = (0..self.views.len())
            .map(|v| self.views[v].matrix(&params[v * 8..v * 8 + 8]))
            .collect();
        self.points
            .iter()
            .zip(use_point)
            .filter(|(_, u)| **u)
            .filter_map(|(p, _)| self.residual(params, p, &matrices[p.view]))
            .map(|r| r * r)
            .sum()
    }

    /// Levenberg–Marquardt on the selected points.
    fn solve(&self, mut params: Vec<f64>, use_point: &[bool]) -> Vec<f64> {
        let mut lambda = 1e-3;
        for _ in 0..50 {
            let (cost, normal, gradient) = self.linearize(&params, use_point);
            let mut improved = false;
            for _ in 0..10 {
                let mut damped = normal.clone();
                for (i, row) in damped.iter_mut().enumerate() {
                    row[i] += lambda * normal[i][i].max(1e-9);
                }
                let Some(step) = solve_linear(damped, gradient.iter().map(|g| -g).collect()) else {
                    lambda *= 10.0;
                    continue;
                };
                let candidate: Vec<f64> = params.iter().zip(&step).map(|(p, s)| p + s).collect();
                if self.cost(&candidate, use_point) < cost {
                    let size = step.iter().map(|s| s * s).sum::<f64>().sqrt();
                    params = candidate;
                    lambda = (lambda / 3.0).max(1e-9);
                    improved = size > 1e-10;
                    break;
                }
                lambda *= 4.0;
            }
            if !improved {
                break;
            }
        }
        params
    }
}

/// Gaussian elimination with partial pivoting.
fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..n {
            let factor = a[row][col] / a[col][col];
            if factor == 0.0 {
                continue;
            }
            for k in col..n {
                a[row][k] -= factor * a[col][k];
            }
            b[row] -= factor * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let sum: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - sum) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Robust scale (1.4826 × median absolute residual).
fn robust_sigma(residuals: &[f64]) -> f64 {
    let mut abs: Vec<f64> = residuals.iter().map(|r| r.abs()).collect();
    if abs.is_empty() {
        return 0.0;
    }
    abs.sort_by(f64::total_cmp);
    1.4826 * abs[abs.len() / 2]
}

/// Jointly measures ring radii and refines every view's mapping.
pub fn refine(
    views: &[(&GrayImage, Homography)],
    start: &BoardDimensions,
    settings: &Settings,
) -> Result<Refinement, String> {
    if views.is_empty() {
        return Err("no views to refine".into());
    }
    let mut radii = start.radii();
    let mut mappings: Vec<Homography> = views.iter().map(|(_, h)| *h).collect();
    let free: Vec<usize> = (0..6).filter(|&r| r != settings.fixed_ring).collect();
    let mut points = Vec::new();
    let mut use_point = Vec::new();
    for _ in 0..settings.rounds {
        points = views
            .iter()
            .zip(&mappings)
            .enumerate()
            .flat_map(|(v, ((image, _), mapping))| find_wires(v, image, mapping, radii, settings))
            .collect();
        let models: Vec<ViewModel> = mappings
            .iter()
            .map(ViewModel::new)
            .collect::<Option<_>>()
            .ok_or("a view does not map the board centre")?;
        let problem = Problem {
            views: models,
            points: &points,
            free: free.clone(),
            radii,
        };
        let mut params = vec![0.0; views.len() * 8];
        params.extend(free.iter().map(|&r| radii[r]));
        use_point = vec![true; points.len()];
        params = problem.solve(params, &use_point);
        // Drop outliers (darts, glare, misdetected ridges) and solve again.
        let matrices: Vec<Matrix3> = (0..views.len())
            .map(|v| problem.views[v].matrix(&params[v * 8..v * 8 + 8]))
            .collect();
        let residuals: Vec<f64> = points
            .iter()
            .map(|p| {
                problem
                    .residual(&params, p, &matrices[p.view])
                    .unwrap_or(f64::INFINITY)
            })
            .collect();
        let limit = (3.0 * robust_sigma(&residuals)).max(0.2);
        use_point = residuals.iter().map(|r| r.abs() <= limit).collect();
        params = problem.solve(params, &use_point);

        for (v, mapping) in mappings.iter_mut().enumerate() {
            *mapping = Homography::from_matrix(problem.views[v].matrix(&params[v * 8..v * 8 + 8]))
                .ok_or("refined mapping is singular")?;
        }
        for &r in &free {
            radii[r] = problem.radius(&params, r);
        }
    }
    let board = dimensions(radii);
    board.validate()?;

    let used: Vec<WirePoint> = points
        .iter()
        .zip(&use_point)
        .filter(|(_, u)| **u)
        .map(|(p, _)| *p)
        .collect();
    let view_fits = (0..views.len())
        .map(|v| {
            let mut ring_sum = [0.0; 6];
            let mut ring_points = [0usize; 6];
            let (mut radial_sum, mut radial_points) = (0.0, 0usize);
            for p in used.iter().filter(|p| p.view == v) {
                let Some(b) = mappings[v].to_board(p.pixel) else {
                    continue;
                };
                match p.wire {
                    Wire::Ring(r) => {
                        ring_sum[r] += (b.radius() - radii[r]).powi(2);
                        ring_points[r] += 1;
                    }
                    Wire::Radial(k) => {
                        let (sin, cos) = radial_degrees(k).to_radians().sin_cos();
                        radial_sum += (b.x * cos - b.y * sin).powi(2);
                        radial_points += 1;
                    }
                }
            }
            let rms = |sum: f64, n: usize| {
                if n == 0 {
                    f64::NAN
                } else {
                    (sum / n as f64).sqrt()
                }
            };
            ViewFit {
                ring_rms_mm: std::array::from_fn(|r| rms(ring_sum[r], ring_points[r])),
                ring_points,
                radial_rms_mm: rms(radial_sum, radial_points),
                radial_points,
            }
        })
        .collect();
    Ok(Refinement {
        board,
        homographies: mappings,
        views: view_fits,
        points: used,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Correspondence;

    /// Renders a grey board with 1.2 mm bright wires at `true_radii`, 2×2
    /// supersampled, seen through `image_to_board`.
    fn render(
        image_to_board: &Homography,
        true_radii: [f64; 6],
        width: usize,
        height: usize,
    ) -> GrayImage {
        let mut data = vec![0u8; width * height];
        let half_wire = 0.6;
        for y in 0..height {
            for x in 0..width {
                let mut total: f64 = 0.0;
                for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let Some(b) = image_to_board
                        .to_board(Pixel::new(x as f64 + sx - 0.5, y as f64 + sy - 0.5))
                    else {
                        continue;
                    };
                    let r = b.radius();
                    let degrees = (90.0 - b.y.atan2(b.x).to_degrees()).rem_euclid(360.0);
                    let on_ring = true_radii.iter().any(|rr| (r - rr).abs() < half_wire);
                    let offset = (degrees - 9.0).rem_euclid(18.0);
                    let on_radial = r > true_radii[1]
                        && r < true_radii[5]
                        && r * offset.min(18.0 - offset).to_radians().sin() < half_wire;
                    let black = black_single(degrees);
                    total += if on_ring || on_radial {
                        200.0
                    } else if r < true_radii[0] {
                        70.0
                    } else if r < true_radii[1] {
                        105.0
                    } else if (true_radii[2]..true_radii[3]).contains(&r)
                        || (true_radii[4]..true_radii[5]).contains(&r)
                    {
                        if black { 70.0 } else { 105.0 }
                    } else if r < true_radii[5] {
                        if black { 20.0 } else { 185.0 }
                    } else {
                        25.0
                    };
                }
                data[y * width + x] = (total / 4.0).round() as u8;
            }
        }
        GrayImage {
            width,
            height,
            data,
        }
    }

    fn mapping(points: &[((f64, f64), (f64, f64))]) -> Homography {
        let correspondences: Vec<_> = points
            .iter()
            .map(|&((ix, iy), (bx, by))| Correspondence {
                image: Pixel::new(ix, iy),
                board: BoardPoint::new(bx, by),
            })
            .collect();
        Homography::fit(&correspondences).unwrap().homography
    }

    #[test]
    fn ridge_finds_sub_sample_peak_and_rejects_flat_profiles() {
        let profile = [20.0, 20.0, 60.0, 150.0, 170.0, 90.0, 70.0, 70.0];
        let at = ridge(&profile, 12.0).unwrap();
        assert!(at > 3.5 && at < 4.0, "{at}");
        assert!(ridge(&[50.0; 8], 12.0).is_none());
        assert!(
            ridge(&[10.0, 20.0, 30.0, 40.0, 50.0, 60.0], 12.0).is_none(),
            "peak at the edge"
        );
    }

    #[test]
    fn measures_radii_that_differ_from_the_starting_model() {
        // About 2 px/mm, mildly oblique.
        let truth = mapping(&[
            ((450.0, 380.0), (0.0, 0.0)),
            ((800.0, 390.0), (180.0, 0.0)),
            ((445.0, 90.0), (0.0, 180.0)),
            ((100.0, 375.0), (-180.0, 0.0)),
            ((455.0, 680.0), (0.0, -180.0)),
            ((710.0, 165.0), (127.0, 127.0)),
        ]);
        let true_radii = [6.0, 16.3, 98.6, 106.6, 161.5, 170.0];
        let image = render(&truth, true_radii, 900, 760);
        // Start 1.5 px off and from the nominal WDF radii.
        let rough = Homography::from_matrix(multiply(
            &truth.matrix(),
            &[[1.0, 0.0, -1.5], [0.0, 1.0, 1.0], [0.0, 0.0, 1.0]],
        ))
        .unwrap();
        let result = refine(
            &[(&image, rough)],
            &BoardDimensions::wdf(),
            &Settings::default(),
        )
        .unwrap();
        for (measured, expected) in result.board.radii().iter().zip(true_radii) {
            assert!(
                (measured - expected).abs() < 0.15,
                "{measured} vs {expected}"
            );
        }
        let probe = Pixel::new(600.0, 300.0);
        let error = result.homographies[0]
            .to_board(probe)
            .unwrap()
            .distance(truth.to_board(probe).unwrap());
        assert!(error < 0.15, "{error}");
        assert!(result.views[0].radial_points > 100);
    }

    #[test]
    fn sector_rules_follow_the_board_colours() {
        assert!(black_single(0.0), "20 is black");
        assert!(!black_single(18.0), "1 is white");
        assert!(near_radial(9.5, 2.0));
        assert!(!near_radial(0.0, 2.0));
    }
}
