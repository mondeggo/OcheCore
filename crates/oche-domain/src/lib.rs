//! Native protocol types. Autodarts compatibility will live in a separate adapter.
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, net::IpAddr};
use uuid::Uuid;

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
    ControlOnly,
    Demo,
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
