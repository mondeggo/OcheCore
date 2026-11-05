//! Serialized control state, independent of camera capture and HTTP transports.
pub mod corrections;
pub mod live;
pub mod storage;

pub use corrections::ArrivalEvidence;

use oche_domain::*;
use std::{
    collections::VecDeque,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;
use uuid::Uuid;

pub struct Engine {
    config: Config,
    store: storage::Store,
    started: Instant,
    last_activity: Instant,
    session_id: Uuid,
    state: ServiceState,
    scene: SceneState,
    mode: Mode,
    throws: Vec<Throw>,
    /// What the cameras saw for each throw, for correction entries; kept
    /// alongside `throws`, in memory only.
    evidence: Vec<Option<ArrivalEvidence>>,
    /// The label the live detector gave each throw, alongside `throws`;
    /// `None` for darts the accuracy counter leaves out (synthetic,
    /// uncertain, or counted before the last counter reset).
    detected: Vec<Option<String>>,
    accuracy: Accuracy,
    physical_darts: Option<u8>,
    revision: u64,
    sequence: u64,
    listening_on: String,
    startup_server: ServerConfig,
    listener_fixed: bool,
    events: broadcast::Sender<NativeEvent>,
    history: VecDeque<NativeEvent>,
    calibration: Option<CalibrationRecord>,
    cameras: Vec<CameraStatus>,
    motion: bool,
}

/// What the live detector saw, already scored on the measured board.
#[derive(Debug, Clone, PartialEq)]
pub enum Observation {
    /// The first still scene, taken as the empty board.
    Baseline,
    /// A dart arrived; `None` when its position could not be determined.
    Arrival(Option<(String, u16)>),
    /// A dart arrived but the cameras could not place it; the text says why.
    Uncertain(String),
    /// Darts were removed; `empty` when the board is clear again.
    Removal { empty: bool },
    /// A large change that is not darts (lighting, a moved camera).
    Disturbance,
}

/// A dart position from a client: finite, on or near the board.
fn check_position(position: Option<[f64; 2]>) -> Result<(), ApiError> {
    if position.is_some_and(|[x, y]| !(x.is_finite() && y.is_finite() && x.hypot(y) <= 500.0)) {
        return Err(error(
            "invalid_request",
            "position must be x,y in mm within 500 mm of the bull",
        ));
    }
    Ok(())
}

pub fn error(code: &str, message: impl Into<String>) -> ApiError {
    ApiError {
        code: code.into(),
        message: message.into(),
    }
}

impl Engine {
    pub fn new(config: Config, store: storage::Store, mode: Mode, listening_on: String) -> Self {
        let (events, _) = broadcast::channel(HISTORY_LIMIT);
        // An unreadable calibration is reported by `calibration_problem`, not fatal.
        let calibration = store.load_calibration().ok().flatten();
        let accuracy = store
            .load_accuracy(config.board.id)
            .unwrap_or_else(|e| {
                eprintln!("Starting a new accuracy count: {e:#}");
                None
            })
            .unwrap_or_else(Accuracy::starting_now);
        Self {
            startup_server: config.server.clone(),
            config,
            store,
            started: Instant::now(),
            last_activity: Instant::now(),
            session_id: Uuid::new_v4(),
            state: ServiceState::Stopped,
            scene: if mode == Mode::Demo {
                SceneState::Empty
            } else {
                SceneState::Unknown
            },
            mode,
            throws: Vec::new(),
            evidence: Vec::new(),
            detected: Vec::new(),
            accuracy,
            physical_darts: (mode == Mode::Demo).then_some(0),
            revision: 0,
            sequence: 0,
            listening_on,
            listener_fixed: false,
            events,
            history: VecDeque::new(),
            calibration,
            cameras: Vec::new(),
            motion: false,
        }
    }

    /// Makes `server.bind` and `server.port` read only: something outside
    /// OcheCore, such as a container's port mapping, decides where it listens.
    pub fn fix_listener(&mut self) {
        self.listener_fixed = true;
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn state(&self) -> ServiceState {
        self.state
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn calibration(&self) -> Option<&CalibrationRecord> {
        self.calibration.as_ref()
    }

    /// Configured cameras that have a calibration matching their settings.
    fn calibrated_cameras(&self) -> usize {
        self.calibration.as_ref().map_or(0, |record| {
            self.config
                .cameras
                .iter()
                .filter(|c| record.for_camera(c).is_some())
                .count()
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            api_version: 1,
            config: self.config.clone(),
            session_id: self.session_id,
            revision: self.revision,
            sequence: self.sequence,
            uptime_ms: self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            state: self.state,
            scene: self.scene,
            mode: self.mode,
            throws: self.throws.clone(),
            physical_darts: self.physical_darts,
            capabilities: Capabilities {
                capture: self.mode == Mode::Live,
                calibration: self.calibrated_cameras() > 0,
                // Fusing shaft lines needs at least two calibrated views.
                vision: self.mode == Mode::Live && self.calibrated_cameras() >= MIN_SCORING_CAMERAS,
                autodarts_compatibility: false,
                synthetic_input: self.mode == Mode::Demo,
            },
            listening_on: self.listening_on.clone(),
            restart_required: self.config.server != self.startup_server,
            listener_fixed: self.listener_fixed,
            cameras: self.cameras.clone(),
            calibration: self.calibration.clone(),
            motion: self.motion,
            accuracy: self.accuracy.clone(),
        }
    }

    pub fn snapshot_event(&self, kind: &str) -> NativeEvent {
        NativeEvent {
            kind: kind.into(),
            board_id: self.config.board.id,
            session_id: self.session_id,
            sequence: self.sequence,
            unix_time_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u64::MAX as u128) as u64,
            snapshot: self.snapshot(),
        }
    }

    pub fn subscribe(&self) -> (NativeEvent, broadcast::Receiver<NativeEvent>) {
        // Caller holds the command lock: subscription and snapshot are atomic.
        (self.snapshot_event("snapshot"), self.events.subscribe())
    }

    pub fn history(&self) -> Vec<NativeEvent> {
        self.history.iter().cloned().collect()
    }

    fn publish(&mut self, kind: &str) {
        self.revision += 1;
        self.sequence += 1;
        let event = self.snapshot_event(kind);
        if self.history.len() == HISTORY_LIMIT {
            self.history.pop_front();
        }
        self.history.push_back(event.clone());
        let _ = self.events.send(event);
    }

    pub fn command(&mut self, command: Command) -> Result<Snapshot, ApiError> {
        let kind = match command {
            Command::Start | Command::Wake => {
                self.last_activity = Instant::now();
                if self.state == ServiceState::Running {
                    return Ok(self.snapshot());
                }
                self.state = ServiceState::Running;
                "started"
            }
            Command::Stop => {
                if self.state == ServiceState::Stopped {
                    return Ok(self.snapshot());
                }
                self.state = ServiceState::Stopped;
                "stopped"
            }
            Command::Reset => {
                self.clear_throws();
                "visit_reset"
            }
            Command::Calibrate { .. } | Command::OrientBegin | Command::OrientFinish => {
                // The live service runs calibration outside the command lock;
                // reaching the engine means no camera pipeline is available.
                return Err(error(
                    "conflict",
                    if self.mode == Mode::Live {
                        "calibration is unavailable in this service; no calibration was changed"
                    } else {
                        "calibration needs cameras; restart the daemon with `run` (not --demo). No calibration was changed"
                    },
                ));
            }
            Command::CorrectThrow {
                dart,
                label,
                expected_revision,
                expected_session_id,
                position,
            } => {
                check_position(position)?;
                if expected_revision != self.revision || expected_session_id != self.session_id {
                    return Err(error(
                        "conflict",
                        "visit changed while editing; refresh and try again",
                    ));
                }
                let index = dart
                    .checked_sub(1)
                    .filter(|i| *i < 3)
                    .ok_or_else(|| error("invalid_request", "dart must be 1–3"))?
                    as usize;
                let current = self
                    .throws
                    .get(index)
                    .ok_or_else(|| error("conflict", "only an existing throw can be corrected"))?;
                let mut corrected =
                    Throw::synthetic(&label).map_err(|e| error("invalid_request", e))?;
                corrected.synthetic = current.synthetic;
                corrected.corrected = true;
                // A new label without a position no longer matches the old one.
                corrected.position = position;
                // A dart counts as corrected while its label differs from the
                // detector's, so correcting it twice or back changes the count.
                let detected_label = self.detected[index].as_deref();
                let was_wrong = detected_label.is_some_and(|d| d != current.label);
                let now_wrong = detected_label.is_some_and(|d| d != corrected.label);
                let detected = std::mem::replace(&mut self.throws[index], corrected);
                if self.config.debug.save_corrections {
                    self.save_correction(dart, &detected, index);
                }
                if was_wrong != now_wrong {
                    if now_wrong {
                        self.accuracy.corrected += 1;
                    } else {
                        self.accuracy.corrected = self.accuracy.corrected.saturating_sub(1);
                    }
                    self.save_accuracy();
                }
                "throw_corrected"
            }
            Command::ResetAccuracy => {
                self.accuracy = Accuracy::starting_now();
                // Darts already on the board were counted before; leave them out.
                self.detected.fill(None);
                self.save_accuracy();
                "accuracy_reset"
            }
            Command::DemoThrow { label, position } => {
                self.require_demo()?;
                check_position(position)?;
                if self.state != ServiceState::Running {
                    return Err(error(
                        "conflict",
                        "start the service before injecting a demo throw",
                    ));
                }
                if self.throws.len() >= 3 || self.physical_darts.unwrap_or(0) >= 3 {
                    return Err(error(
                        "conflict",
                        "three darts are still on the board (a visit reset keeps them); take them out first",
                    ));
                }
                if self.scene == SceneState::PartialTakeout {
                    return Err(error("conflict", "complete the partial takeout first"));
                }
                let mut dart = Throw::synthetic(&label).map_err(|e| error("invalid_request", e))?;
                dart.position = position;
                self.throws.push(dart);
                self.evidence.push(None);
                self.detected.push(None);
                self.physical_darts = Some(self.physical_darts.unwrap_or(0) + 1);
                self.scene = SceneState::DartsPresent;
                "throw_accepted"
            }
            Command::DemoTakeout { remaining } => {
                self.require_demo()?;
                if self.state != ServiceState::Running {
                    return Err(error(
                        "conflict",
                        "start the service before injecting a demo takeout",
                    ));
                }
                if remaining >= self.physical_darts.unwrap_or(0) {
                    return Err(error(
                        "invalid_request",
                        "remaining darts must be less than the current simulated occupancy",
                    ));
                }
                self.physical_darts = Some(remaining);
                if remaining == 0 {
                    self.clear_throws();
                    self.scene = SceneState::Empty;
                    "takeout_completed"
                } else {
                    self.scene = SceneState::PartialTakeout;
                    "partial_takeout"
                }
            }
        };
        self.last_activity = Instant::now();
        self.publish(kind);
        Ok(self.snapshot())
    }

    fn require_demo(&self) -> Result<(), ApiError> {
        if self.mode != Mode::Demo {
            return Err(error(
                "conflict",
                "synthetic input is disabled; restart the daemon with `run --demo` to enable it",
            ));
        }
        Ok(())
    }

    pub fn configure(&mut self, change: ConfigChange) -> Result<Snapshot, ApiError> {
        if change
            .expected_revision
            .is_some_and(|revision| revision != self.revision)
        {
            return Err(error(
                "conflict",
                "state changed while editing; refresh and try again",
            ));
        }
        if self.listener_fixed && change.key.starts_with("server.") {
            return Err(error(
                "invalid_request",
                "the listen address is set by the container's port mapping (OCHECORE_BIND and OCHECORE_PORT); server.bind and server.port cannot be changed here",
            ));
        }
        let candidate = self
            .config
            .changed(&change)
            .map_err(|e| error("invalid_request", e))?;
        if candidate == self.config {
            return Ok(self.snapshot());
        }
        self.store.save(&candidate).map_err(|e| {
            error(
                "storage_error",
                format!("configuration was not applied: {e:#}"),
            )
        })?;
        self.config = candidate;
        self.publish("config_changed");
        Ok(self.snapshot())
    }

    /// Applies a detector observation to the visit. Ignored unless running live.
    pub fn observe(&mut self, observation: Observation) {
        self.observe_with(observation, None);
    }

    /// As [`Self::observe`], with what the cameras saw for an arrival: the
    /// throw's event already carries its detected position, which clients
    /// that only read events (such as Autodarts ones) need to draw the dart.
    pub fn observe_with(&mut self, observation: Observation, evidence: Option<ArrivalEvidence>) {
        if self.mode != Mode::Live || self.state != ServiceState::Running {
            return;
        }
        let kind = match observation {
            Observation::Baseline => {
                self.scene = SceneState::Empty;
                self.physical_darts = Some(0);
                "board_ready"
            }
            Observation::Arrival(scored) => self.arrival(scored, None, evidence),
            Observation::Uncertain(reason) => self.arrival(None, Some(reason), evidence),
            Observation::Removal { empty: true } => {
                self.last_activity = Instant::now();
                self.clear_throws();
                self.physical_darts = Some(0);
                self.scene = SceneState::Empty;
                "takeout_completed"
            }
            Observation::Removal { empty: false } => {
                self.last_activity = Instant::now();
                self.physical_darts = None;
                self.scene = SceneState::PartialTakeout;
                "partial_takeout"
            }
            Observation::Disturbance => "scene_disturbed",
        };
        self.publish(kind);
    }

    fn arrival(
        &mut self,
        scored: Option<(String, u16)>,
        reason: Option<String>,
        evidence: Option<ArrivalEvidence>,
    ) -> &'static str {
        self.last_activity = Instant::now();
        self.physical_darts = self.physical_darts.map(|n| n.saturating_add(1));
        self.scene = SceneState::DartsPresent;
        // A fourth dart must not overwrite an accepted three-dart visit.
        if self.throws.len() >= 3 {
            return "arrival_ignored";
        }
        let uncertain = scored.is_none();
        let (label, score) = scored.unwrap_or_else(|| ("?".into(), 0));
        self.detected.push((!uncertain).then(|| label.clone()));
        self.throws.push(Throw {
            label,
            score,
            synthetic: false,
            corrected: false,
            uncertain,
            uncertainty: reason,
            position: evidence.as_ref().and_then(|e| e.position),
        });
        self.evidence.push(evidence);
        self.accuracy.darts += 1;
        if uncertain {
            self.accuracy.uncertain += 1;
        }
        self.save_accuracy();
        if uncertain {
            "throw_uncertain"
        } else {
            "throw_accepted"
        }
    }

    fn clear_throws(&mut self) {
        self.throws.clear();
        self.evidence.clear();
        self.detected.clear();
    }

    /// A failure to save is reported; the count in memory stays current.
    fn save_accuracy(&self) {
        if let Err(e) = self
            .store
            .save_accuracy(self.config.board.id, &self.accuracy)
        {
            eprintln!("Could not save the accuracy count: {e:#}");
        }
    }

    /// The folder for diagnostics, beside the configuration file.
    pub fn debug_dir(&self) -> std::path::PathBuf {
        self.store.dir().join("debug")
    }

    /// A failure to save is reported but never undoes the correction.
    fn save_correction(&self, dart: u8, detected: &Throw, index: usize) {
        let snapshot = self.snapshot();
        let entry = corrections::Correction {
            snapshot: &snapshot,
            dart,
            detected,
            corrected: &self.throws[index],
            evidence: self.evidence.get(index).and_then(Option::as_ref),
        };
        let keep = self.config.debug.keep_corrections as usize;
        match corrections::save(&self.debug_dir(), &entry, keep) {
            Ok(path) => eprintln!("Saved the corrected dart to {}", path.display()),
            Err(e) => eprintln!("Could not save the corrected dart: {e}"),
        }
    }

    /// Whether the cameras currently see movement. Not published as an event:
    /// it changes constantly and consumers read it from the snapshot.
    pub fn set_motion(&mut self, motion: bool) {
        self.motion = motion;
    }

    /// Updates live camera status; publishes only when a state or error changes.
    pub fn set_camera_status(&mut self, cameras: Vec<CameraStatus>) {
        let changed = cameras.len() != self.cameras.len()
            || cameras
                .iter()
                .zip(&self.cameras)
                .any(|(a, b)| a.slot != b.slot || a.state != b.state || a.error != b.error);
        self.cameras = cameras;
        if changed {
            self.publish("cameras_changed");
        }
    }

    /// Saves a validated calibration atomically and makes it current.
    pub fn install_calibration(&mut self, record: CalibrationRecord) -> Result<Snapshot, ApiError> {
        self.store
            .save_calibration(&record)
            .map_err(|e| error("storage_error", format!("calibration was not saved: {e:#}")))?;
        self.calibration = Some(record);
        self.last_activity = Instant::now();
        self.publish("calibration_updated");
        Ok(self.snapshot())
    }

    pub fn tick(&mut self, now: Instant) {
        let timeout = self.config.detection.standby_seconds;
        if timeout != 0
            && self.state == ServiceState::Running
            && now.duration_since(self.last_activity).as_secs() >= timeout
        {
            self.state = ServiceState::Standby;
            self.publish("standby");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(mode: Mode) -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        let store = storage::Store::acquire(&dir.path().join("board.toml")).unwrap();
        let config = store.initialize().unwrap();
        let engine = Engine::new(config, store, mode, "127.0.0.1:5180".into());
        (dir, engine)
    }

    #[test]
    fn a_fixed_listener_refuses_address_changes_but_not_others() {
        let (_dir, mut e) = engine(Mode::Demo);
        assert!(!e.snapshot().listener_fixed);
        e.fix_listener();
        assert!(e.snapshot().listener_fixed);
        for (key, value) in [("server.port", "5181"), ("server.bind", "127.0.0.1")] {
            let refused = e
                .configure(ConfigChange {
                    key: key.into(),
                    value: value.into(),
                    expected_revision: None,
                })
                .unwrap_err();
            assert_eq!(refused.code, "invalid_request");
            assert!(refused.message.contains("port mapping"));
        }
        let before = e.snapshot();
        assert!(!before.restart_required);
        let renamed = e
            .configure(ConfigChange {
                key: "board.name".into(),
                value: "Club board".into(),
                expected_revision: None,
            })
            .unwrap();
        assert_eq!(renamed.config.board.name, "Club board");
        assert_eq!(renamed.config.server, before.config.server);
    }

    #[test]
    fn corrections_preserve_occupancy_and_reject_stale_or_empty_slots() {
        let (_dir, mut e) = engine(Mode::Demo);
        e.command(Command::Start).unwrap();
        e.command(Command::DemoThrow {
            label: "T20".into(),
            position: None,
        })
        .unwrap();
        let before = e.snapshot();
        let command = Command::CorrectThrow {
            dart: 1,
            label: "D20".into(),
            expected_revision: before.revision,
            expected_session_id: before.session_id,
            position: None,
        };
        let after = e.command(command.clone()).unwrap();
        assert_eq!(after.throws[0].score, 40);
        assert!(after.throws[0].synthetic && after.throws[0].corrected);
        assert_eq!(after.physical_darts, before.physical_darts);
        assert_eq!(after.scene, before.scene);
        assert_eq!(e.history().last().unwrap().kind, "throw_corrected");
        assert!(e.command(command).is_err());
        for (dart, label, session) in [
            (2, "D20", after.session_id),
            (0, "D20", after.session_id),
            (1, "T21", after.session_id),
            (1, "S1", uuid::Uuid::new_v4()),
        ] {
            assert!(
                e.command(Command::CorrectThrow {
                    dart,
                    label: label.into(),
                    expected_revision: after.revision,
                    expected_session_id: session,
                    position: None,
                })
                .is_err()
            );
            assert_eq!(e.snapshot().revision, after.revision);
        }
        e.command(Command::Reset).unwrap();
        let cleared = e.snapshot();
        assert!(
            e.command(Command::CorrectThrow {
                dart: 1,
                label: "S1".into(),
                expected_revision: cleared.revision,
                expected_session_id: cleared.session_id,
                position: None,
            })
            .is_err()
        );
    }

    #[test]
    fn corrected_darts_are_saved_with_what_the_cameras_saw_when_enabled() {
        let (dir, mut e) = engine(Mode::Live);
        e.command(Command::Start).unwrap();
        e.observe(Observation::Baseline);
        let mut events = e.subscribe().1;
        e.observe_with(
            Observation::Arrival(Some(("S1".into(), 1))),
            Some(ArrivalEvidence {
                position: Some([20.0, 60.0]),
                detection: "arrival; impact S1".into(),
                before: vec![(1, b"before".to_vec())],
                after: vec![(1, b"after".to_vec())],
            }),
        );
        // The throw's own event carries the detected position.
        let event = events.try_recv().unwrap();
        assert_eq!(event.kind, "throw_accepted");
        assert_eq!(event.snapshot.throws[0].position, Some([20.0, 60.0]));
        let correct = |e: &mut Engine, label: &str, position| {
            let s = e.snapshot();
            e.command(Command::CorrectThrow {
                dart: 1,
                label: label.into(),
                expected_revision: s.revision,
                expected_session_id: s.session_id,
                position,
            })
        };
        // Off by default: nothing is written.
        correct(&mut e, "S20", Some([0.0, 60.0])).unwrap();
        assert!(!e.debug_dir().exists());
        assert!(correct(&mut e, "S20", Some([f64::NAN, 0.0])).is_err());
        e.configure(ConfigChange {
            key: "debug.save_corrections".into(),
            value: "true".into(),
            expected_revision: None,
        })
        .unwrap();
        let after = correct(&mut e, "T20", Some([0.0, 103.0])).unwrap();
        assert_eq!(after.throws[0].position, Some([0.0, 103.0]));
        let entries: Vec<_> = std::fs::read_dir(dir.path().join("debug"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        let [entry] = entries.as_slice() else {
            panic!("expected one entry, got {entries:?}");
        };
        assert!(entry.to_string_lossy().ends_with("-dart1-S20-to-T20"));
        assert_eq!(
            std::fs::read(entry.join("before-cam1.jpg")).unwrap(),
            b"before"
        );
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(entry.join("entry.json")).unwrap()).unwrap();
        assert_eq!(json["corrected"]["label"], "T20");
        assert_eq!(json["corrected"]["position_mm"][1], 103.0);
        assert_eq!(json["detection"], "arrival; impact S1");
        // A correction without a position drops the detected one.
        assert_eq!(
            correct(&mut e, "S5", None).unwrap().throws[0].position,
            None
        );
        for (value, kept) in [("all", 0), ("50", 50)] {
            let s = e
                .configure(ConfigChange {
                    key: "debug.keep_corrections".into(),
                    value: value.into(),
                    expected_revision: None,
                })
                .unwrap();
            assert_eq!(s.config.debug.keep_corrections, kept);
        }
        assert!(
            e.configure(ConfigChange {
                key: "debug.keep_corrections".into(),
                value: "7".into(),
                expected_revision: None,
            })
            .is_err()
        );
    }

    #[test]
    fn reset_preserves_physical_occupancy_and_fourth_dart_is_rejected() {
        let (_dir, mut e) = engine(Mode::Demo);
        e.command(Command::Start).unwrap();
        for _ in 0..3 {
            e.command(Command::DemoThrow {
                label: "T20".into(),
                position: None,
            })
            .unwrap();
        }
        let revision = e.snapshot().revision;
        assert!(
            e.command(Command::DemoThrow {
                label: "DB".into(),
                position: None
            })
            .is_err()
        );
        assert_eq!(e.snapshot().revision, revision);
        e.command(Command::Reset).unwrap();
        assert_eq!(e.snapshot().physical_darts, Some(3));
        assert!(e.snapshot().throws.is_empty());
        assert!(
            e.command(Command::DemoThrow {
                label: "DB".into(),
                position: None
            })
            .is_err()
        );
        e.command(Command::DemoTakeout { remaining: 1 }).unwrap();
        assert_eq!(e.snapshot().scene, SceneState::PartialTakeout);
        e.command(Command::DemoTakeout { remaining: 0 }).unwrap();
        assert_eq!(e.snapshot().scene, SceneState::Empty);
    }

    #[test]
    fn early_takeout_and_event_order_are_explicit() {
        let (_dir, mut e) = engine(Mode::Demo);
        e.command(Command::Start).unwrap();
        e.command(Command::DemoThrow {
            label: "S1".into(),
            position: None,
        })
        .unwrap();
        e.command(Command::DemoTakeout { remaining: 0 }).unwrap();
        let events = e.history();
        assert_eq!(
            events.iter().map(|v| v.kind.as_str()).collect::<Vec<_>>(),
            ["started", "throw_accepted", "takeout_completed"]
        );
        assert_eq!(
            events.iter().map(|v| v.sequence).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(e.snapshot().throws.is_empty());
    }

    #[test]
    fn control_only_does_not_claim_observations_or_accept_synthetic_input() {
        let (_dir, mut e) = engine(Mode::ControlOnly);
        assert_eq!(e.snapshot().physical_darts, None);
        assert!(e.command(Command::Calibrate { camera: None }).is_err());
        assert!(
            e.command(Command::DemoThrow {
                label: "T20".into(),
                position: None,
            })
            .is_err()
        );
        assert!(!e.snapshot().capabilities.vision);
    }

    #[test]
    fn live_observations_follow_visit_rules() {
        let (_dir, mut e) = engine(Mode::Live);
        e.observe(Observation::Baseline);
        assert!(e.history().is_empty(), "ignored while stopped");
        e.command(Command::Start).unwrap();
        e.observe(Observation::Baseline);
        for label in ["T20", "S1", "D5"] {
            e.observe(Observation::Arrival(Some((label.into(), 1))));
        }
        e.observe(Observation::Arrival(None));
        let snapshot = e.snapshot();
        assert_eq!(
            snapshot
                .throws
                .iter()
                .map(|t| t.label.as_str())
                .collect::<Vec<_>>(),
            ["T20", "S1", "D5"],
            "a fourth arrival does not overwrite the visit"
        );
        assert_eq!(snapshot.physical_darts, Some(4));
        e.observe(Observation::Removal { empty: false });
        assert_eq!(e.snapshot().scene, SceneState::PartialTakeout);
        assert_eq!(
            e.snapshot().throws.len(),
            3,
            "partial takeout keeps the visit"
        );
        e.observe(Observation::Removal { empty: true });
        let snapshot = e.snapshot();
        assert!(snapshot.throws.is_empty());
        assert_eq!(snapshot.physical_darts, Some(0));
        e.observe(Observation::Arrival(None));
        assert!(e.snapshot().throws[0].uncertain);
        e.observe(Observation::Removal { empty: true });
        e.observe(Observation::Uncertain("only 1 usable dart line".into()));
        let throw = &e.snapshot().throws[0];
        assert!(throw.uncertain);
        assert_eq!(
            throw.uncertainty.as_deref(),
            Some("only 1 usable dart line")
        );
        assert!(!e.snapshot().throws[0].synthetic);
        let kinds: Vec<_> = e.history().iter().map(|v| v.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                "started",
                "board_ready",
                "throw_accepted",
                "throw_accepted",
                "throw_accepted",
                "arrival_ignored",
                "partial_takeout",
                "takeout_completed",
                "throw_uncertain",
                "takeout_completed",
                "throw_uncertain"
            ]
        );
    }

    #[test]
    fn accuracy_counts_detected_darts_corrections_and_survives_restart_until_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.toml");
        let mut e = {
            let store = storage::Store::acquire(&path).unwrap();
            let config = store.initialize().unwrap();
            Engine::new(config, store, Mode::Live, "127.0.0.1:5180".into())
        };
        let correct = |e: &mut Engine, dart: u8, label: &str| {
            let s = e.snapshot();
            e.command(Command::CorrectThrow {
                dart,
                label: label.into(),
                expected_revision: s.revision,
                expected_session_id: s.session_id,
                position: None,
            })
            .unwrap()
            .accuracy
        };
        e.command(Command::Start).unwrap();
        e.observe(Observation::Baseline);
        e.observe(Observation::Arrival(Some(("T20".into(), 60))));
        e.observe(Observation::Arrival(Some(("S1".into(), 1))));
        e.observe(Observation::Uncertain("only 1 usable dart line".into()));
        // A fourth arrival is not a reported dart.
        e.observe(Observation::Arrival(Some(("S5".into(), 5))));
        let a = e.snapshot().accuracy;
        assert_eq!((a.darts, a.corrected, a.uncertain), (3, 0, 1));
        // Confirming the same label is no correction; twice or back is counted once.
        assert_eq!(correct(&mut e, 1, "T20").corrected, 0);
        assert_eq!(correct(&mut e, 2, "S18").corrected, 1);
        assert_eq!(correct(&mut e, 2, "D18").corrected, 1);
        assert_eq!(correct(&mut e, 2, "S1").corrected, 0);
        assert_eq!(correct(&mut e, 2, "S18").corrected, 1);
        // Filling in an uncertain dart leaves it counted as uncertain.
        let a = correct(&mut e, 3, "S5");
        assert_eq!((a.darts, a.corrected, a.uncertain, a.right()), (3, 1, 1, 1));

        // The count survives a restart of the daemon.
        drop(e);
        let mut e = {
            let store = storage::Store::acquire(&path).unwrap();
            let config = store.load_or_create().unwrap();
            Engine::new(config, store, Mode::Live, "127.0.0.1:5180".into())
        };
        assert_eq!(e.snapshot().accuracy, a);

        // Demo darts are synthetic and never count.
        let (_demo_dir, mut demo) = engine(Mode::Demo);
        demo.command(Command::Start).unwrap();
        demo.command(Command::DemoThrow {
            label: "T20".into(),
            position: None,
        })
        .unwrap();
        assert_eq!(correct(&mut demo, 1, "S1").darts, 0);

        // Resetting starts again and leaves darts on the board out of it.
        e.command(Command::Start).unwrap();
        e.observe(Observation::Baseline);
        e.observe(Observation::Arrival(Some(("S20".into(), 20))));
        let reset = e.command(Command::ResetAccuracy).unwrap().accuracy;
        assert_eq!((reset.darts, reset.corrected, reset.uncertain), (0, 0, 0));
        assert!(reset.since_unix_ms >= a.since_unix_ms);
        assert_eq!(correct(&mut e, 1, "S5"), reset);
        assert_eq!(e.history().last().unwrap().kind, "throw_corrected");
        assert!(
            e.history()
                .iter()
                .any(|event| event.kind == "accuracy_reset")
        );
    }

    #[test]
    fn camera_status_publishes_only_state_changes() {
        let (_dir, mut e) = engine(Mode::Live);
        let status = |state, fps| CameraStatus {
            slot: 1,
            state,
            fps,
            error: None,
        };
        e.set_camera_status(vec![status(CameraState::Streaming, 29.9)]);
        e.set_camera_status(vec![status(CameraState::Streaming, 30.0)]);
        e.set_camera_status(vec![status(CameraState::Error, 0.0)]);
        assert_eq!(e.history().len(), 2);
        assert_eq!(e.snapshot().cameras[0].state, CameraState::Error);
    }

    #[test]
    fn polling_does_not_prevent_standby_and_wake_preserves_visit() {
        let (_dir, mut e) = engine(Mode::Demo);
        e.command(Command::Start).unwrap();
        e.command(Command::DemoThrow {
            label: "D20".into(),
            position: None,
        })
        .unwrap();
        for _ in 0..20 {
            e.snapshot();
        }
        e.tick(e.last_activity + std::time::Duration::from_secs(901));
        assert_eq!(e.snapshot().state, ServiceState::Standby);
        e.command(Command::Wake).unwrap();
        assert_eq!(e.snapshot().throws.len(), 1);
    }

    #[test]
    fn history_is_bounded_and_stale_configuration_is_rejected() {
        let (_dir, mut e) = engine(Mode::Demo);
        let (_, mut slow) = e.subscribe();
        for _ in 0..100 {
            e.command(Command::Reset).unwrap();
        }
        assert_eq!(e.history().len(), HISTORY_LIMIT);
        assert!(matches!(
            slow.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
        assert!(
            e.configure(ConfigChange {
                key: "board.name".into(),
                value: "New".into(),
                expected_revision: Some(0)
            })
            .is_err()
        );
    }
}
