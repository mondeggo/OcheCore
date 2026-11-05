//! The local board state of Autodarts 2.0.2, built from native events.
//!
//! The payload and its sequence follow a capture of the installed board's
//! `board.state` feed as Oche received it (2026-11-05, see `docs/discovery.md`):
//! one state per change, with every field present.
//!
//! ```json
//! {"connected":true,"event":"throwDetected","running":true,"status":"throw",
//!  "throws":[{"bed":"Triple","bouncer":false,"coordsX":-0.0098,"coordsY":0.5761,
//!             "failed":false,"method":"UnanimousCam","multiplier":3,
//!             "segmentName":"T20","segmentNumber":20}]}
//! ```
//!
//! A visit reads `throwDetected` with status `throw` for each dart, status
//! `takeout` once three are in, then `takeoutStarted`/`takeoutInProgress` with
//! the darts still listed and `takeoutFinished`/`throw` with none. Coordinates
//! are fractions of the 170 mm double-outer radius, on the native axes.
//!
//! Not observed, so chosen here and marked as such: bulls, misses, darts
//! OcheCore could not place, the `method` value, and states while stopped.
use oche_domain::{
    NativeEvent, ServiceState, Snapshot, Throw,
    board::{BoardDimensions, BoardPoint, SEGMENTS},
};
use serde::{Deserialize, Serialize};

pub const THROW_DETECTED: &str = "throwDetected";
pub const TAKEOUT_STARTED: &str = "takeoutStarted";
pub const TAKEOUT_FINISHED: &str = "takeoutFinished";
pub const MANUAL_RESET: &str = "manualReset";
pub const CALIBRATION_FINISHED: &str = "calibrationFinished";

/// Coordinates are given as fractions of this radius.
const UNIT_MM: f64 = 170.0;

/// Fields in the order Autodarts sends them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardState {
    pub connected: bool,
    pub event: String,
    pub running: bool,
    pub status: String,
    pub throws: Vec<BoardThrow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardThrow {
    pub bed: String,
    pub bouncer: bool,
    /// `None` (null) only for a dart OcheCore could not place.
    pub coords_x: Option<f64>,
    pub coords_y: Option<f64>,
    pub failed: bool,
    pub method: String,
    pub multiplier: u8,
    pub segment_name: String,
    pub segment_number: u8,
}

/// Turns the native event stream into the states an Autodarts client would
/// have received. One instance serves all clients, so polls and the socket agree.
#[derive(Debug, Clone)]
pub struct Profile {
    event: &'static str,
    /// Between `takeoutStarted` and `takeoutFinished`.
    taking_out: bool,
    current: BoardState,
}

impl Profile {
    pub fn new(snapshot: &Snapshot) -> Self {
        let mut profile = Self {
            event: if ready(snapshot) {
                CALIBRATION_FINISHED
            } else {
                ""
            },
            taking_out: false,
            current: BoardState {
                connected: true,
                event: String::new(),
                running: false,
                status: String::new(),
                throws: Vec::new(),
            },
        };
        profile.current = profile.state(snapshot);
        profile
    }

    pub fn current(&self) -> &BoardState {
        &self.current
    }

    /// The states to send for one native event, oldest first; empty when
    /// nothing an Autodarts client sees has changed.
    pub fn apply(&mut self, event: &NativeEvent) -> Vec<BoardState> {
        let mut states = Vec::new();
        match event.kind.as_str() {
            "throw_accepted" | "throw_uncertain" => {
                self.event = THROW_DETECTED;
                self.taking_out = false;
            }
            "partial_takeout" => {
                self.event = TAKEOUT_STARTED;
                self.taking_out = true;
            }
            "takeout_completed" => {
                // Autodarts announced every takeout before finishing it, darts
                // still listed. OcheCore sees a quick takeout only once the
                // board is empty, so both states go out together.
                if !self.taking_out && !self.current.throws.is_empty() {
                    let mut started = self.current.clone();
                    started.event = TAKEOUT_STARTED.into();
                    started.status = "takeoutInProgress".into();
                    self.push(&mut states, started);
                }
                self.event = TAKEOUT_FINISHED;
                self.taking_out = false;
            }
            "visit_reset" => {
                self.event = MANUAL_RESET;
                self.taking_out = false;
            }
            // Autodarts calibrates when it starts; OcheCore keeps its calibration.
            "started" | "calibration_updated" if ready(&event.snapshot) => {
                self.event = CALIBRATION_FINISHED;
            }
            _ => {}
        }
        let next = self.state(&event.snapshot);
        self.push(&mut states, next);
        states
    }

    /// After missed events: the visit from a fresh snapshot, keeping the last
    /// event name.
    pub fn resync(&mut self, snapshot: &Snapshot) -> Vec<BoardState> {
        if snapshot.throws.is_empty() {
            self.taking_out = false;
        }
        let mut states = Vec::new();
        let next = self.state(snapshot);
        self.push(&mut states, next);
        states
    }

    fn push(&mut self, states: &mut Vec<BoardState>, state: BoardState) {
        if state != self.current {
            self.current = state.clone();
            states.push(state);
        }
    }

    fn state(&self, snapshot: &Snapshot) -> BoardState {
        let status = if self.taking_out {
            "takeoutInProgress"
        } else if snapshot.throws.len() >= 3 {
            "takeout"
        } else {
            "throw"
        };
        BoardState {
            connected: true,
            event: self.event.into(),
            running: snapshot.state == ServiceState::Running,
            status: status.into(),
            throws: snapshot.throws.iter().map(board_throw).collect(),
        }
    }
}

/// Darts can be scored: calibrated cameras, or synthetic input in demo mode.
fn ready(snapshot: &Snapshot) -> bool {
    snapshot.capabilities.vision || snapshot.capabilities.synthetic_input
}

pub fn board_throw(throw: &Throw) -> BoardThrow {
    let nominal = BoardDimensions::wdf();
    let label = throw.label.as_str();
    let (kind, number) = match label {
        "SB" => ('B', 25),
        "DB" => ('E', 25),
        "MISS" | "?" => ('M', 0),
        _ => {
            let (prefix, rest) = label.split_at(1);
            (
                prefix.chars().next().unwrap_or('M'),
                rest.parse().unwrap_or(0),
            )
        }
    };
    // Darts without a detected position sit in the middle of their bed, as the
    // browser UI draws them.
    let position = throw.position.or_else(|| {
        let radius = match kind {
            'S' => (nominal.treble_outer + nominal.double_inner) / 2.0,
            'T' => (nominal.treble_inner + nominal.treble_outer) / 2.0,
            'D' => (nominal.double_inner + nominal.double_outer) / 2.0,
            'B' => (nominal.inner_bull + nominal.outer_bull) / 2.0,
            'E' => 0.0,
            _ => return None,
        };
        let index = SEGMENTS.iter().position(|&s| s == number).unwrap_or(0);
        let point = BoardPoint::polar_cw_from_top(radius, 18.0 * index as f64);
        Some([point.x, point.y])
    });
    let (bed, multiplier, name) = match kind {
        'S' => {
            // Singles inside the treble are inner. Any radius inside the
            // treble band separates them, on a nominal or a measured board.
            let middle = (nominal.treble_inner + nominal.treble_outer) / 2.0;
            let inner = position.is_some_and(|[x, y]| x.hypot(y) < middle);
            let bed = if inner { "SingleInner" } else { "SingleOuter" };
            (bed, 1, label.to_string())
        }
        'T' => ("Triple", 3, label.to_string()),
        'D' => ("Double", 2, label.to_string()),
        // Not observed locally: named like Oche's segments.
        'B' => ("SingleBull", 1, "S25".into()),
        'E' => ("DoubleBull", 2, "D25".into()),
        // An Autodarts cloud capture shows a miss as bed `Outside`, multiplier
        // 0 and the segment it landed beside, named `M17`.
        _ => {
            let number = position.map_or(0, |[x, y]| beside(x, y));
            return BoardThrow {
                bed: "Outside".into(),
                bouncer: false,
                coords_x: position.map(|[x, _]| x / UNIT_MM),
                coords_y: position.map(|[_, y]| y / UNIT_MM),
                failed: throw.uncertain,
                method: METHOD.into(),
                multiplier: 0,
                segment_name: format!("M{number}"),
                segment_number: number,
            };
        }
    };
    BoardThrow {
        bed: bed.into(),
        bouncer: false,
        coords_x: position.map(|[x, _]| x / UNIT_MM),
        coords_y: position.map(|[_, y]| y / UNIT_MM),
        failed: false,
        method: METHOD.into(),
        multiplier,
        segment_name: name,
        segment_number: number,
    }
}

/// Autodarts names how its cameras agreed (`UnanimousCam`, `Cam+1`,
/// `BestCam`); OcheCore names itself instead of claiming one of those.
const METHOD: &str = "OcheCore";

/// The segment a point lies beside, by angle.
fn beside(x: f64, y: f64) -> u8 {
    let degrees = (90.0 - y.atan2(x).to_degrees()).rem_euclid(360.0);
    SEGMENTS[((degrees + 9.0) / 18.0) as usize % 20]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn throw(label: &str, position: Option<[f64; 2]>) -> BoardThrow {
        let mut throw = Throw::synthetic(label).unwrap();
        throw.position = position;
        board_throw(&throw)
    }

    #[test]
    fn throws_match_the_observed_fields_and_units() {
        // From the capture: T20 at (-0.0098, 0.5761), method aside.
        let t20 = throw(
            "T20",
            Some([-0.009766811527955475 * 170.0, 0.5761372139248918 * 170.0]),
        );
        let json = serde_json::to_string(&t20).unwrap();
        assert_eq!(
            json,
            r#"{"bed":"Triple","bouncer":false,"coordsX":-0.009766811527955475,"coordsY":0.5761372139248918,"failed":false,"method":"OcheCore","multiplier":3,"segmentName":"T20","segmentNumber":20}"#
        );
        assert_eq!(throw("S20", Some([-1.7, 87.2])).bed, "SingleInner");
        assert_eq!(throw("S20", Some([0.0, 110.0])).bed, "SingleOuter");
        assert_eq!(throw("D20", Some([0.8, 167.8])).bed, "Double");
    }

    #[test]
    fn darts_without_a_position_sit_in_the_middle_of_their_bed() {
        let s6 = throw("S6", None);
        assert_eq!(s6.bed, "SingleOuter");
        assert!((s6.coords_x.unwrap() * 170.0 - 134.7).abs() < 1e-9);
        assert!(s6.coords_y.unwrap().abs() < 1e-9);
        let db = throw("DB", None);
        assert_eq!((db.segment_name.as_str(), db.multiplier), ("D25", 2));
        assert_eq!((db.coords_x, db.coords_y), (Some(0.0), Some(0.0)));
    }

    #[test]
    fn misses_name_the_segment_beside_them_and_unplaced_darts_fail() {
        let miss = throw("MISS", Some([0.0, -190.0]));
        assert_eq!(
            (
                miss.bed.as_str(),
                miss.segment_name.as_str(),
                miss.multiplier,
                miss.failed
            ),
            ("Outside", "M3", 0, false)
        );
        let mut unplaced = Throw::synthetic("MISS").unwrap();
        unplaced.label = "?".into();
        unplaced.uncertain = true;
        let unplaced = board_throw(&unplaced);
        assert!(unplaced.failed);
        assert_eq!((unplaced.coords_x, unplaced.segment_number), (None, 0));
    }
}
