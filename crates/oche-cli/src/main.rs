mod client;
mod setup;
mod tui;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use client::Client;
use oche_domain::{
    CalibrationRecord, CameraState, Command, ConfigChange, DEFAULT_PORT, Mode, NativeEvent,
    Snapshot,
};
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

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum LogLevel {
    /// Native events: starts, throws, takeouts, camera changes.
    Info,
    /// Events plus detector internals.
    Debug,
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
        #[arg(long, conflicts_with = "no_capture")]
        demo: bool,
        /// Run controls only and never open cameras.
        #[arg(long)]
        no_capture: bool,
        /// Print events to the terminal as they happen: `info` shows the same
        /// events as `oche-core events`; `debug` adds what the detector sees
        /// (changed pixels, dart lines, impacts, abstentions). Silent by default.
        #[arg(long, value_enum, value_name = "LEVEL")]
        log: Option<LogLevel>,
        /// Save an annotated PNG per camera for each arrival into this folder
        /// (changed pixels in red, the fitted dart line in cyan, the tip in green),
        /// keeping the newest 30 arrivals. For diagnosing detection.
        #[arg(long, value_name = "DIR")]
        debug_images: Option<std::path::PathBuf>,
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
    /// Measure the board and calibrate cameras from their live frames. The
    /// board must be empty, lit, and running; set cameras.N.hint first.
    Calibrate {
        #[arg(long, conflicts_with = "all", required_unless_present = "all", value_parser = clap::value_parser!(u8).range(1..=4))]
        camera: Option<u8>,
        #[arg(long)]
        all: bool,
    },
    /// Find the 20 in every camera: with an empty board, follow the prompt to
    /// throw one dart at the 20. The hints are stored until you change them.
    Orient,
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
    /// Save a streaming camera's latest frame as JPEG, e.g. to find the pixel
    /// of the 20 segment for cameras.N.hint.
    Snapshot {
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=4))]
        camera: u8,
        #[arg(long)]
        out: std::path::PathBuf,
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
    if let Some(Action::Run {
        demo,
        no_capture,
        init_bind,
        log,
        debug_images,
    }) = args.command
    {
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
        let mode = if demo {
            Mode::Demo
        } else if no_capture {
            Mode::ControlOnly
        } else {
            Mode::Live
        };
        eprintln!(
            "OcheCore listening on http://{address} ({mode:?}); config: {}",
            path.display()
        );
        eprintln!(
            "{}",
            match mode {
                Mode::Live =>
                    "Cameras open while the board is running. Autodarts compatibility is unavailable.",
                _ => "Cameras are not opened in this mode. Autodarts compatibility is unavailable.",
            }
        );
        let engine = std::sync::Arc::new(std::sync::Mutex::new(Engine::new(
            config,
            store,
            mode,
            address.to_string(),
        )));
        if log.is_some() {
            let (_, mut events) = engine
                .lock()
                .map_err(|_| anyhow::anyhow!("engine unavailable"))?
                .subscribe();
            tokio::spawn(async move {
                use tokio::sync::broadcast::error::RecvError;
                loop {
                    match events.recv().await {
                        Ok(event) => eprintln!("{}", event_line(&event)),
                        Err(RecvError::Lagged(n)) => eprintln!("log: skipped {n} events"),
                        Err(RecvError::Closed) => break,
                    }
                }
            });
            eprintln!(
                "Logging {} to this terminal.",
                if log == Some(LogLevel::Debug) {
                    "events and detector details"
                } else {
                    "events"
                }
            );
        }
        let pipeline = (mode == Mode::Live).then(|| {
            oche_engine::live::Pipeline::spawn_with(
                engine.clone(),
                log == Some(LogLevel::Debug),
                debug_images,
            )
        });
        let service = oche_api::Service::shared(engine, pipeline.clone());
        let result = oche_api::serve(listener, service, shutdown()).await;
        if let Some(pipeline) = pipeline {
            tokio::task::spawn_blocking(move || pipeline.shutdown()).await?;
        }
        result?;
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
        Some(Action::Orient) => orient(&client, args.json).await?,
        Some(Action::Calibrate { camera, .. }) => {
            // Calibration needs to know where the 20 is. Ask once; later runs
            // reuse the stored reference and only realign the cameras.
            if !client
                .snapshot()
                .await?
                .cameras_needing_orientation()
                .is_empty()
            {
                if !std::io::stdin().is_terminal() {
                    anyhow::bail!(
                        "the 20 is not recorded yet; run `oche-core orient` in a terminal first"
                    );
                }
                orient(&client, args.json).await?;
            }
            if !args.json {
                eprintln!("Calibrating; keep the board empty and still…");
            }
            let snapshot = client.command(&Command::Calibrate { camera }).await?;
            if args.json {
                print_json(&snapshot.calibration)?;
            } else if let Some(record) = &snapshot.calibration {
                print_calibration(record);
            }
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
        Some(Action::Cameras {
            action: CameraAction::Snapshot { camera, out },
        }) => {
            let jpeg = client.camera_snapshot(camera).await?;
            std::fs::write(&out, &jpeg).with_context(|| format!("write {}", out.display()))?;
            if args.json {
                print_json(
                    &serde_json::json!({"camera": camera, "path": out, "bytes": jpeg.len()}),
                )?;
            } else {
                println!(
                    "Saved camera {camera} frame to {} ({} bytes). Note the pixel x,y of the 20 segment and run: oche-core config set cameras.{camera}.hint X,Y",
                    out.display(),
                    jpeg.len()
                );
            }
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
                .iter()
                .filter(|c| match action {
                    CameraAction::List => true,
                    CameraAction::Available | CameraAction::Snapshot { .. } => unreachable!(),
                    CameraAction::Inspect { camera } => c.slot == camera,
                })
                .cloned()
                .collect::<Vec<_>>();
            if cameras.is_empty() {
                bail!("camera slot is not configured");
            }
            if args.json {
                print_json(&serde_json::json!({
                    "capture_available": snapshot.capabilities.capture,
                    "calibration_available": snapshot.capabilities.calibration,
                    "requested": cameras,
                    "status": snapshot.cameras,
                }))?;
            } else {
                for c in cameras {
                    let calibrated = snapshot
                        .calibration
                        .as_ref()
                        .and_then(|r| r.for_camera(&c))
                        .map(|cal| format!("calibrated ({:.2} mm)", cal.wire_rms_mm))
                        .unwrap_or_else(|| "not calibrated".into());
                    println!(
                        "{}  {}  {}x{} @ {} FPS  rotation {}°  exposure {}  hint {}\n   {}  ·  {}",
                        c.slot,
                        if c.device.is_empty() {
                            "unassigned"
                        } else {
                            &c.device
                        },
                        c.width,
                        c.height,
                        c.fps,
                        c.rotation,
                        c.exposure.map_or("auto".into(), |e| e.to_string()),
                        c.hint.map_or("none".into(), |[x, y]| format!("{x},{y}")),
                        camera_state(&snapshot, c.slot),
                        calibrated
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
                let yes_no = |v: bool| if v { "yes" } else { "no" };
                println!(
                    "Configuration: valid\nNative API: reachable\nCapture: {} | calibration: {} | detection: {} | Autodarts compatibility: not implemented",
                    yes_no(snapshot.capabilities.capture),
                    yes_no(snapshot.capabilities.calibration),
                    yes_no(snapshot.capabilities.vision)
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

/// One line for the daemon's own log: seconds since start, kind, state, visit.
fn event_line(event: &NativeEvent) -> String {
    let s = &event.snapshot;
    let visit = if s.throws.is_empty() {
        "—".to_string()
    } else {
        s.throws
            .iter()
            .map(|t| t.label.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut line = format!(
        "[{:>8.1}s] #{:<4} {:<18} {:?}, scene {:?}, visit {visit}, darts on board {}",
        s.uptime_ms as f64 / 1000.0,
        event.sequence,
        event.kind,
        s.state,
        s.scene,
        s.physical_darts
            .map_or("unknown".to_string(), |n| n.to_string())
    );
    if event.kind == "throw_uncertain"
        && let Some(note) = s.throws.iter().rev().find_map(|t| t.uncertainty.as_deref())
    {
        line.push_str(&format!("\n             ? {note}"));
    }
    line
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Records where the 20 is in every camera: remember the empty board, wait for
/// one dart thrown at the 20, then locate it.
async fn orient(client: &Client, json: bool) -> Result<()> {
    client.command(&Command::OrientBegin).await?;
    eprintln!(
        "First setup: the board is empty and remembered. Throw ONE dart at the 20 (aim for\nthe middle of the segment), let it settle, then press Enter."
    );
    std::io::stdin().read_line(&mut String::new())?;
    show(&client.command(&Command::OrientFinish).await?, json)?;
    if !json {
        eprintln!("The 20 is recorded. Remove the dart and keep the board empty for calibration.");
    }
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
    for (index, throw) in snapshot.throws.iter().enumerate() {
        if let Some(note) = &throw.uncertainty {
            println!("Dart {} is uncertain: {note}", index + 1);
        }
    }
    let activity = snapshot.activity();
    println!("Board: {} — {}", activity.label, activity.detail);
    println!(
        "Detection: {}",
        match snapshot.mode {
            Mode::Demo => "synthetic input (demo)".to_string(),
            Mode::ControlOnly => "off (controls only)".to_string(),
            Mode::Live if !snapshot.capabilities.calibration => {
                "needs calibration (oche-core calibrate --all)".to_string()
            }
            Mode::Live if !snapshot.capabilities.vision => {
                "needs at least two calibrated cameras".to_string()
            }
            Mode::Live => format!(
                "live at {} analyses/s per camera",
                snapshot.config.detection.speed.analysis_hz()
            ),
        }
    );
    for camera in &snapshot.config.cameras {
        println!(
            "Camera {}: {}",
            camera.slot,
            camera_state(snapshot, camera.slot)
        );
    }
    if snapshot.restart_required {
        println!("Listen settings saved; restart the daemon to apply them.");
    }
    Ok(())
}

fn camera_state(snapshot: &Snapshot, slot: u8) -> String {
    match snapshot.cameras.iter().find(|c| c.slot == slot) {
        Some(c) => match c.state {
            CameraState::Streaming => format!("streaming {:.1} FPS", c.fps),
            CameraState::Starting => "starting".into(),
            CameraState::Off => "off".into(),
            CameraState::Error => format!("error: {}", c.error.as_deref().unwrap_or("unknown")),
        },
        None => "off".into(),
    }
}

fn print_calibration(record: &CalibrationRecord) {
    let b = &record.board;
    println!(
        "Board (wire centres, mm): bull {:.2} · 25 {:.2} · treble {:.2}–{:.2} · double {:.2}–{:.2}",
        b.inner_bull, b.outer_bull, b.treble_inner, b.treble_outer, b.double_inner, b.double_outer
    );
    for camera in &record.cameras {
        println!(
            "Camera {}: wire fit {:.2} mm RMS over {} points",
            camera.slot, camera.wire_rms_mm, camera.wire_points
        );
    }
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
