use anyhow::{Context, Result, bail};
use fs2::FileExt;
use oche_domain::{CalibrationRecord, Config};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

/// Held for the daemon lifetime, including across atomic config replacements.
pub struct Store {
    path: PathBuf,
    _lock: File,
}

impl Store {
    pub fn acquire(path: &Path) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent).context("create configuration directory")?;
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        lock.try_lock_exclusive()
            .context("configuration is in use by another daemon or config command")?;
        Ok(Self {
            path: path.into(),
            _lock: lock,
        })
    }

    pub fn load_or_create(&self) -> Result<Config> {
        if self.path.try_exists()? {
            return read(&self.path);
        }
        let config = Config::default();
        self.save(&config)?;
        Ok(config)
    }

    pub fn initialize(&self) -> Result<Config> {
        if self.path.try_exists()? {
            bail!("configuration already exists; it has not been overwritten");
        }
        self.load_or_create()
    }

    /// `board.toml` keeps its calibration in `board.calibration.json`.
    pub fn calibration_path(&self) -> PathBuf {
        sibling(&self.path, "calibration.json")
    }

    pub fn load_calibration(&self) -> Result<Option<CalibrationRecord>> {
        let path = self.calibration_path();
        if !path.try_exists()? {
            return Ok(None);
        }
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let record: CalibrationRecord =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        record.validate().map_err(anyhow::Error::msg)?;
        Ok(Some(record))
    }

    /// Validates, keeps the current file as `*.calibration.previous.json`, and
    /// atomically replaces it.
    pub fn save_calibration(&self, record: &CalibrationRecord) -> Result<()> {
        record.validate().map_err(anyhow::Error::msg)?;
        let path = self.calibration_path();
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if path.try_exists()? {
            let mut previous = tempfile::NamedTempFile::new_in(parent)?;
            previous.write_all(&fs::read(&path)?)?;
            previous.as_file().sync_all()?;
            previous
                .persist(sibling(&self.path, "calibration.previous.json"))
                .context("keep previous calibration")?;
        }
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(serde_json::to_string_pretty(record)?.as_bytes())?;
        temp.as_file().sync_all()?;
        temp.persist(&path)
            .context("atomically replace calibration")?;
        File::open(parent)?
            .sync_all()
            .context("sync calibration directory")?;
        Ok(())
    }

    pub fn save(&self, config: &Config) -> Result<()> {
        config.validate().map_err(anyhow::Error::msg)?;
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(toml::to_string_pretty(config)?.as_bytes())?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path)
            .context("atomically replace configuration")?;
        File::open(parent)?
            .sync_all()
            .context("sync configuration directory")?;
        Ok(())
    }
}

fn sibling(config: &Path, suffix: &str) -> PathBuf {
    let stem = config
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "board".into());
    config.with_file_name(format!("{stem}.{suffix}"))
}

pub fn read(path: &Path) -> Result<Config> {
    let metadata = fs::metadata(path).with_context(|| {
        format!(
            "read {}; run `oche-core --config {:?} init` first",
            path.display(),
            path
        )
    })?;
    if metadata.len() > 64 * 1024 {
        bail!("configuration exceeds 64 KiB");
    }
    let config: Config =
        toml::from_str(&fs::read_to_string(path)?).context("parse configuration TOML")?;
    config.validate().map_err(anyhow::Error::msg)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_restart_and_lock_prevents_two_owners() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.toml");
        let store = Store::acquire(&path).unwrap();
        let mut config = store.initialize().unwrap();
        let id = config.board.id;
        assert!(Store::acquire(&path).is_err());
        assert!(store.initialize().is_err());
        config.board.name = "Club".into();
        store.save(&config).unwrap();
        drop(store);
        let next = Store::acquire(&path).unwrap().load_or_create().unwrap();
        assert_eq!(next.board.id, id);
        assert_eq!(next.board.name, "Club");
    }

    #[test]
    fn invalid_config_does_not_replace_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.toml");
        let store = Store::acquire(&path).unwrap();
        let mut config = store.initialize().unwrap();
        let original = fs::read(&path).unwrap();
        config.server.port = 0;
        assert!(store.save(&config).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    fn record(created: u64) -> CalibrationRecord {
        CalibrationRecord {
            schema_version: oche_domain::CALIBRATION_VERSION,
            created_unix_ms: created,
            board: oche_domain::board::BoardDimensions::wdf(),
            cameras: vec![oche_domain::CameraCalibration {
                slot: 1,
                device: "/dev/v4l/by-path/a".into(),
                width: 1280,
                height: 720,
                rotation: 0,
                to_board: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                wire_rms_mm: 0.25,
                wire_points: 2000,
            }],
        }
    }

    #[test]
    fn calibration_round_trips_keeps_the_previous_and_rejects_invalid_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::acquire(&dir.path().join("board.toml")).unwrap();
        assert!(store.load_calibration().unwrap().is_none());
        store.save_calibration(&record(1)).unwrap();
        store.save_calibration(&record(2)).unwrap();
        assert_eq!(
            store.load_calibration().unwrap().unwrap().created_unix_ms,
            2
        );
        let previous =
            fs::read_to_string(dir.path().join("board.calibration.previous.json")).unwrap();
        assert!(previous.contains("\"created_unix_ms\": 1"));

        let mut invalid = record(3);
        invalid.cameras[0].to_board[0][0] = f64::NAN;
        assert!(store.save_calibration(&invalid).is_err());
        assert_eq!(
            store.load_calibration().unwrap().unwrap().created_unix_ms,
            2
        );
    }
}
