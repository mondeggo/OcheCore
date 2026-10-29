//! Board-plane coordinates and scoring geometry.
//!
//! Coordinates are millimetres on the board face with the bull at the origin,
//! +y toward the centre of 20 and +x toward the centre of 6. Ring boundaries are
//! modelled as zero-width circles; real wires have thickness, so callers should
//! treat a small `wire_distance_mm` as ambiguous rather than trusting the label.

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

/// Segment numbers clockwise from the top.
pub const SEGMENTS: [u8; 20] = [
    20, 1, 18, 4, 13, 6, 10, 15, 2, 17, 3, 19, 7, 16, 8, 11, 14, 9, 12, 5,
];

const SEGMENT_DEGREES: f64 = 18.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoardPoint {
    pub x: f64,
    pub y: f64,
}

impl BoardPoint {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn radius(self) -> f64 {
        self.x.hypot(self.y)
    }

    pub fn distance(self, other: Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    /// Point at `radius` along the direction `degrees` clockwise from the top.
    pub fn polar_cw_from_top(radius: f64, degrees: f64) -> Self {
        let angle = (90.0 - degrees).to_radians();
        Self::new(radius * angle.cos(), radius * angle.sin())
    }
}

/// Line `a·x + b·y + c = 0` on the board plane with `(a, b)` of unit length,
/// so [`BoardLine::distance`] is in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoardLine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
}

impl BoardLine {
    /// Normalizes homogeneous coefficients; `None` for a line at infinity.
    pub fn new(a: f64, b: f64, c: f64) -> Option<Self> {
        let norm = a.hypot(b);
        (norm > f64::EPSILON && c.is_finite()).then(|| Self {
            a: a / norm,
            b: b / norm,
            c: c / norm,
        })
    }

    pub fn through(p: BoardPoint, q: BoardPoint) -> Option<Self> {
        Self::new(p.y - q.y, q.x - p.x, p.x * q.y - q.x * p.y)
    }

    /// Signed distance in millimetres.
    pub fn distance(&self, point: BoardPoint) -> f64 {
        self.a * point.x + self.b * point.y + self.c
    }
}

/// Boundary radii in millimetres, from the centre outward.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoardDimensions {
    pub inner_bull: f64,
    pub outer_bull: f64,
    pub treble_inner: f64,
    pub treble_outer: f64,
    pub double_inner: f64,
    pub double_outer: f64,
}

impl BoardDimensions {
    /// Nominal values from the WDF approved-equipment specification
    /// (<https://site.dartswdf.com/rules/approved-by-wdf/>): double wire outer
    /// edge 170.0 mm, treble 107.4 mm, ring inside width 8.0 mm, bull and outer
    /// bull inside diameters 12.7 mm and 31.8 mm. A particular board may differ
    /// and should be measured.
    pub fn wdf() -> Self {
        Self {
            inner_bull: 12.7 / 2.0,
            outer_bull: 31.8 / 2.0,
            treble_inner: 107.4 - 8.0,
            treble_outer: 107.4,
            double_inner: 170.0 - 8.0,
            double_outer: 170.0,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let radii = self.radii();
        if radii[0] <= 0.0 || radii.iter().any(|r| !r.is_finite()) {
            return Err("board radii must be positive and finite".into());
        }
        if radii.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err("board radii must increase from the bull outward".into());
        }
        Ok(())
    }

    pub fn radii(&self) -> [f64; 6] {
        [
            self.inner_bull,
            self.outer_bull,
            self.treble_inner,
            self.treble_outer,
            self.double_inner,
            self.double_outer,
        ]
    }

    pub fn hit(&self, point: BoardPoint) -> Hit {
        let radius = point.radius();
        let ring = if radius <= self.inner_bull {
            Ring::InnerBull
        } else if radius <= self.outer_bull {
            Ring::OuterBull
        } else if radius <= self.treble_inner {
            Ring::InnerSingle
        } else if radius <= self.treble_outer {
            Ring::Treble
        } else if radius <= self.double_inner {
            Ring::OuterSingle
        } else if radius <= self.double_outer {
            Ring::Double
        } else {
            Ring::Miss
        };
        let degrees = clockwise_degrees(point);
        let index = ((degrees + SEGMENT_DEGREES / 2.0) / SEGMENT_DEGREES) as usize % 20;
        let segment = match ring {
            Ring::InnerBull | Ring::OuterBull | Ring::Miss => None,
            _ => Some(SEGMENTS[index]),
        };

        let mut wire_distance = self
            .radii()
            .iter()
            .map(|boundary| (radius - boundary).abs())
            .fold(f64::INFINITY, f64::min);
        if radius > self.outer_bull && radius <= self.double_outer {
            let offset = (degrees + SEGMENT_DEGREES / 2.0).rem_euclid(SEGMENT_DEGREES);
            let to_radial = offset.min(SEGMENT_DEGREES - offset).to_radians();
            wire_distance = wire_distance.min(radius * to_radial.sin());
        }
        Hit {
            ring,
            segment,
            wire_distance_mm: wire_distance,
        }
    }

    /// Calibration anchors where the 20 radial wires cross the treble and double
    /// boundaries, plus the bull centre. `boundary` 0 lies between 20 and 1.
    pub fn anchors(&self) -> Vec<Anchor> {
        let mut anchors = vec![Anchor {
            name: "bull".into(),
            point: BoardPoint::new(0.0, 0.0),
        }];
        for (name, radius) in [
            ("treble_inner", self.treble_inner),
            ("treble_outer", self.treble_outer),
            ("double_inner", self.double_inner),
            ("double_outer", self.double_outer),
        ] {
            for boundary in 0..20 {
                let after = SEGMENTS[(boundary + 1) % 20];
                anchors.push(Anchor {
                    name: format!("{name}:{}/{after}", SEGMENTS[boundary]),
                    point: BoardPoint::polar_cw_from_top(
                        radius,
                        SEGMENT_DEGREES * (boundary as f64 + 0.5),
                    ),
                });
            }
        }
        anchors
    }
}

fn clockwise_degrees(point: BoardPoint) -> f64 {
    (90.0 - point.y.atan2(point.x) * 180.0 / PI).rem_euclid(360.0)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub name: String,
    pub point: BoardPoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ring {
    InnerBull,
    OuterBull,
    InnerSingle,
    Treble,
    OuterSingle,
    Double,
    Miss,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub ring: Ring,
    pub segment: Option<u8>,
    /// Distance to the nearest ring or radial boundary.
    pub wire_distance_mm: f64,
}

impl Hit {
    /// Label in the same notation as [`crate::Throw`]: S20, T20, D20, SB, DB, MISS.
    pub fn label(&self) -> String {
        match (self.ring, self.segment) {
            (Ring::InnerBull, _) => "DB".into(),
            (Ring::OuterBull, _) => "SB".into(),
            (Ring::Treble, Some(segment)) => format!("T{segment}"),
            (Ring::Double, Some(segment)) => format!("D{segment}"),
            (Ring::InnerSingle | Ring::OuterSingle, Some(segment)) => format!("S{segment}"),
            _ => "MISS".into(),
        }
    }

    pub fn score(&self) -> u16 {
        let segment = u16::from(self.segment.unwrap_or(0));
        match self.ring {
            Ring::InnerBull => 50,
            Ring::OuterBull => 25,
            Ring::InnerSingle | Ring::OuterSingle => segment,
            Ring::Treble => segment * 3,
            Ring::Double => segment * 2,
            Ring::Miss => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(radius: f64, degrees: f64) -> Hit {
        BoardDimensions::wdf().hit(BoardPoint::polar_cw_from_top(radius, degrees))
    }

    #[test]
    fn segment_centres_score_in_clockwise_order() {
        for (index, segment) in SEGMENTS.iter().enumerate() {
            let hit = at(140.0, index as f64 * SEGMENT_DEGREES);
            assert_eq!(hit.segment, Some(*segment));
            assert_eq!(hit.label(), format!("S{segment}"));
        }
        assert_eq!(
            BoardDimensions::wdf()
                .hit(BoardPoint::new(0.0, 103.0))
                .label(),
            "T20"
        );
        assert_eq!(
            BoardDimensions::wdf()
                .hit(BoardPoint::new(166.0, 0.0))
                .label(),
            "D6"
        );
        assert_eq!(
            BoardDimensions::wdf()
                .hit(BoardPoint::new(-60.0, 0.0))
                .label(),
            "S11"
        );
        assert_eq!(
            BoardDimensions::wdf()
                .hit(BoardPoint::new(0.0, -103.0))
                .score(),
            9
        );
    }

    #[test]
    fn ring_boundaries_and_bulls_score_correctly() {
        let board = BoardDimensions::wdf();
        assert_eq!(at(0.0, 0.0).label(), "DB");
        assert_eq!(at(board.inner_bull - 0.01, 200.0).score(), 50);
        assert_eq!(at(board.inner_bull + 0.01, 200.0).score(), 25);
        assert_eq!(at(board.outer_bull + 0.01, 0.0).label(), "S20");
        assert_eq!(at(board.treble_inner + 0.01, 0.0).score(), 60);
        assert_eq!(at(board.treble_outer + 0.01, 0.0).label(), "S20");
        assert_eq!(at(board.double_inner + 0.01, 0.0).score(), 40);
        assert_eq!(at(board.double_outer + 0.01, 0.0).label(), "MISS");
        assert_eq!(at(board.double_outer + 0.01, 0.0).score(), 0);
        assert_eq!(at(board.double_outer + 0.01, 0.0).segment, None);
    }

    #[test]
    fn wire_distance_reports_nearby_ring_and_radial_wires() {
        let board = BoardDimensions::wdf();
        assert!((at(board.treble_outer + 0.5, 0.0).wire_distance_mm - 0.5).abs() < 1e-9);
        // 1° from the 20/1 radial wire at 140 mm is about 2.4 mm away.
        let near_radial = at(140.0, 8.0);
        assert_eq!(near_radial.segment, Some(20));
        assert!((near_radial.wire_distance_mm - 140.0 * 1f64.to_radians().sin()).abs() < 1e-9);
        assert!(
            at(1.0, 9.0).wire_distance_mm > 5.0,
            "radial wires stop at the bull"
        );
    }

    #[test]
    fn anchors_lie_on_boundaries_between_named_segments() {
        let board = BoardDimensions::wdf();
        let anchors = board.anchors();
        assert_eq!(anchors.len(), 81);
        let first = anchors
            .iter()
            .find(|a| a.name == "double_outer:20/1")
            .unwrap();
        assert!((first.point.radius() - 170.0).abs() < 1e-9);
        assert!((clockwise_degrees(first.point) - 9.0).abs() < 1e-9);
        let last = anchors
            .iter()
            .find(|a| a.name == "treble_inner:5/20")
            .unwrap();
        assert!((clockwise_degrees(last.point) - 351.0).abs() < 1e-9);
    }

    #[test]
    fn dimensions_must_increase_outward() {
        assert!(BoardDimensions::wdf().validate().is_ok());
        let mut swapped = BoardDimensions::wdf();
        swapped.treble_outer = swapped.double_outer + 1.0;
        assert!(swapped.validate().is_err());
        let mut invalid = BoardDimensions::wdf();
        invalid.inner_bull = f64::NAN;
        assert!(invalid.validate().is_err());
    }
}
