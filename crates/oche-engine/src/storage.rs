use anyhow::{Context, Result, bail};
use fs2::FileExt;
use oche_domain::Config;
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
}
