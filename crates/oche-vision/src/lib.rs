//! Impact estimation from per-camera observations on the board plane.
//!
//! A shaft seen by one camera constrains the impact to a board line (see
//! `oche_calibration::Homography::line_to_board`); it does not locate the impact
//! on that line. [`fuse_lines`] intersects lines from distinct cameras by weighted
//! least squares and abstains when the geometry cannot fix a point.
//!
//! Three lines can reveal an inconsistency but cannot attribute it: dropping any
//! one leaves a pair that always intersects exactly. A view is therefore only
//! rejected when at least three consistent views remain; otherwise the fusion
//! abstains. Poor observations should be filtered before fusion by their own
//! feature quality.

use oche_domain::board::{BoardLine, BoardPoint};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LineObservation {
    pub camera: u8,
    pub line: BoardLine,
    /// One-sigma perpendicular uncertainty of the line, in millimetres.
    pub sigma_mm: f64,
}

/// Acceptance limits. There are deliberately no defaults: values must be chosen
/// from labelled recordings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FusionLimits {
    /// Smallest acceptable angle between the two most divergent lines.
    pub min_angle_deg: f64,
    /// Largest acceptable line residual, in units of that line's sigma.
    pub max_residual_sigma: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Impact {
    pub point: BoardPoint,
    /// One-sigma semi-axes of the uncertainty ellipse, largest first.
    pub semi_axes_mm: (f64, f64),
    pub cameras: Vec<u8>,
    pub rejected_cameras: Vec<u8>,
    /// Whether the remaining cameras over-determine the point. With exactly two
    /// lines any intersection fits perfectly, so agreement is unverified.
    pub consistency_checked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Abstention {
    /// Fewer than two usable cameras; one line does not determine an impact.
    TooFewViews { cameras: Vec<u8> },
    /// The lines are too close to parallel to intersect reliably.
    IllConditioned { angle_deg: f64 },
    /// The views disagree and the disagreement cannot be attributed to one view.
    Inconsistent {
        cameras: Vec<u8>,
        worst_residual_sigma: f64,
    },
}

pub fn fuse_lines(
    observations: &[LineObservation],
    limits: FusionLimits,
) -> Result<Impact, Abstention> {
    let mut used: Vec<LineObservation> = observations
        .iter()
        .filter(|o| o.sigma_mm.is_finite() && o.sigma_mm > 0.0)
        .copied()
        .collect();
    let mut rejected = Vec::new();
    loop {
        let cameras: Vec<u8> = used.iter().map(|o| o.camera).collect();
        let distinct = {
            let mut sorted = cameras.clone();
            sorted.sort_unstable();
            sorted.dedup();
            sorted.len()
        };
        if distinct < 2 {
            return Err(Abstention::TooFewViews { cameras });
        }
        let angle_deg = widest_angle_deg(&used);
        if angle_deg < limits.min_angle_deg {
            return Err(Abstention::IllConditioned { angle_deg });
        }
        let (point, covariance) = solve(&used).ok_or(Abstention::IllConditioned { angle_deg })?;
        let (worst, worst_sigma) = used
            .iter()
            .enumerate()
            .map(|(i, o)| (i, o.line.distance(point).abs() / o.sigma_mm))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("at least two observations");
        if used.len() > 2 && worst_sigma > limits.max_residual_sigma {
            if used.len() == 3 {
                return Err(Abstention::Inconsistent {
                    cameras,
                    worst_residual_sigma: worst_sigma,
                });
            }
            rejected.push(used.remove(worst).camera);
            continue;
        }
        return Ok(Impact {
            point,
            semi_axes_mm: semi_axes(covariance),
            cameras,
            rejected_cameras: rejected,
            consistency_checked: used.len() > 2,
        });
    }
}

/// Angle in degrees between the most divergent pair of lines, in [0, 90].
fn widest_angle_deg(lines: &[LineObservation]) -> f64 {
    let mut widest: f64 = 0.0;
    for (i, a) in lines.iter().enumerate() {
        for b in &lines[i + 1..] {
            let sine = (a.line.a * b.line.b - a.line.b * b.line.a).abs().min(1.0);
            widest = widest.max(sine.asin().to_degrees());
        }
    }
    widest
}

/// Weighted least-squares intersection and its covariance `[xx, xy, yy]`.
fn solve(lines: &[LineObservation]) -> Option<(BoardPoint, [f64; 3])> {
    let (mut sxx, mut sxy, mut syy, mut rx, mut ry) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for o in lines {
        let w = 1.0 / (o.sigma_mm * o.sigma_mm);
        let BoardLine { a, b, c } = o.line;
        sxx += w * a * a;
        sxy += w * a * b;
        syy += w * b * b;
        rx -= w * a * c;
        ry -= w * b * c;
    }
    let det = sxx * syy - sxy * sxy;
    if det <= f64::EPSILON * (sxx + syy).powi(2) {
        return None;
    }
    let point = BoardPoint::new((syy * rx - sxy * ry) / det, (sxx * ry - sxy * rx) / det);
    Some((point, [syy / det, -sxy / det, sxx / det]))
}

fn semi_axes([xx, xy, yy]: [f64; 3]) -> (f64, f64) {
    let mean = (xx + yy) / 2.0;
    let spread = (((xx - yy) / 2.0).powi(2) + xy * xy).sqrt();
    ((mean + spread).sqrt(), (mean - spread).max(0.0).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: FusionLimits = FusionLimits {
        min_angle_deg: 20.0,
        max_residual_sigma: 3.0,
    };

    /// Line through `impact` heading `degrees` from the +x axis, shifted sideways.
    fn line(camera: u8, impact: BoardPoint, degrees: f64, offset_mm: f64) -> LineObservation {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let through = BoardPoint::new(impact.x - sin * offset_mm, impact.y + cos * offset_mm);
        let other = BoardPoint::new(through.x + cos * 50.0, through.y + sin * 50.0);
        LineObservation {
            camera,
            line: BoardLine::through(through, other).unwrap(),
            sigma_mm: 1.0,
        }
    }

    #[test]
    fn two_crossing_views_locate_the_impact_without_claiming_consistency() {
        let impact = BoardPoint::new(12.0, 103.0);
        let fused = fuse_lines(
            &[line(1, impact, 10.0, 0.0), line(2, impact, 100.0, 0.0)],
            LIMITS,
        )
        .unwrap();
        assert!(fused.point.distance(impact) < 1e-9);
        assert!(!fused.consistency_checked);
        // Perpendicular unit-sigma lines give a unit-sigma circle.
        assert!((fused.semi_axes_mm.0 - 1.0).abs() < 1e-9);
        assert!((fused.semi_axes_mm.1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn three_agreeing_views_verify_consistency() {
        let impact = BoardPoint::new(-40.0, 60.0);
        let fused = fuse_lines(
            &[
                line(1, impact, 30.0, 0.2),
                line(2, impact, 150.0, -0.2),
                line(3, impact, 270.0, 0.1),
            ],
            LIMITS,
        )
        .unwrap();
        assert!(fused.consistency_checked && fused.rejected_cameras.is_empty());
        assert!(fused.point.distance(impact) < 0.5);
    }

    #[test]
    fn three_views_with_one_outlier_abstain_because_it_cannot_be_attributed() {
        let impact = BoardPoint::new(-40.0, 60.0);
        let result = fuse_lines(
            &[
                line(1, impact, 30.0, 0.0),
                line(2, impact, 150.0, 0.0),
                line(3, impact, 270.0, 15.0),
            ],
            LIMITS,
        );
        assert!(
            matches!(result, Err(Abstention::Inconsistent { ref cameras, .. }) if cameras == &[1, 2, 3])
        );
    }

    #[test]
    fn four_views_reject_the_disagreeing_camera() {
        let impact = BoardPoint::new(-40.0, 60.0);
        let fused = fuse_lines(
            &[
                line(1, impact, 30.0, 0.2),
                line(2, impact, 150.0, -0.2),
                line(3, impact, 270.0, 15.0),
                line(4, impact, 80.0, 0.1),
            ],
            LIMITS,
        )
        .unwrap();
        assert_eq!(fused.rejected_cameras, vec![3]);
        assert_eq!(fused.cameras, vec![1, 2, 4]);
        assert!(fused.consistency_checked);
        assert!(fused.point.distance(impact) < 0.5);
    }

    #[test]
    fn near_parallel_lines_abstain() {
        let impact = BoardPoint::new(0.0, 0.0);
        let result = fuse_lines(
            &[line(1, impact, 40.0, 0.0), line(2, impact, 45.0, 0.0)],
            LIMITS,
        );
        assert!(
            matches!(result, Err(Abstention::IllConditioned { angle_deg }) if (angle_deg - 5.0).abs() < 1e-6)
        );
    }

    #[test]
    fn one_camera_or_one_line_per_camera_cannot_locate_an_impact() {
        let impact = BoardPoint::new(5.0, 5.0);
        assert_eq!(
            fuse_lines(&[line(1, impact, 0.0, 0.0)], LIMITS),
            Err(Abstention::TooFewViews { cameras: vec![1] })
        );
        // Two lines from the same camera are not independent views.
        assert!(matches!(
            fuse_lines(
                &[line(1, impact, 0.0, 0.0), line(1, impact, 90.0, 0.0)],
                LIMITS
            ),
            Err(Abstention::TooFewViews { .. })
        ));
    }
}
