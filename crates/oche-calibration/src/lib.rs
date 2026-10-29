//! Per-camera mapping between image pixels and the board plane.
//!
//! A [`Homography`] maps undistorted image coordinates onto board millimetres
//! (see [`oche_domain::board`]). It is fitted from anchor correspondences with the
//! normalized direct linear transform. Lens distortion is not modelled yet: fit
//! residuals on real frames include it and must be checked before trusting a fit.

pub mod detect;
pub mod refine;

use oche_domain::board::{BoardLine, BoardPoint};
use serde::{Deserialize, Serialize};

/// Image coordinates in pixels, origin at the top-left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pixel {
    pub x: f64,
    pub y: f64,
}

impl Pixel {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Correspondence {
    pub image: Pixel,
    pub board: BoardPoint,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FitError {
    TooFewPoints(usize),
    NonFinite,
    /// The points do not determine a unique mapping, e.g. too many are collinear.
    Degenerate,
    /// Some points map to the far side of the vanishing line, so they cannot all
    /// lie on one visible plane.
    InconsistentSides,
}

impl std::fmt::Display for FitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooFewPoints(count) => write!(f, "need at least 4 anchors, got {count}"),
            Self::NonFinite => f.write_str("anchor coordinates must be finite"),
            Self::Degenerate => f.write_str("anchors are degenerate (too many collinear)"),
            Self::InconsistentSides => f.write_str("anchors do not lie on one visible plane"),
        }
    }
}

impl std::error::Error for FitError {}

type Matrix3 = [[f64; 3]; 3];

/// Image-to-board projective mapping.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Homography {
    to_board: Matrix3,
    to_image: Matrix3,
}

/// Board-plane residuals between mapped image points and their reference points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Residuals {
    pub per_point_mm: Vec<f64>,
    pub rms_mm: f64,
    pub max_mm: f64,
}

impl Residuals {
    fn new(per_point_mm: Vec<f64>) -> Self {
        let count = per_point_mm.len().max(1) as f64;
        let rms_mm = (per_point_mm.iter().map(|r| r * r).sum::<f64>() / count).sqrt();
        let max_mm = per_point_mm.iter().copied().fold(0.0, f64::max);
        Self {
            per_point_mm,
            rms_mm,
            max_mm,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fit {
    pub homography: Homography,
    /// Residuals on the fitted points. Use [`Homography::residuals`] on held-out
    /// anchors for an independent check.
    pub residuals: Residuals,
}

/// Relative eigenvalue below which the second null direction counts as zero.
const DEGENERATE_RATIO: f64 = 1e-10;

impl Homography {
    pub fn fit(points: &[Correspondence]) -> Result<Fit, FitError> {
        if points.len() < 4 {
            return Err(FitError::TooFewPoints(points.len()));
        }
        if points.iter().any(|p| {
            ![p.image.x, p.image.y, p.board.x, p.board.y]
                .iter()
                .all(|v| v.is_finite())
        }) {
            return Err(FitError::NonFinite);
        }
        let image: Vec<_> = points.iter().map(|p| (p.image.x, p.image.y)).collect();
        let board: Vec<_> = points.iter().map(|p| (p.board.x, p.board.y)).collect();
        let (image_norm, image_t) = normalize(&image).ok_or(FitError::Degenerate)?;
        let (board_norm, board_t) = normalize(&board).ok_or(FitError::Degenerate)?;

        // Accumulate AᵀA of the DLT system directly; the solution is its
        // eigenvector with the smallest eigenvalue.
        let mut ata = [[0.0; 9]; 9];
        for (&(x, y), &(u, v)) in image_norm.iter().zip(&board_norm) {
            for row in [
                [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, -u],
                [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, -v],
            ] {
                for i in 0..9 {
                    for j in 0..9 {
                        ata[i][j] += row[i] * row[j];
                    }
                }
            }
        }
        let (values, vectors) = symmetric_eigen(ata);
        let mut order: Vec<usize> = (0..9).collect();
        order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
        if values[order[1]] <= DEGENERATE_RATIO * values[order[8]] {
            return Err(FitError::Degenerate);
        }
        let h = vectors[order[0]];
        let normalized = [[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], h[8]]];
        let board_inverse = invert(&board_t).ok_or(FitError::Degenerate)?;
        let to_board = multiply(&multiply(&board_inverse, &normalized), &image_t);
        let to_image = invert(&to_board).ok_or(FitError::Degenerate)?;

        let sides: Vec<_> = image
            .iter()
            .map(|&(x, y)| apply(&to_board, x, y).2.signum())
            .collect();
        if sides.iter().any(|side| *side != sides[0]) {
            return Err(FitError::InconsistentSides);
        }
        let homography = Self { to_board, to_image };
        let residuals = homography.residuals(points);
        Ok(Fit {
            homography,
            residuals,
        })
    }

    /// Builds a mapping from an image-to-board matrix; `None` if singular.
    pub fn from_matrix(to_board: [[f64; 3]; 3]) -> Option<Self> {
        if to_board.iter().flatten().any(|v| !v.is_finite()) {
            return None;
        }
        Some(Self {
            to_board,
            to_image: invert(&to_board)?,
        })
    }

    pub fn matrix(&self) -> [[f64; 3]; 3] {
        self.to_board
    }

    pub fn to_board(&self, pixel: Pixel) -> Option<BoardPoint> {
        dehomogenize(apply(&self.to_board, pixel.x, pixel.y)).map(|(x, y)| BoardPoint::new(x, y))
    }

    pub fn to_image(&self, point: BoardPoint) -> Option<Pixel> {
        dehomogenize(apply(&self.to_image, point.x, point.y)).map(|(x, y)| Pixel::new(x, y))
    }

    /// Maps the image line through two pixels onto the board plane. For a dart
    /// shaft this is the board line through the impact; it does not locate the
    /// impact on that line.
    pub fn line_to_board(&self, a: Pixel, b: Pixel) -> Option<BoardLine> {
        // Lines transform by the inverse transpose: l' = H⁻ᵀ l.
        let line = cross([a.x, a.y, 1.0], [b.x, b.y, 1.0]);
        let h = &self.to_image;
        let mapped: Vec<f64> = (0..3)
            .map(|j| (0..3).map(|i| h[i][j] * line[i]).sum())
            .collect();
        BoardLine::new(mapped[0], mapped[1], mapped[2])
    }

    /// Board-plane error of each correspondence; unmappable points count as infinite.
    pub fn residuals(&self, points: &[Correspondence]) -> Residuals {
        Residuals::new(
            points
                .iter()
                .map(|p| {
                    self.to_board(p.image)
                        .map_or(f64::INFINITY, |mapped| mapped.distance(p.board))
                })
                .collect(),
        )
    }
}

/// Hartley normalization: centroid at the origin, mean distance √2.
fn normalize(points: &[(f64, f64)]) -> Option<(Vec<(f64, f64)>, Matrix3)> {
    let n = points.len() as f64;
    let cx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let cy = points.iter().map(|p| p.1).sum::<f64>() / n;
    let mean = points
        .iter()
        .map(|p| (p.0 - cx).hypot(p.1 - cy))
        .sum::<f64>()
        / n;
    if mean <= f64::EPSILON {
        return None;
    }
    let s = std::f64::consts::SQRT_2 / mean;
    let scaled = points
        .iter()
        .map(|p| ((p.0 - cx) * s, (p.1 - cy) * s))
        .collect();
    Some((
        scaled,
        [[s, 0.0, -s * cx], [0.0, s, -s * cy], [0.0, 0.0, 1.0]],
    ))
}

fn apply(m: &Matrix3, x: f64, y: f64) -> (f64, f64, f64) {
    (
        m[0][0] * x + m[0][1] * y + m[0][2],
        m[1][0] * x + m[1][1] * y + m[1][2],
        m[2][0] * x + m[2][1] * y + m[2][2],
    )
}

fn dehomogenize((x, y, w): (f64, f64, f64)) -> Option<(f64, f64)> {
    let point = (x / w, y / w);
    (w.abs() > f64::EPSILON && point.0.is_finite() && point.1.is_finite()).then_some(point)
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn multiply(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, value) in row.iter_mut().enumerate() {
            *value = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn invert(m: &Matrix3) -> Option<Matrix3> {
    let rows = [cross(m[1], m[2]), cross(m[2], m[0]), cross(m[0], m[1])];
    let det: f64 = (0..3).map(|i| m[0][i] * rows[0][i]).sum();
    if det.abs()
        <= f64::EPSILON
            * m.iter()
                .flatten()
                .map(|v| v.abs())
                .fold(0.0, f64::max)
                .powi(3)
    {
        return None;
    }
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, value) in row.iter_mut().enumerate() {
            *value = rows[j][i] / det;
        }
    }
    Some(out)
}

/// Cyclic Jacobi eigen decomposition of a symmetric matrix. Returns eigenvalues
/// and the matching unit eigenvectors.
fn symmetric_eigen<const N: usize>(mut a: [[f64; N]; N]) -> ([f64; N], [[f64; N]; N]) {
    let mut v = [[0.0; N]; N];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _sweep in 0..100 {
        let off: f64 = (0..N)
            .flat_map(|i| (0..N).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| a[i][j] * a[i][j])
            .sum();
        let scale: f64 = (0..N).map(|i| a[i][i] * a[i][i]).sum();
        if off <= f64::EPSILON * f64::EPSILON * scale.max(f64::MIN_POSITIVE) {
            break;
        }
        for p in 0..N {
            for q in p + 1..N {
                if a[p][q] == 0.0 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for row in a.iter_mut() {
                    let (akp, akq) = (row[p], row[q]);
                    row[p] = c * akp - s * akq;
                    row[q] = s * akp + c * akq;
                }
                for k in 0..N {
                    let (apk, aqk) = (a[p][k], a[q][k]);
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for row in v.iter_mut() {
                    let (vp, vq) = (row[p], row[q]);
                    row[p] = c * vp - s * vq;
                    row[q] = s * vp + c * vq;
                }
            }
        }
    }
    let mut values = [0.0; N];
    let mut vectors = [[0.0; N]; N];
    for (i, (value, vector)) in values.iter_mut().zip(vectors.iter_mut()).enumerate() {
        *value = a[i][i];
        for (component, row) in vector.iter_mut().zip(&v) {
            *component = row[i];
        }
    }
    (values, vectors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oche_domain::board::BoardDimensions;

    /// A plausible oblique camera: board → image.
    const CAMERA: Matrix3 = [
        [1.6, 0.35, 640.0],
        [-0.08, 0.9, 330.0],
        [0.00002, 0.0011, 1.0],
    ];

    fn project(point: BoardPoint) -> Pixel {
        // Image y grows downward, board y upward.
        let (x, y) = dehomogenize(apply(&CAMERA, point.x, -point.y)).unwrap();
        Pixel::new(x, y)
    }

    fn anchors() -> Vec<Correspondence> {
        BoardDimensions::wdf()
            .anchors()
            .into_iter()
            .map(|anchor| Correspondence {
                image: project(anchor.point),
                board: anchor.point,
            })
            .collect()
    }

    #[test]
    fn exact_anchors_recover_the_mapping_and_its_inverse() {
        let fit = Homography::fit(&anchors()).unwrap();
        assert!(fit.residuals.max_mm < 1e-6, "{:?}", fit.residuals.max_mm);
        let probe = BoardPoint::new(-37.0, 121.5);
        let pixel = project(probe);
        let mapped = fit.homography.to_board(pixel).unwrap();
        assert!(mapped.distance(probe) < 1e-6);
        let back = fit.homography.to_image(probe).unwrap();
        assert!((back.x - pixel.x).hypot(back.y - pixel.y) < 1e-6);
    }

    #[test]
    fn four_points_suffice_and_holdout_points_verify_the_fit() {
        let all = anchors();
        let corners: Vec<_> = [
            "double_outer:20/1",
            "double_outer:6/10",
            "double_outer:3/19",
            "double_outer:11/14",
        ]
        .iter()
        .map(|name| {
            let index = BoardDimensions::wdf()
                .anchors()
                .iter()
                .position(|a| a.name == *name)
                .unwrap();
            all[index]
        })
        .collect();
        let fit = Homography::fit(&corners).unwrap();
        assert!(fit.homography.residuals(&all).max_mm < 1e-6);
    }

    #[test]
    fn noisy_anchors_report_residuals_in_millimetres() {
        let mut points = anchors();
        // Deterministic ±0.7 px pseudo-noise.
        for (i, p) in points.iter_mut().enumerate() {
            p.image.x += if i % 2 == 0 { 0.7 } else { -0.7 };
            p.image.y += if i % 3 == 0 { -0.7 } else { 0.7 };
        }
        let fit = Homography::fit(&points).unwrap();
        assert!(fit.residuals.rms_mm > 0.01 && fit.residuals.rms_mm < 2.0);
        assert_eq!(fit.residuals.per_point_mm.len(), points.len());
    }

    #[test]
    fn mislabelled_anchor_shows_as_a_large_holdout_residual() {
        let mut points = anchors();
        let good = Homography::fit(&points).unwrap().homography;
        points[5].board = points[6].board;
        let worst = good.residuals(&points).max_mm;
        assert!(worst > 10.0);
    }

    #[test]
    fn degenerate_and_invalid_inputs_are_rejected() {
        let all = anchors();
        assert_eq!(
            Homography::fit(&all[..3]).unwrap_err(),
            FitError::TooFewPoints(3)
        );
        let collinear: Vec<_> = (0..6)
            .map(|i| {
                let board = BoardPoint::new(f64::from(i) * 20.0, 0.0);
                Correspondence {
                    image: project(board),
                    board,
                }
            })
            .collect();
        assert_eq!(
            Homography::fit(&collinear).unwrap_err(),
            FitError::Degenerate
        );
        let mut nan = all.clone();
        nan[0].image.x = f64::NAN;
        assert_eq!(Homography::fit(&nan).unwrap_err(), FitError::NonFinite);
    }

    #[test]
    fn shaft_line_maps_to_a_board_line_through_the_impact() {
        let homography = Homography::fit(&anchors()).unwrap().homography;
        let impact = BoardPoint::new(30.0, 95.0);
        // Any image line through the impact pixel, e.g. towards an off-board pixel.
        let tip = project(impact);
        let shaft_end = Pixel::new(tip.x + 15.0, tip.y - 60.0);
        let line = homography.line_to_board(tip, shaft_end).unwrap();
        assert!(line.distance(impact).abs() < 1e-6);
    }

    #[test]
    fn jacobi_matches_a_known_decomposition() {
        let (values, vectors) = symmetric_eigen([[2.0, 1.0], [1.0, 2.0]]);
        let mut sorted = values;
        sorted.sort_by(f64::total_cmp);
        assert!((sorted[0] - 1.0).abs() < 1e-12 && (sorted[1] - 3.0).abs() < 1e-12);
        for (value, vector) in values.iter().zip(vectors) {
            let product = [2.0 * vector[0] + vector[1], vector[0] + 2.0 * vector[1]];
            assert!((product[0] - value * vector[0]).abs() < 1e-12);
            assert!((product[1] - value * vector[1]).abs() < 1e-12);
        }
    }
}
