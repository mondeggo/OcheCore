use anyhow::{Result, bail};
use oche_domain::{Config, ConfigChange};
use oche_engine::storage::Store;
use std::{
    io::{self, BufRead, IsTerminal, Write},
    net::IpAddr,
    path::Path,
};

#[derive(clap::Args)]
pub struct Options {
    /// Save defaults without prompts (also required with --json).
    #[arg(long)]
    defaults: bool,
    /// Initial board name, or the default shown by the wizard.
    #[arg(long)]
    name: Option<String>,
}

pub fn initialize(
    path: &Path,
    options: &Options,
    reset: bool,
    yes: bool,
    bind: Option<IpAddr>,
    port: Option<u16>,
    json: bool,
) -> Result<Config> {
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal() && !json;
    if !options.defaults && !interactive {
        bail!("setup requires an interactive terminal; use --defaults for unattended setup");
    }
    if reset && !yes && !interactive {
        bail!(
            "config reset requires confirmation; use --yes to replace settings and generate a new board ID"
        );
    }
    let store = Store::acquire(path)?;
    if !reset && path.try_exists()? {
        bail!(
            "configuration already exists; use config reset to start fresh, or config set to edit it"
        );
    }
    let mut config = Config::default();
    if let Some(name) = &options.name {
        config = change(&config, "board.name", name).map_err(anyhow::Error::msg)?;
    }
    if let Some(bind) = bind {
        config.server.bind = bind;
    }
    if let Some(port) = port {
        config.server.port = port;
    }
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    if reset && !yes {
        writeln!(
            output,
            "Replace all settings at {} and generate a new board ID?",
            path.display()
        )?;
        if !matches!(
            answer(&mut input, &mut output, "Continue", "no")?
                .to_lowercase()
                .as_str(),
            "y" | "yes"
        ) {
            bail!("reset cancelled; configuration unchanged");
        }
    }
    if !options.defaults {
        config = wizard(config, &mut input, &mut output)?;
    }
    store.save(&config)?;
    Ok(config)
}

fn change(config: &Config, key: &str, value: &str) -> Result<Config, String> {
    config.changed(&ConfigChange {
        key: key.into(),
        value: value.into(),
        expected_revision: None,
    })
}

fn answer(
    input: &mut impl BufRead,
    output: &mut impl Write,
    label: &str,
    default: &str,
) -> Result<String> {
    write!(output, "{label} [{default}]: ")?;
    output.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        bail!("setup cancelled at end of input; configuration unchanged");
    }
    let value = line.trim();
    Ok(if value.is_empty() { default } else { value }.into())
}

fn wizard(mut config: Config, input: &mut impl BufRead, output: &mut impl Write) -> Result<Config> {
    writeln!(
        output,
        "OcheCore setup — press Enter to accept each default."
    )?;
    writeln!(
        output,
        "Listen address is this board's local IP; 127.0.0.1 allows local access only."
    )?;
    writeln!(
        output,
        "Use 0.0.0.0 for all IPv4 interfaces. --host selects a remote board when managing it."
    )?;
    writeln!(
        output,
        "Camera slots are configuration only; capture and calibration are not yet available."
    )?;
    let fields = [
        ("Board name", "board.name", config.board.name.clone()),
        (
            "Listen address",
            "server.bind",
            config.server.bind.to_string(),
        ),
        ("Port", "server.port", config.server.port.to_string()),
        (
            "Camera count (1–4)",
            "cameras.count",
            config.cameras.len().to_string(),
        ),
    ];
    for (label, key, default) in fields {
        loop {
            let value = answer(input, output, label, &default)?;
            match change(&config, key, &value) {
                Ok(next) => {
                    config = next;
                    break;
                }
                Err(error) => writeln!(output, "Invalid value: {error}")?,
            }
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_accepts_defaults_and_retries_invalid_values() {
        let config = Config::default();
        let defaults = wizard(config.clone(), &mut &b"\n\n\n\n"[..], &mut Vec::new()).unwrap();
        assert_eq!(defaults, config);
        let mut output = Vec::new();
        let custom = wizard(
            config.clone(),
            &mut &b"Basement\ninvalid\n0.0.0.0\n0\n5181\n5\n3\n"[..],
            &mut output,
        )
        .unwrap();
        assert_eq!(custom.board.id, config.board.id);
        assert_eq!(custom.board.name, "Basement");
        assert_eq!(custom.server.bind.to_string(), "0.0.0.0");
        assert_eq!(custom.server.port, 5181);
        assert_eq!(custom.cameras.len(), 3);
        assert_eq!(
            String::from_utf8(output)
                .unwrap()
                .matches("Invalid value")
                .count(),
            3
        );
        assert!(wizard(config, &mut &b"\n"[..], &mut Vec::new()).is_err());
    }
}
