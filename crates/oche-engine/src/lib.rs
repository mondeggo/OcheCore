//! Serialized control state, independent of camera capture and HTTP transports.
pub mod storage;

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
    physical_darts: Option<u8>,
    revision: u64,
    sequence: u64,
    listening_on: String,
    startup_server: ServerConfig,
    events: broadcast::Sender<NativeEvent>,
    history: VecDeque<NativeEvent>,
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
            physical_darts: (mode == Mode::Demo).then_some(0),
            revision: 0,
            sequence: 0,
            listening_on,
            events,
            history: VecDeque::new(),
        }
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
                capture: false,
                calibration: false,
                vision: false,
                autodarts_compatibility: false,
                synthetic_input: self.mode == Mode::Demo,
            },
            listening_on: self.listening_on.clone(),
            restart_required: self.config.server != self.startup_server,
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
                self.throws.clear();
                "visit_reset"
            }
            Command::Calibrate { .. } => {
                return Err(error(
                    "not_implemented",
                    "Calibration requires the future camera and calibration backends. No calibration was changed.",
                ));
            }
            Command::CorrectThrow {
                dart,
                label,
                expected_revision,
                expected_session_id,
            } => {
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
                self.throws[index] = corrected;
                "throw_corrected"
            }
            Command::DemoThrow { label } => {
                self.require_demo()?;
                if self.state != ServiceState::Running {
                    return Err(error(
                        "conflict",
                        "start the service before injecting a demo throw",
                    ));
                }
                if self.throws.len() >= 3 || self.physical_darts.unwrap_or(0) >= 3 {
                    return Err(error(
                        "conflict",
                        "three darts are already present; complete takeout before another throw",
                    ));
                }
                if self.scene == SceneState::PartialTakeout {
                    return Err(error("conflict", "complete the partial takeout first"));
                }
                let dart = Throw::synthetic(&label).map_err(|e| error("invalid_request", e))?;
                self.throws.push(dart);
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
                    self.throws.clear();
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
    fn corrections_preserve_occupancy_and_reject_stale_or_empty_slots() {
        let (_dir, mut e) = engine(Mode::Demo);
        e.command(Command::Start).unwrap();
        e.command(Command::DemoThrow {
            label: "T20".into(),
        })
        .unwrap();
        let before = e.snapshot();
        let command = Command::CorrectThrow {
            dart: 1,
            label: "D20".into(),
            expected_revision: before.revision,
            expected_session_id: before.session_id,
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
                    expected_session_id: session
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
                expected_session_id: cleared.session_id
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
            })
            .unwrap();
        }
        let revision = e.snapshot().revision;
        assert!(
            e.command(Command::DemoThrow { label: "DB".into() })
                .is_err()
        );
        assert_eq!(e.snapshot().revision, revision);
        e.command(Command::Reset).unwrap();
        assert_eq!(e.snapshot().physical_darts, Some(3));
        assert!(e.snapshot().throws.is_empty());
        assert!(
            e.command(Command::DemoThrow { label: "DB".into() })
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
        e.command(Command::DemoThrow { label: "S1".into() })
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
                label: "T20".into()
            })
            .is_err()
        );
        assert!(!e.snapshot().capabilities.vision);
    }

    #[test]
    fn polling_does_not_prevent_standby_and_wake_preserves_visit() {
        let (_dir, mut e) = engine(Mode::Demo);
        e.command(Command::Start).unwrap();
        e.command(Command::DemoThrow {
            label: "D20".into(),
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
