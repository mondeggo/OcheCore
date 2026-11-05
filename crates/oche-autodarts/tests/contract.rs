//! Replays a captured Autodarts 2.1.1 session through OcheCore's visit state
//! and requires the same `board.state` sequence.
//!
//! `fixtures/autodarts-2.1.1-board-state.jsonl` holds the 123 distinct states
//! of `autodarts-20261105T181739178120Z-48906701.jsonl`, which Oche recorded
//! from the installed board on 2026-11-05 (18 visits, 53 darts), with times in
//! seconds from the first state. The darts' labels and positions are injected
//! as demo throws, so this checks the visit lifecycle and payloads, not
//! detection.
use oche_autodarts::*;
use oche_domain::{Command, Mode, NativeEvent};
use oche_engine::{Engine, storage::Store};
use tokio::sync::broadcast::Receiver;

fn fixture() -> Vec<BoardState> {
    include_str!("fixtures/autodarts-2.1.1-board-state.jsonl")
        .lines()
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
        self.command(Command::DemoThrow {
            label: throw.segment_name.clone(),
            position: Some([
                throw.coords_x.unwrap() * 170.0,
                throw.coords_y.unwrap() * 170.0,
            ]),
        });
    }
}

#[test]
fn the_captured_session_replays_to_the_same_states() {
    // Autodarts sometimes lists a new dart while still naming the previous
    // `manualReset`, then renames it a moment later; OcheCore sends one state.
    let expected: Vec<BoardState> = fixture()
        .into_iter()
        .filter(|s| s.event != MANUAL_RESET || s.throws.is_empty())
        .collect();
    let mut board = Board::new();
    let mut thrown = 0;
    for state in &expected {
        match state.event.as_str() {
            CALIBRATION_FINISHED => board.command(Command::Start),
            // Something resets the visit about 2 s after each takeout in the
            // capture; Oche does not, and the cause is not known yet.
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
