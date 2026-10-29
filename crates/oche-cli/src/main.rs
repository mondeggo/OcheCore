mod client;
mod setup;
mod tui;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use client::Client;
use oche_domain::{Command, ConfigChange, DEFAULT_PORT, Mode, NativeEvent, Snapshot};
use oche_engine::{
    Engine,
    storage::{self, Store},
};
use std::{
    io::{IsTerminal, Write},
    net::IpAddr,
    path::PathBuf,
    time::Duration,
};

#[derive(Parser)]
#[command(
    name = "oche-core",
    version,
    about = "OcheCore offline board control and terminal UI",
    long_about = "OcheCore offline board control and terminal UI. Capture, vision, calibration, and Autodarts compatibility are not implemented yet. Use run --demo to exercise synthetic visits."
)]
struct Args {
    #[arg(short = 'c', long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,
    #[arg(short = 'H', long, global = true, default_value = "127.0.0.1")]
    host: String,
    #[arg(short = 'p', long, global = true, value_parser = clap::value_parser!(u16).range(1..))]
    port: Option<u16>,
    /// Listen address for run or initialization (loopback by default).
    #[arg(long, global = true)]
    bind: Option<IpAddr>,
    /// Emit machine-readable JSON on stdout.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Create configuration interactively; Enter accepts each default.
    Init(setup::Options),
    /// Set up missing configuration, then run the foreground control service.
    Run {
        /// Initial listen address for a new configuration only (used by Docker).
        #[arg(long)]
        init_bind: Option<IpAddr>,
        /// Enable explicitly synthetic throws and takeout; no cameras are opened.
        #[arg(long)]
        demo: bool,
    },
    /// Open the interactive terminal UI for a local or remote daemon.
    Remote,
    Status,
    Start,
    Stop,
    Reset,
    /// Correct an existing dart without changing physical occupancy.
    Correct {
        #[arg(value_parser = clap::value_parser!(u8).range(1..=3))]
        dart: u8,
        label: String,
    },
    Wake,
    /// Follow native events; reconnects with a fresh snapshot.
    Events {
        /// Exit after this many events; omitted means follow until Ctrl-C.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        count: Option<u64>,
    },
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    Cameras {
        #[command(subcommand)]
        action: CameraAction,
    },
    /// Report service capabilities and configuration health.
    Doctor,
    /// Reserved calibration control; currently reports not implemented.
    Calibrate {
        #[arg(long, conflicts_with = "all", required_unless_present = "all", value_parser = clap::value_parser!(u8).range(1..=4))]
        camera: Option<u8>,
        #[arg(long)]
        all: bool,
    },
    /// Inject synthetic observations into a daemon started with run --demo.
    Demo {
        #[command(subcommand)]
        action: DemoAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Same as init; never overwrite an existing file.
    Init(setup::Options),
    /// Replace local settings and board ID; stop the daemon first.
    Reset {
        #[command(flatten)]
        options: setup::Options,
        /// Confirm replacement of all settings and creation of a new board ID.
        #[arg(long)]
        yes: bool,
    },
    /// Validate the local TOML file without contacting a daemon.
    Validate,
    /// Export the connected daemon's configuration as TOML (or --json).
    Export,
    /// Change a setting through the daemon and atomically save it.
    Set { key: String, value: String },
}

#[derive(Subcommand)]
enum CameraAction {
    /// Query real devices and advertised modes on the daemon's machine.
    Available,
    /// Show requested camera slots; hardware enumeration will arrive with capture.
    List,
    Inspect {
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=4))]
        camera: u8,
    },
}

#[derive(Subcommand)]
enum DemoAction {
    Throw {
        label: String,
    },
    Takeout {
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=2))]
        remaining: u8,
    },
}

fn config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(path).join("ochecore/board.toml"));
    }
    let home = std::env::var_os("HOME")
        .context("set --config or XDG_CONFIG_HOME when HOME is unavailable")?;
    Ok(PathBuf::from(home).join(".config/ochecore/board.toml"))
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let json = args.json;
    if let Err(error) = execute(args).await {
        if json {
            eprintln!("{}", serde_json::json!({"error": format!("{error:#}")}));
        } else {
            eprintln!("Error: {error:#}");
        }
        std::process::exit(1);
    }
}

async fn execute(args: Args) -> Result<()> {
    let path = match &args.config {
        Some(path) => path.clone(),
        None => config_path()?,
    };
    let setup = match &args.command {
        Some(Action::Init(options))
        | Some(Action::Config {
            action: ConfigAction::Init(options),
        }) => Some((options, false, false)),
        Some(Action::Config {
            action: ConfigAction::Reset { options, yes },
        }) => Some((options, true, *yes)),
        _ => None,
    };
    if let Some((options, reset, yes)) = setup {
        if args.host != "127.0.0.1" {
            bail!(
                "--host selects a remote daemon; use --bind for the local board's listen address"
            );
        }
        let config =
            setup::initialize(&path, options, reset, yes, args.bind, args.port, args.json)?;
        if args.json {
            print_json(&config)?;
        } else {
            println!(
                "Saved {}\nBoard ID: {}\nStart the daemon with `oche-core --config {:?} run`, then open `oche-core --config {:?} remote`.",
                path.display(),
                config.board.id,
                path,
                path
            );
        }
        return Ok(());
    }
    if args.bind.is_some() && !matches!(args.command, Some(Action::Run { .. })) {
        bail!(
            "--bind is only valid with run, init, or config reset; --host selects a daemon to manage"
        );
    }
    if args.command.is_none() && args.host == "127.0.0.1" && !path.try_exists()? {
        bail!(
            "no configuration at {}; run `oche-core --config {:?} init` first (or add --defaults for unattended setup)",
            path.display(),
            path
        );
    }
    if let Some(Action::Run { demo, init_bind }) = args.command {
        if args.host != "127.0.0.1" {
            bail!(
                "--host is for management commands; use --bind to select the service listen address"
            );
        }
        if !path.try_exists()? {
            setup::initialize_for_run(&path, args.bind.or(init_bind), args.port, args.json)?;
        }
        let store = Store::acquire(&path)?;
        let config = storage::read(&path)?;
        let address = std::net::SocketAddr::new(
            args.bind.unwrap_or(config.server.bind),
            args.port.unwrap_or(config.server.port),
        );
        let listener = tokio::net::TcpListener::bind(address).await.with_context(|| format!("cannot listen on {address}; another service may own this port. Use --port to choose another"))?;
        let mode = if demo { Mode::Demo } else { Mode::ControlOnly };
        eprintln!(
            "OcheCore listening on http://{address} ({mode:?}); config: {}",
            path.display()
        );
        eprintln!("Capture, vision, calibration, and Autodarts compatibility are unavailable.");
        let service = oche_api::Service::new(Engine::new(config, store, mode, address.to_string()));
        oche_api::serve(listener, service, shutdown()).await?;
        return Ok(());
    }
    if let Some(Action::Config {
        action: ConfigAction::Validate,
    }) = args.command
    {
        let config = storage::read(&path)?;
        if args.json {
            print_json(&serde_json::json!({"valid": true, "board_id": config.board.id}))?;
        } else {
            println!("Valid configuration: {}", path.display());
        }
        return Ok(());
    }
    let port = match args.port {
        Some(port) => port,
        None if path.try_exists()? => storage::read(&path)?.server.port,
        None => DEFAULT_PORT,
    };
    let client = Client::new(&args.host, port)?;
    match args.command {
        None | Some(Action::Remote) => {
            if args.json {
                bail!("the interactive UI does not support --json; use status --json");
            }
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                bail!(
                    "the terminal UI requires an interactive terminal; use status --json or --help"
                );
            }
            tui::run(client).await?;
        }
        Some(Action::Status) => show(&client.snapshot().await?, args.json)?,
        Some(Action::Start) => show(&client.command(&Command::Start).await?, args.json)?,
        Some(Action::Stop) => show(&client.command(&Command::Stop).await?, args.json)?,
        Some(Action::Reset) => show(&client.command(&Command::Reset).await?, args.json)?,
        Some(Action::Correct { dart, label }) => {
            let snapshot = client.snapshot().await?;
            show(
                &client
                    .command(&Command::CorrectThrow {
                        dart,
                        label,
                        expected_revision: snapshot.revision,
                        expected_session_id: snapshot.session_id,
                    })
                    .await?,
                args.json,
            )?;
        }
        Some(Action::Wake) => show(&client.command(&Command::Wake).await?, args.json)?,
        Some(Action::Calibrate { camera, .. }) => {
            client.command(&Command::Calibrate { camera }).await?;
        }
        Some(Action::Demo { action }) => {
            let command = match action {
                DemoAction::Throw { label } => Command::DemoThrow { label },
                DemoAction::Takeout { remaining } => Command::DemoTakeout { remaining },
            };
            show(&client.command(&command).await?, args.json)?;
        }
        Some(Action::Config {
            action: ConfigAction::Export,
        }) => {
            let config = client.snapshot().await?.config;
            if args.json {
                print_json(&config)?;
            } else {
                print!("{}", toml::to_string_pretty(&config)?);
            }
        }
        Some(Action::Config {
            action: ConfigAction::Set { key, value },
        }) => {
            show(
                &client
                    .configure(&ConfigChange {
                        key,
                        value,
                        expected_revision: None,
                    })
                    .await?,
                args.json,
            )?;
        }
        Some(Action::Cameras { action }) => {
            if matches!(action, CameraAction::Available) {
                let inventory = client.cameras().await?;
                if args.json {
                    print_json(&inventory)?;
                } else {
                    for camera in inventory.devices {
                        println!(
                            "{}  {}  {} modes{}",
                            camera.path,
                            camera.name,
                            camera.modes.len(),
                            camera.error.map(|e| format!(" — {e}")).unwrap_or_default()
                        );
                    }
                    for warning in inventory.warnings {
                        eprintln!("{warning}");
                    }
                }
                return Ok(());
            }
            let snapshot = client.snapshot().await?;
            let cameras = snapshot
                .config
                .cameras
                .into_iter()
                .filter(|c| match action {
                    CameraAction::List => true,
                    CameraAction::Available => unreachable!(),
                    CameraAction::Inspect { camera } => c.slot == camera,
                })
                .collect::<Vec<_>>();
            if cameras.is_empty() {
                bail!("camera slot is not configured");
            }
            if args.json {
                print_json(
                    &serde_json::json!({"capture_available": false, "calibration_available": false, "requested": cameras}),
                )?;
            } else {
                println!("Requested slots only — capture and calibration are not implemented.");
                for c in cameras {
                    println!(
                        "{}  {}  {}x{} @ {} FPS  rotation {}°",
                        c.slot,
                        if c.device.is_empty() {
                            "unassigned"
                        } else {
                            &c.device
                        },
                        c.width,
                        c.height,
                        c.fps,
                        c.rotation
                    );
                }
            }
        }
        Some(Action::Doctor) => {
            let snapshot = client.snapshot().await?;
            if args.json {
                print_json(
                    &serde_json::json!({"reachable": true, "configuration_valid": snapshot.config.validate().is_ok(), "capabilities": snapshot.capabilities, "listening_on": snapshot.listening_on, "mode": snapshot.mode}),
                )?;
            } else {
                show(&snapshot, false)?;
                println!(
                    "Configuration: valid\nNative API: reachable\nCapture / calibration / vision / Autodarts compatibility: not implemented"
                );
            }
        }
        Some(Action::Events { count }) => {
            tokio::select! { result = follow_events(&client, args.json, count) => result?, _ = tokio::signal::ctrl_c() => {} }
        }
        Some(
            Action::Run { .. }
            | Action::Init(_)
            | Action::Config {
                action: ConfigAction::Init(_) | ConfigAction::Reset { .. } | ConfigAction::Validate,
            },
        ) => unreachable!(),
    }
    Ok(())
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn show(snapshot: &Snapshot, json: bool) -> Result<()> {
    if json {
        return print_json(snapshot);
    }
    println!(
        "{} [{}]\nState: {:?} | mode: {:?} | scene: {:?}\nEndpoint: {} | revision: {}\nVisit: {} | physical darts: {}",
        snapshot.config.board.name,
        snapshot.config.board.id,
        snapshot.state,
        snapshot.mode,
        snapshot.scene,
        snapshot.listening_on,
        snapshot.revision,
        if snapshot.throws.is_empty() {
            "—".into()
        } else {
            snapshot
                .throws
                .iter()
                .map(|t| t.label.clone())
                .collect::<Vec<_>>()
                .join("  ")
        },
        snapshot
            .physical_darts
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    println!(
        "Vision: unavailable{}",
        if snapshot.mode == Mode::Demo {
            " (synthetic input enabled)"
        } else {
            ""
        }
    );
    if snapshot.restart_required {
        println!("Listen settings saved; restart the daemon to apply them.");
    }
    Ok(())
}

async fn follow_events(client: &Client, json: bool, count: Option<u64>) -> Result<()> {
    let mut emitted = 0;
    loop {
        let result: Result<()> = async {
            let mut response = client.events().await?;
            let mut line = Vec::new();
            while let Some(chunk) = tokio::time::timeout(Duration::from_secs(25), response.chunk())
                .await
                .context("event stream heartbeat timed out")??
            {
                for byte in chunk {
                    if byte == b'\n' {
                        if let Some(data) = line.strip_prefix(b"data: ") {
                            let event: NativeEvent = serde_json::from_slice(data)?;
                            if json {
                                println!("{}", serde_json::to_string(&event)?);
                            } else {
                                println!(
                                    "{} #{} {} — {:?}",
                                    event.session_id,
                                    event.sequence,
                                    event.kind,
                                    event.snapshot.state
                                );
                            }
                            std::io::stdout().flush()?;
                            emitted += 1;
                            if count.is_some_and(|limit| emitted >= limit) {
                                return Ok(());
                            }
                        }
                        line.clear();
                    } else {
                        line.push(byte);
                        if line.len() > 64 * 1024 {
                            bail!("event line exceeds 64 KiB");
                        }
                    }
                }
            }
            bail!("event stream disconnected")
        }
        .await;
        if count.is_some_and(|limit| emitted >= limit) {
            return Ok(());
        }
        if let Err(error) = result {
            eprintln!("{error:#}; reconnecting in one second");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
