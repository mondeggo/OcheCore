//! Replays captured Autodarts 2.1.1 sessions through OcheCore's visit state
//! and requires the same `board.state` sequence.
//!
//! The fixtures hold the distinct consecutive states of two recordings Oche
//! made from the installed board on 2026-11-05, with times in seconds from the
//! first state:
//! - `autodarts-2.1.1-board-state.jsonl`: 123 states of
//!   `autodarts-20261105T181739178120Z-48906701.jsonl` (18 visits, 53 darts);
//! - `autodarts-2.1.1-board-state-2.jsonl`: 82 states of
//!   `autodarts-20261105T220437178595Z-9bc003b4.jsonl` (12 takeouts, 31 darts
//!   including outer bulls, bullseyes and misses).
//!
//! The darts' labels and positions are injected as demo throws, so this checks
//! the visit lifecycle and payloads, not detection.
use oche_autodarts::*;
use oche_domain::{Command, Mode, NativeEvent};
use oche_engine::{Engine, storage::Store};
use tokio::sync::broadcast::Receiver;

fn fixture(text: &str) -> Vec<BoardState> {
    text.lines()
        .map(|line| {
            let mut entry: serde_json::Value = serde_json::from_str(line).unwrap();
            serde_json::from_value(entry["data"].take()).unwrap()
        })
        .collect()
}

/// Comparable form: OcheCore names itself as the method, and coordinates
/// pass through millimetres and back.
fn normalized(state: &BoardState) -> BoardState {
    let mut state = state.clone();
    for throw in &mut state.throws {
        throw.method.clear();
        for c in [&mut throw.coords_x, &mut throw.coords_y] {
            *c = c.map(|v| (v * 1e9).round() / 1e9);
        }
    }
    state
}

struct Board {
    _dir: tempfile::TempDir,
    engine: Engine,
    events: Receiver<NativeEvent>,
    profile: Profile,
    sent: Vec<BoardState>,
}

impl Board {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::acquire(&dir.path().join("board.toml")).unwrap();
        let config = store.initialize().unwrap();
        let engine = Engine::new(config, store, Mode::Demo, "127.0.0.1:5180".into());
        let (initial, events) = engine.subscribe();
        let profile = Profile::new(&initial.snapshot);
        Self {
            _dir: dir,
            engine,
            events,
            profile,
            sent: Vec::new(),
        }
    }

    fn command(&mut self, command: Command) {
        self.engine.command(command).unwrap();
        while let Ok(event) = self.events.try_recv() {
            self.sent.extend(self.profile.apply(&event));
        }
    }

    fn throw(&mut self, throw: &BoardThrow) {
        // OcheCore's labels for Autodarts' bull and miss names.
        let label = match (throw.segment_name.as_str(), throw.multiplier) {
            ("25", _) => "SB".into(),
            ("Bull", _) => "DB".into(),
            (_, 0) => "MISS".into(),
            (name, _) => name.to_string(),
        };
        self.command(Command::DemoThrow {
            label,
            position: Some([
                throw.coords_x.unwrap() * 170.0,
                throw.coords_y.unwrap() * 170.0,
            ]),
        });
    }
}

/// Replays `text` and requires the same states. A recording that does not
/// begin with `calibrationFinished` starts on a running board, whose first
/// state the client read as its baseline.
fn replay(text: &str) {
    // Autodarts lists a visit's first dart while still naming the previous
    // event, then renames it `throwDetected` 0.5–40 ms later; OcheCore sends
    // one state.
    let mut previous = 0;
    let expected: Vec<BoardState> = fixture(text)
        .into_iter()
        .filter(|s| {
            let early = s.event != THROW_DETECTED && s.throws.len() > previous;
            previous = s.throws.len();
            !early
        })
        .collect();
    // Once, 2 s after a takeout, Autodarts listed a dart that its automatic
    // reset then cleared, and three more darts followed: a detection it
    // discarded. The replay leaves out that dart and the reset.
    let mut kept: Vec<BoardState> = Vec::new();
    for state in expected {
        let discards = state.event == MANUAL_RESET
            && state.throws.is_empty()
            && kept.last().is_some_and(|s| !s.throws.is_empty());
        if discards {
            while kept.last().is_some_and(|s| s.event == THROW_DETECTED) {
                kept.pop();
            }
            continue;
        }
        kept.push(state);
    }
    let mut expected = kept;
    let mut board = Board::new();
    if expected[0].event != CALIBRATION_FINISHED {
        board.command(Command::Start);
        board.sent.clear();
        expected.remove(0);
    }
    let mut thrown = 0;
    for state in &expected {
        match state.event.as_str() {
            CALIBRATION_FINISHED => board.command(Command::Start),
            // Autodarts resets the visit about 2 s after each takeout; Oche
            // sends no reset.
            MANUAL_RESET => board.command(Command::Reset),
            THROW_DETECTED => {
                for throw in &state.throws[thrown..] {
                    board.throw(throw);
                }
            }
            TAKEOUT_FINISHED => board.command(Command::DemoTakeout { remaining: 0 }),
            _ => {}
        }
        thrown = state.throws.len();
    }
    assert_eq!(board.sent.len(), expected.len());
    for (index, (sent, expected)) in board.sent.iter().zip(&expected).enumerate() {
        assert_eq!(normalized(sent), normalized(expected), "state {index}");
    }
    assert!(
        board
            .sent
            .iter()
            .all(|s| s.throws.iter().all(|t| t.method == "OcheCore"))
    );
}

#[test]
fn the_first_captured_session_replays_to_the_same_states() {
    replay(include_str!("fixtures/autodarts-2.1.1-board-state.jsonl"));
}

#[test]
fn bulls_and_misses_replay_to_the_same_states() {
    replay(include_str!("fixtures/autodarts-2.1.1-board-state-2.jsonl"));
}

#[test]
fn a_partial_takeout_starts_the_takeout_and_keeps_the_visit() {
    let mut board = Board::new();
    board.command(Command::Start);
    for label in ["T20", "S1", "S5"] {
        board.command(Command::DemoThrow {
            label: label.into(),
            position: None,
        });
    }
    board.command(Command::DemoTakeout { remaining: 1 });
    board.command(Command::DemoTakeout { remaining: 0 });
    let steps: Vec<_> = board
        .sent
        .iter()
        .map(|s| (s.event.as_str(), s.status.as_str(), s.throws.len()))
        .collect();
    assert_eq!(
        steps,
        [
            (CALIBRATION_FINISHED, "throw", 0),
            (THROW_DETECTED, "throw", 1),
            (THROW_DETECTED, "throw", 2),
            (THROW_DETECTED, "takeout", 3),
            (TAKEOUT_STARTED, "takeoutInProgress", 3),
            (TAKEOUT_FINISHED, "throw", 0),
        ]
    );
}

#[test]
fn a_stopped_board_is_not_running() {
    let mut board = Board::new();
    assert!(!board.profile.current().running);
    board.command(Command::Start);
    board.command(Command::Stop);
    let last = board.sent.last().unwrap();
    assert!(!last.running);
    assert_eq!(last.event, CALIBRATION_FINISHED);
}

#[test]
fn a_detected_dart_is_sent_where_the_cameras_placed_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::acquire(&dir.path().join("board.toml")).unwrap();
    let config = store.initialize().unwrap();
    let mut engine = Engine::new(config, store, Mode::Live, "127.0.0.1:5180".into());
    let (initial, mut events) = engine.subscribe();
    let mut profile = Profile::new(&initial.snapshot);
    engine.command(Command::Start).unwrap();
    engine.observe(oche_engine::Observation::Baseline);
    engine.observe_with(
        oche_engine::Observation::Arrival(Some(("S20".into(), 20))),
        Some(oche_engine::ArrivalEvidence {
            position: Some([-5.1, 125.8]),
            detection: String::new(),
            before: Vec::new(),
            after: Vec::new(),
        }),
    );
    let mut sent = Vec::new();
    while let Ok(event) = events.try_recv() {
        sent.extend(profile.apply(&event));
    }
    let dart = &sent.last().unwrap().throws[0];
    assert_eq!(dart.bed, "SingleOuter");
    assert!((dart.coords_x.unwrap() * 170.0 + 5.1).abs() < 1e-9);
    assert!((dart.coords_y.unwrap() * 170.0 - 125.8).abs() < 1e-9);
}
