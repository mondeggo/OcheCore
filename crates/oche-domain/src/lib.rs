//! Native protocol types. Autodarts compatibility will live in a separate adapter.
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, net::IpAddr};
use uuid::Uuid;

pub mod board;

pub const CONFIG_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 5180;
pub const HISTORY_LIMIT: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub board: Board,
    pub server: ServerConfig,
    pub detection: DetectionConfig,
    pub cameras: Vec<CameraConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Board {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub bind: IpAddr,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DetectionConfig {
    pub speed: Speed,
    /// Zero disables standby. Status polling does not count as activity.
    pub standby_seconds: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Speed {
    VeryLow,
    Low,
    Default,
    High,
    VeryHigh,
}

impl Speed {
    pub const ALL: [Self; 5] = [
        Self::VeryLow,
        Self::Low,
        Self::Default,
        Self::High,
        Self::VeryHigh,
    ];

    /// Frames analysed per second per camera, capped by the camera's FPS.
    /// Decoding one 1280x720 frame took about 17 ms on a Raspberry Pi 4 core
    /// (measured 2026-10-30), so the default of 10 per camera costs roughly
    /// half a core with three cameras.
    pub fn analysis_hz(self) -> u32 {
        match self {
            Self::VeryLow => 3,
            Self::Low => 5,
            Self::Default => 10,
            Self::High => 15,
            Self::VeryHigh => 30,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::VeryLow => "very_low",
            Self::Low => "low",
            Self::Default => "default",
            Self::High => "high",
            Self::VeryHigh => "very_high",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CameraConfig {
    pub slot: u8,
    /// A requested persistent device path, not a verified capture assignment.
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub rotation: u16,
    /// Manual exposure in units of 100 µs; `None` leaves auto exposure on.
    /// Detection compares frames, so a fixed exposure avoids false changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure: Option<u32>,
    /// Pixel near the 20 segment, which orients the first calibration. Later
    /// calibrations reuse the previous orientation when this is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<[f64; 2]>,
}

impl CameraConfig {
    pub fn new(slot: u8) -> Self {
        Self {
            slot,
            device: String::new(),
            width: 1280,
            height: 720,
            fps: 30,
            rotation: 0,
            exposure: None,
            hint: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_VERSION,
            board: Board {
                id: Uuid::new_v4(),
                name: "My dartboard".into(),
            },
            server: ServerConfig {
                bind: IpAddr::from([127, 0, 0, 1]),
                port: DEFAULT_PORT,
            },
            detection: DetectionConfig {
                speed: Speed::Default,
                standby_seconds: 900,
            },
            cameras: vec![CameraConfig::new(1)],
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != CONFIG_VERSION {
            return Err(format!(
                "unsupported configuration schema {}; expected {CONFIG_VERSION}",
                self.schema_version
            ));
        }
        if self.board.id.is_nil() {
            return Err("board.id must not be nil".into());
        }
        if self.board.name.trim().is_empty()
            || self.board.name.chars().count() > 80
            || self.board.name.chars().any(char::is_control)
        {
            return Err("board.name must contain 1–80 printable characters".into());
        }
        if self.server.port == 0 {
            return Err("server.port must be between 1 and 65535".into());
        }
        if ![0, 300, 600, 900, 1800, 3600].contains(&self.detection.standby_seconds) {
            return Err("detection.standby_seconds must be 0, 300, 600, 900, 1800, or 3600".into());
        }
        if !(1..=4).contains(&self.cameras.len()) {
            return Err("configure between one and four camera slots".into());
        }
        let mut devices = HashSet::new();
        for (index, camera) in self.cameras.iter().enumerate() {
            if camera.slot as usize != index + 1 {
                return Err("camera slots must be contiguous and ordered from 1".into());
            }
            if camera.device.len() > 512 || camera.device.chars().any(char::is_control) {
                return Err("camera device must be a printable path of at most 512 bytes".into());
            }
            if !camera.device.is_empty() && !devices.insert(&camera.device) {
                return Err("the same device path cannot be assigned to multiple slots".into());
            }
            if camera.width == 0
                || camera.height == 0
                || camera.width > 8192
                || camera.height > 8192
                || !(1..=240).contains(&camera.fps)
            {
                return Err(
                    "camera dimensions must be 1–8192 and FPS 1–240 (requested modes only)".into(),
                );
            }
            if ![0, 90, 180, 270].contains(&camera.rotation) {
                return Err("camera rotation must be 0, 90, 180, or 270".into());
            }
            if camera.exposure.is_some_and(|e| !(2..=10_000).contains(&e)) {
                return Err("camera exposure must be auto or 2–10000 (100 µs units)".into());
            }
            // Not bounded by the resolution: changing the resolution must not make
            // an otherwise valid configuration unsavable.
            if camera
                .hint
                .is_some_and(|[x, y]| !(x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0))
            {
                return Err("camera hint must be a pixel position x,y".into());
            }
        }
        Ok(())
    }

    /// Build a validated candidate; never mutate the live configuration on error.
    pub fn changed(&self, change: &ConfigChange) -> Result<Self, String> {
        let mut next = self.clone();
        let value = change.value.as_str();
        match change.key.as_str() {
            "board.name" => next.board.name = value.trim().into(),
            "server.bind" => {
                next.server.bind = value.parse().map_err(|_| "invalid IP listen address")?
            }
            "server.port" => next.server.port = value.parse().map_err(|_| "invalid port")?,
            "detection.speed" => {
                next.detection.speed = Speed::ALL
                    .into_iter()
                    .find(|s| s.as_str() == value)
                    .ok_or("unknown speed preset")?
            }
            "detection.standby_seconds" => {
                next.detection.standby_seconds =
                    value.parse().map_err(|_| "invalid standby timeout")?
            }
            "cameras.count" => {
                let count: usize = value.parse().map_err(|_| "invalid camera count")?;
                if !(1..=4).contains(&count) {
                    return Err("camera count must be 1–4".into());
                }
                next.cameras.resize_with(count, || CameraConfig::new(1));
                for (index, camera) in next.cameras.iter_mut().enumerate() {
                    camera.slot = index as u8 + 1;
                }
            }
            key if key.starts_with("cameras.") => {
                let parts: Vec<_> = key.split('.').collect();
                if parts.len() != 3 {
                    return Err("use cameras.<slot>.<field>".into());
                }
                let slot: usize = parts[1].parse().map_err(|_| "invalid camera slot")?;
                let camera = slot
                    .checked_sub(1)
                    .and_then(|i| next.cameras.get_mut(i))
                    .ok_or("camera slot is not configured")?;
                match parts[2] {
                    "device" => camera.device = value.into(),
                    "width" => camera.width = value.parse().map_err(|_| "invalid width")?,
                    "height" => camera.height = value.parse().map_err(|_| "invalid height")?,
                    "resolution" => {
                        let (width, height) = value.split_once('x').ok_or("use WIDTHxHEIGHT")?;
                        camera.width = width.parse().map_err(|_| "invalid width")?;
                        camera.height = height.parse().map_err(|_| "invalid height")?;
                    }
                    "fps" => camera.fps = value.parse().map_err(|_| "invalid FPS")?,
                    "rotation" => {
                        camera.rotation = value.parse().map_err(|_| "invalid rotation")?
                    }
                    "exposure" => {
                        camera.exposure = match value.trim() {
                            "" | "auto" => None,
                            v => Some(v.parse().map_err(|_| "use auto or a number")?),
                        }
                    }
                    "hint" => {
                        camera.hint = match value.trim() {
                            "" | "none" => None,
                            v => {
                                let (x, y) = v.split_once(',').ok_or("use x,y in pixels")?;
                                Some([
                                    x.trim().parse().map_err(|_| "invalid hint x")?,
                                    y.trim().parse().map_err(|_| "invalid hint y")?,
                                ])
                            }
                        }
                    }
                    _ => return Err("unknown camera field".into()),
                }
            }
            _ => return Err("unknown or read-only configuration key".into()),
        }
        next.validate()?;
        Ok(next)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigChange {
    pub key: String,
    pub value: String,
    /// Optional optimistic concurrency check for interactive editors.
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Stopped,
    Running,
    Standby,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Controls only; cameras are never opened.
    ControlOnly,
    Demo,
    /// Cameras are captured while running and darts are detected when calibrated.
    Live,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SceneState {
    Unknown,
    Empty,
    DartsPresent,
    PartialTakeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Throw {
    pub label: String,
    pub score: u16,
    pub synthetic: bool,
    #[serde(default)]
    pub corrected: bool,
    /// A dart was seen but its position could not be determined reliably;
    /// it scores 0 until corrected.
    #[serde(default)]
    pub uncertain: bool,
}

impl Throw {
    pub fn synthetic(label: &str) -> Result<Self, String> {
        let label = label.trim().to_uppercase();
        let score = match label.as_str() {
            "MISS" => 0,
            "SB" => 25,
            "DB" => 50,
            _ => {
                let (prefix, rest) = label
                    .split_at_checked(1)
                    .ok_or("use S1–S20, D1–D20, T1–T20, SB, DB, or MISS")?;
                let multiplier = match prefix {
                    "S" => 1,
                    "D" => 2,
                    "T" => 3,
                    _ => return Err("invalid dart label".into()),
                };
                let segment: u16 = rest.parse().map_err(|_| "invalid dart segment")?;
                if !(1..=20).contains(&segment) {
                    return Err("dart segment must be 1–20".into());
                }
                segment * multiplier
            }
        };
        Ok(Self {
            label,
            score,
            synthetic: true,
            corrected: false,
            uncertain: false,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Start,
    Stop,
    Reset,
    Wake,
    DemoThrow {
        label: String,
    },
    DemoTakeout {
        remaining: u8,
    },
    Calibrate {
        camera: Option<u8>,
    },
    /// Remember the empty board before a dart is thrown at the 20.
    OrientBegin,
    /// Find that dart in every camera and store each camera's 20 hint.
    OrientFinish,
    CorrectThrow {
        dart: u8,
        label: String,
        expected_revision: u64,
        expected_session_id: Uuid,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CameraInventory {
    pub devices: Vec<CameraDevice>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraDevice {
    pub path: String,
    pub aliases: Vec<String>,
    pub name: String,
    pub modes: Vec<CameraMode>,
    pub error: Option<String>,
}

impl CameraDevice {
    pub fn matches(&self, path: &str) -> bool {
        self.path == path || self.aliases.iter().any(|alias| alias == path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraMode {
    pub width: u32,
    pub height: u32,
    pub pixel_format: String,
    pub fps: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub capture: bool,
    pub calibration: bool,
    pub vision: bool,
    pub autodarts_compatibility: bool,
    pub synthetic_input: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub api_version: u32,
    pub config: Config,
    pub session_id: Uuid,
    pub revision: u64,
    pub sequence: u64,
    pub uptime_ms: u64,
    pub state: ServiceState,
    pub scene: SceneState,
    pub mode: Mode,
    pub throws: Vec<Throw>,
    /// Unknown until a capture backend can observe the board; simulated in demo.
    pub physical_darts: Option<u8>,
    pub capabilities: Capabilities,
    pub listening_on: String,
    pub restart_required: bool,
    #[serde(default)]
    pub cameras: Vec<CameraStatus>,
    #[serde(default)]
    pub calibration: Option<CalibrationRecord>,
    /// The cameras see movement: a hand, a thrown dart, or a scene still settling.
    #[serde(default)]
    pub motion: bool,
}

/// How a consumer should present a [`BoardActivity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityLevel {
    /// Nothing is running, by choice.
    Idle,
    /// Working or waiting for the scene to settle.
    Busy,
    /// Ready to score.
    Ready,
    /// Something needs the user's attention.
    Attention,
    /// Something is wrong.
    Problem,
}

/// What the board is doing, as a short label and an explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardActivity {
    pub level: ActivityLevel,
    pub label: &'static str,
    pub detail: String,
}

impl Snapshot {
    /// Camera slots that have a device but no way to find the 20: no stored
    /// hint and no calibration made with the current settings.
    pub fn cameras_needing_orientation(&self) -> Vec<u8> {
        self.config
            .cameras
            .iter()
            .filter(|c| !c.device.is_empty() && c.hint.is_none())
            .filter(|c| {
                self.calibration
                    .as_ref()
                    .is_none_or(|record| record.for_camera(c).is_none())
            })
            .map(|c| c.slot)
            .collect()
    }

    /// The board's state in the terms a player cares about, derived from the
    /// service state, camera status, calibration and scene.
    pub fn activity(&self) -> BoardActivity {
        let make = |level, label, detail: &str| BoardActivity {
            level,
            label,
            detail: detail.into(),
        };
        match self.mode {
            Mode::ControlOnly => {
                return make(
                    ActivityLevel::Idle,
                    "CONTROLS ONLY",
                    "cameras are not used in this mode",
                );
            }
            Mode::Demo => {}
            Mode::Live => {}
        }
        match self.state {
            ServiceState::Stopped => {
                return make(
                    ActivityLevel::Idle,
                    "STOPPED",
                    "press Start to open the cameras",
                );
            }
            ServiceState::Standby => {
                return make(
                    ActivityLevel::Idle,
                    "STANDBY",
                    "cameras are closed; press Wake",
                );
            }
            ServiceState::Running => {}
        }
        if self.mode == Mode::Live {
            let assigned: Vec<&CameraConfig> = self
                .config
                .cameras
                .iter()
                .filter(|c| !c.device.is_empty())
                .collect();
            if assigned.is_empty() {
                return make(
                    ActivityLevel::Attention,
                    "NEEDS SETUP",
                    "assign a device to each camera",
                );
            }
            for camera in &assigned {
                let status = self.cameras.iter().find(|s| s.slot == camera.slot);
                if let Some(status) = status.filter(|s| s.state == CameraState::Error) {
                    return BoardActivity {
                        level: ActivityLevel::Problem,
                        label: "ERROR",
                        detail: format!(
                            "camera {}: {}",
                            camera.slot,
                            status.error.as_deref().unwrap_or("unknown error")
                        ),
                    };
                }
            }
            for camera in &assigned {
                let streaming = self
                    .cameras
                    .iter()
                    .any(|s| s.slot == camera.slot && s.state == CameraState::Streaming);
                if !streaming {
                    return BoardActivity {
                        level: ActivityLevel::Busy,
                        label: "STARTING",
                        detail: format!("waiting for camera {}", camera.slot),
                    };
                }
            }
            if !self.cameras_needing_orientation().is_empty() {
                return make(
                    ActivityLevel::Attention,
                    "NEEDS SETUP",
                    "press Calibrate all; you will be asked to throw one dart at the 20",
                );
            }
            if !self.capabilities.calibration {
                return make(
                    ActivityLevel::Attention,
                    "NEEDS CALIBRATION",
                    "empty the board and press Calibrate all",
                );
            }
            if !self.capabilities.vision {
                return make(
                    ActivityLevel::Attention,
                    "NEEDS CALIBRATION",
                    "at least two calibrated cameras are needed to score",
                );
            }
            if self.motion {
                return make(
                    ActivityLevel::Busy,
                    "MOTION",
                    "waiting for the scene to settle",
                );
            }
        }
        let on_board = self
            .physical_darts
            .map_or(String::new(), |n| format!("{n} on the board"));
        match self.scene {
            SceneState::PartialTakeout => make(
                ActivityLevel::Busy,
                "TAKING OUT",
                "darts are being removed; finish clearing the board",
            ),
            SceneState::DartsPresent if self.throws.len() >= 3 => make(
                ActivityLevel::Attention,
                "TAKE OUT DARTS",
                "the visit is complete; remove the darts",
            ),
            SceneState::DartsPresent => BoardActivity {
                level: ActivityLevel::Ready,
                label: "READY",
                detail: on_board,
            },
            SceneState::Empty => make(ActivityLevel::Ready, "READY", "board empty"),
            SceneState::Unknown => make(
                ActivityLevel::Busy,
                "STARTING",
                "waiting for the first still scene",
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CameraState {
    Off,
    Starting,
    Streaming,
    Error,
}

/// Live state of one camera slot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraStatus {
    pub slot: u8,
    pub state: CameraState,
    /// Delivered frames per second over the last few seconds.
    pub fps: f64,
    pub error: Option<String>,
}

pub const CALIBRATION_VERSION: u32 = 1;

/// Measured board and per-camera mappings, bound to the camera settings
/// they were made with.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CalibrationRecord {
    pub schema_version: u32,
    pub created_unix_ms: u64,
    /// Wire-centre radii in millimetres.
    pub board: board::BoardDimensions,
    pub cameras: Vec<CameraCalibration>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CameraCalibration {
    pub slot: u8,
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub rotation: u16,
    /// Image-to-board homography (pixels to millimetres).
    pub to_board: [[f64; 3]; 3],
    /// RMS distance of detected wire points from the fitted board, in mm.
    pub wire_rms_mm: f64,
    pub wire_points: usize,
}

impl CalibrationRecord {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != CALIBRATION_VERSION {
            return Err(format!(
                "unsupported calibration schema {}; expected {CALIBRATION_VERSION}",
                self.schema_version
            ));
        }
        self.board.validate()?;
        if self.cameras.is_empty() || self.cameras.len() > 4 {
            return Err("calibration must cover one to four cameras".into());
        }
        if self.cameras.iter().any(|c| {
            c.to_board.iter().flatten().any(|v| !v.is_finite()) || !c.wire_rms_mm.is_finite()
        }) {
            return Err("calibration contains non-finite values".into());
        }
        Ok(())
    }

    /// The calibration for a slot, if it still matches that camera's settings.
    pub fn for_camera(&self, camera: &CameraConfig) -> Option<&CameraCalibration> {
        self.cameras.iter().find(|c| {
            c.slot == camera.slot
                && c.device == camera.device
                && c.width == camera.width
                && c.height == camera.height
                && c.rotation == camera.rotation
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeEvent {
    pub kind: String,
    pub board_id: Uuid,
    pub session_id: Uuid,
    pub sequence: u64,
    pub unix_time_ms: u64,
    pub snapshot: Snapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    fn live_snapshot() -> Snapshot {
        let mut config = Config {
            cameras: (1..=2).map(CameraConfig::new).collect(),
            ..Config::default()
        };
        for camera in &mut config.cameras {
            camera.device = format!("/dev/video{}", camera.slot);
            camera.hint = Some([10.0, 10.0]);
        }
        Snapshot {
            api_version: 1,
            config,
            session_id: Uuid::nil(),
            revision: 1,
            sequence: 0,
            uptime_ms: 0,
            state: ServiceState::Running,
            scene: SceneState::Empty,
            mode: Mode::Live,
            throws: vec![],
            physical_darts: Some(0),
            capabilities: Capabilities {
                capture: true,
                calibration: true,
                vision: true,
                autodarts_compatibility: false,
                synthetic_input: false,
            },
            listening_on: String::new(),
            restart_required: false,
            cameras: (1..=2)
                .map(|slot| CameraStatus {
                    slot,
                    state: CameraState::Streaming,
                    fps: 30.0,
                    error: None,
                })
                .collect(),
            calibration: None,
            motion: false,
        }
    }

    #[test]
    fn activity_follows_the_board_from_stopped_to_ready_and_back() {
        let mut s = live_snapshot();
        assert_eq!(s.activity().label, "READY");
        s.motion = true;
        assert_eq!(s.activity().label, "MOTION");
        s.motion = false;
        s.scene = SceneState::DartsPresent;
        s.throws = vec![Throw::synthetic("S20").unwrap(); 3];
        assert_eq!(s.activity().label, "TAKE OUT DARTS");
        s.scene = SceneState::PartialTakeout;
        assert_eq!(s.activity().label, "TAKING OUT");
        s.scene = SceneState::Empty;
        s.cameras[1].state = CameraState::Error;
        s.cameras[1].error = Some("device missing".into());
        let activity = s.activity();
        assert_eq!(
            (activity.label, activity.level),
            ("ERROR", ActivityLevel::Problem)
        );
        assert!(activity.detail.contains("camera 2") && activity.detail.contains("device missing"));
        s.cameras[1].state = CameraState::Starting;
        assert_eq!(s.activity().label, "STARTING");
        s.state = ServiceState::Standby;
        assert_eq!(s.activity().label, "STANDBY");
        s.state = ServiceState::Stopped;
        assert_eq!(s.activity().label, "STOPPED");
    }

    #[test]
    fn missing_setup_is_reported_before_the_board_claims_to_be_ready() {
        let mut s = live_snapshot();
        s.config.cameras[0].hint = None;
        assert_eq!(s.cameras_needing_orientation(), [1]);
        assert_eq!(s.activity().label, "NEEDS SETUP");
        s.config.cameras[0].hint = Some([1.0, 1.0]);
        s.capabilities.calibration = false;
        assert_eq!(s.activity().label, "NEEDS CALIBRATION");
        s.config.cameras[0].device.clear();
        s.config.cameras[1].device.clear();
        assert_eq!(s.activity().label, "NEEDS SETUP");
    }
    use super::*;

    #[test]
    fn config_edits_preserve_identity_and_reject_invalid_candidates() {
        let initial = Config::default();
        let change = |key: &str, value: &str| ConfigChange {
            key: key.into(),
            value: value.into(),
            expected_revision: None,
        };
        let renamed = initial
            .changed(&change("board.name", "Club board"))
            .unwrap();
        assert_eq!(renamed.board.id, initial.board.id);
        assert!(initial.changed(&change("board.id", "new")).is_err());
        assert!(initial.changed(&change("cameras.count", "5")).is_err());
        assert!(initial.changed(&change("board.name", "\u{1b}[2J")).is_err());
        let two = initial
            .changed(&change("cameras.count", "2"))
            .unwrap()
            .changed(&change("cameras.1.device", "/dev/v4l/by-path/a"))
            .unwrap();
        assert!(
            two.changed(&change("cameras.2.device", "/dev/v4l/by-path/a"))
                .is_err()
        );
    }

    #[test]
    fn demo_labels_have_explicit_valid_scores() {
        for (label, score) in [("T20", 60), ("D1", 2), ("SB", 25), ("DB", 50), ("MISS", 0)] {
            assert_eq!(Throw::synthetic(label).unwrap().score, score);
        }
        for label in ["T21", "D0", "S25", "", "💡"] {
            assert!(Throw::synthetic(label).is_err());
        }
    }
}
