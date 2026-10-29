use crate::client::Client;
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use oche_domain::{Command, ConfigChange, Mode, NativeEvent, Snapshot, Speed};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Tabs, Wrap},
};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

const TABS: [&str; 5] = ["Overview", "Cameras", "Configuration", "Events", "Help"];
const CONFIG_FIELDS: [&str; 6] = [
    "board.name",
    "detection.speed",
    "detection.standby_seconds",
    "server.bind",
    "server.port",
    "cameras.count",
];
const CAMERA_FIELDS: [&str; 5] = ["device", "width", "height", "fps", "rotation"];

enum Request {
    Command(Command),
    Configure(ConfigChange),
}

#[derive(Clone, Default)]
struct View {
    snapshot: Option<Snapshot>,
    events: Vec<NativeEvent>,
    error: Option<String>,
    feedback: String,
}

struct Editor {
    key: String,
    value: String,
    choices: Vec<String>,
    revision: u64,
}

#[derive(Default)]
struct Ui {
    tab: usize,
    selected: usize,
    camera_field: usize,
    editor: Option<Editor>,
    notice: String,
}

struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        ratatui::restore();
    }
}
struct AbortTask(tokio::task::JoinHandle<()>);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn run(client: Client) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    let _restore = Restore;
    let (request_tx, request_rx) = mpsc::channel(8);
    let (view_tx, mut view_rx) = watch::channel(View::default());
    let endpoint = client.endpoint.clone();
    let _worker = AbortTask(tokio::spawn(network(client, request_rx, view_tx)));
    let mut input = EventStream::new();
    let mut ui = Ui::default();
    let mut view = View::default();
    let shutdown = crate::shutdown();
    tokio::pin!(shutdown);
    loop {
        terminal.draw(|frame| render(frame, &ui, &view, &endpoint))?;
        tokio::select! {
            _ = &mut shutdown => break,
            result = view_rx.changed() => {
                if result.is_err() { break; }
                view = view_rx.borrow_and_update().clone();
            }
            event = input.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) { break; }
                    if ui.editor.is_none() && key.code == KeyCode::Char('q') { break; }
                    handle_key(&mut ui, &view, key, &request_tx);
                }
                Some(Err(error)) => return Err(error.into()),
                None => break,
                _ => {}
            }
        }
    }
    Ok(())
}

async fn network(
    client: Client,
    mut requests: mpsc::Receiver<Request>,
    updates: watch::Sender<View>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut view = View::default();
    loop {
        let result = tokio::select! {
            _ = interval.tick() => client.snapshot().await,
            request = requests.recv() => match request {
                Some(Request::Command(command)) => {
                    let result = client.command(&command).await;
                    view.feedback = match &result { Ok(_) => "Command applied".into(), Err(error) => format!("{error:#}") };
                    // A rejected command does not mean the daemon is disconnected.
                    if result.is_err() { let _ = updates.send(view.clone()); continue; }
                    result
                }
                Some(Request::Configure(change)) => {
                    let result = client.configure(&change).await;
                    view.feedback = match &result { Ok(_) => "Configuration saved".into(), Err(error) => format!("{error:#}") };
                    if result.is_err() { let _ = updates.send(view.clone()); continue; }
                    result
                }
                None => return,
            }
        };
        match result {
            Ok(snapshot) => {
                view.snapshot = Some(snapshot);
                view.error = None;
                match client.history().await {
                    Ok(events) => view.events = events,
                    Err(error) => view.error = Some(format!("{error:#}")),
                }
            }
            Err(error) => view.error = Some(format!("{error:#}")),
        }
        if updates.send(view.clone()).is_err() {
            return;
        }
    }
}

fn send(ui: &mut Ui, tx: &mpsc::Sender<Request>, request: Request) {
    ui.notice = match tx.try_send(request) {
        Ok(()) => "Request queued".into(),
        Err(_) => "Command queue is full or disconnected; retry shortly".into(),
    };
}

fn handle_key(ui: &mut Ui, view: &View, key: KeyEvent, tx: &mpsc::Sender<Request>) {
    if let Some(editor) = &mut ui.editor {
        match key.code {
            KeyCode::Esc => ui.editor = None,
            KeyCode::Enter => {
                let editor = ui.editor.take().unwrap();
                send(
                    ui,
                    tx,
                    Request::Configure(ConfigChange {
                        key: editor.key,
                        value: editor.value,
                        expected_revision: Some(editor.revision),
                    }),
                );
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Right | KeyCode::Down
                if !editor.choices.is_empty() =>
            {
                let index = editor
                    .choices
                    .iter()
                    .position(|v| *v == editor.value)
                    .unwrap_or(0);
                let delta = if matches!(key.code, KeyCode::Left | KeyCode::Up) {
                    editor.choices.len() - 1
                } else {
                    1
                };
                editor.value = editor.choices[(index + delta) % editor.choices.len()].clone();
            }
            KeyCode::Backspace if editor.choices.is_empty() => {
                editor.value.pop();
            }
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL) && editor.choices.is_empty() =>
            {
                editor.value.clear()
            }
            KeyCode::Char(ch)
                if editor.choices.is_empty() && !ch.is_control() && editor.value.len() < 512 =>
            {
                editor.value.push(ch)
            }
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Tab => {
            ui.tab = (ui.tab + 1) % TABS.len();
            ui.selected = 0;
        }
        KeyCode::BackTab => {
            ui.tab = (ui.tab + TABS.len() - 1) % TABS.len();
            ui.selected = 0;
        }
        KeyCode::Char(ch @ '1'..='5') => {
            ui.tab = ch as usize - '1' as usize;
            ui.selected = 0;
        }
        KeyCode::Char('?') => ui.tab = 4,
        _ => {}
    }
    let Some(snapshot) = &view.snapshot else {
        return;
    };
    if view.error.is_some() {
        return;
    }
    let count = if ui.tab == 1 {
        snapshot.config.cameras.len()
    } else {
        CONFIG_FIELDS.len()
    };
    ui.selected = ui.selected.min(count.saturating_sub(1));
    match key.code {
        KeyCode::Up => ui.selected = (ui.selected + count - 1) % count,
        KeyCode::Down => ui.selected = (ui.selected + 1) % count,
        KeyCode::Left if ui.tab == 1 => {
            ui.camera_field = (ui.camera_field + CAMERA_FIELDS.len() - 1) % CAMERA_FIELDS.len()
        }
        KeyCode::Right if ui.tab == 1 => {
            ui.camera_field = (ui.camera_field + 1) % CAMERA_FIELDS.len()
        }
        KeyCode::Enter if ui.tab == 1 || ui.tab == 2 => {
            let (key, value, choices) = editable(snapshot, ui);
            ui.editor = Some(Editor {
                key,
                value,
                choices,
                revision: snapshot.revision,
            });
        }
        KeyCode::Char('s') => send(ui, tx, Request::Command(Command::Start)),
        KeyCode::Char('x') => send(ui, tx, Request::Command(Command::Stop)),
        KeyCode::Char('r') => send(ui, tx, Request::Command(Command::Reset)),
        KeyCode::Char('w') => send(ui, tx, Request::Command(Command::Wake)),
        KeyCode::Char('t') if snapshot.mode == Mode::Demo => send(
            ui,
            tx,
            Request::Command(Command::DemoThrow {
                label: "T20".into(),
            }),
        ),
        KeyCode::Char('o') if snapshot.mode == Mode::Demo => send(
            ui,
            tx,
            Request::Command(Command::DemoTakeout { remaining: 0 }),
        ),
        _ => {}
    }
}

fn editable(snapshot: &Snapshot, ui: &Ui) -> (String, String, Vec<String>) {
    let c = &snapshot.config;
    if ui.tab == 1 {
        let camera = &c.cameras[ui.selected];
        let field = CAMERA_FIELDS[ui.camera_field];
        let value = match field {
            "device" => camera.device.clone(),
            "width" => camera.width.to_string(),
            "height" => camera.height.to_string(),
            "fps" => camera.fps.to_string(),
            _ => camera.rotation.to_string(),
        };
        let choices = if field == "rotation" {
            vec!["0", "90", "180", "270"]
        } else {
            vec![]
        };
        return (
            format!("cameras.{}.{}", camera.slot, field),
            value,
            choices.into_iter().map(str::to_string).collect(),
        );
    }
    let key = CONFIG_FIELDS[ui.selected];
    let value = config_value(snapshot, key);
    let choices = match key {
        "detection.speed" => Speed::ALL
            .into_iter()
            .map(|s| s.as_str().to_string())
            .collect(),
        "detection.standby_seconds" => [0, 300, 600, 900, 1800, 3600]
            .map(|v| v.to_string())
            .to_vec(),
        "cameras.count" => [1, 2, 3, 4].map(|v| v.to_string()).to_vec(),
        _ => vec![],
    };
    (key.into(), value, choices)
}

fn config_value(s: &Snapshot, key: &str) -> String {
    match key {
        "board.name" => s.config.board.name.clone(),
        "detection.speed" => s.config.detection.speed.as_str().into(),
        "detection.standby_seconds" => s.config.detection.standby_seconds.to_string(),
        "server.bind" => s.config.server.bind.to_string(),
        "server.port" => s.config.server.port.to_string(),
        "cameras.count" => s.config.cameras.len().to_string(),
        _ => String::new(),
    }
}

fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}

fn render(frame: &mut Frame, ui: &Ui, view: &View, endpoint: &str) {
    let area = frame.area();
    if area.width < 60 || area.height < 20 {
        frame.render_widget(
            Paragraph::new("OcheCore\nResize to at least 60×20.\nq: quit")
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let sections = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(4),
    ])
    .split(area);
    let connected = view.snapshot.is_some() && view.error.is_none();
    frame.render_widget(
        Paragraph::new(format!(
            "OcheCore  ·  {}  ·  {}",
            if connected {
                "CONNECTED"
            } else {
                "CONNECTING / OFFLINE"
            },
            endpoint
        ))
        .style(Style::default().fg(if connected {
            Color::Cyan
        } else {
            Color::Yellow
        }))
        .block(Block::default().borders(Borders::ALL)),
        sections[0],
    );
    frame.render_widget(
        Tabs::new(TABS)
            .select(ui.tab)
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .block(Block::default().borders(Borders::ALL)),
        sections[1],
    );
    let mut lines: Vec<Line> = Vec::new();
    if let Some(error) = &view.error {
        lines.push(Line::styled(
            "OFFLINE — last known state; controls disabled",
            Style::default().fg(Color::Yellow),
        ));
        lines.push(Line::raw(plain(error)));
        lines.push(Line::raw(
            "Start `oche-core run` in another terminal. Reconnecting automatically.",
        ));
        lines.push(Line::raw(""));
    }
    if let Some(s) = &view.snapshot {
        match ui.tab {
            0 => {
                lines.extend([
                    Line::styled(
                        plain(&s.config.board.name),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Line::raw(format!("Board ID    {}", s.config.board.id)),
                    Line::raw(format!("Service     {:?}     Mode: {:?}", s.state, s.mode)),
                    Line::styled(
                        "Capture, calibration, vision: not implemented",
                        Style::default().fg(Color::Yellow),
                    ),
                    Line::raw(format!(
                        "Scene       {:?}     Physical darts: {}",
                        s.scene,
                        s.physical_darts
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "unknown".into())
                    )),
                    Line::raw(format!("Listening   {}", s.listening_on)),
                    Line::raw(format!(
                        "Uptime      {}s     Revision: {}",
                        s.uptime_ms / 1000,
                        s.revision
                    )),
                    Line::raw(""),
                    Line::styled("CURRENT VISIT", Style::default().fg(Color::Cyan)),
                    Line::raw(if s.throws.is_empty() {
                        "No accepted throws".into()
                    } else {
                        s.throws
                            .iter()
                            .enumerate()
                            .map(|(i, t)| format!("{}. {} ({})", i + 1, t.label, t.score))
                            .collect::<Vec<_>>()
                            .join("   ")
                    }),
                    Line::raw(""),
                    Line::raw(if s.mode == Mode::Demo {
                        "DEMO: t adds synthetic T20; o completes synthetic takeout."
                    } else {
                        "Control service only. No cameras are opened and no scores are detected."
                    }),
                ]);
            }
            1 => {
                lines.push(Line::raw(
                    "Requested configuration — hardware modes are not verified.",
                ));
                lines.push(Line::raw(
                    "↑/↓ camera · ←/→ field · Enter edit · camera count in Configuration",
                ));
                lines.push(Line::raw(format!(
                    "Selected field: {}",
                    CAMERA_FIELDS[ui.camera_field]
                )));
                lines.push(Line::raw(""));
                let visible = (sections[2].height.saturating_sub(6) / 3).max(1) as usize;
                let first = ui.selected.saturating_sub(visible - 1);
                for (i, c) in s
                    .config
                    .cameras
                    .iter()
                    .enumerate()
                    .skip(first)
                    .take(visible)
                {
                    lines.push(Line::styled(
                        format!(
                            "{} Camera {}   {}x{} @ {} FPS   {}°",
                            if i == ui.selected { "›" } else { " " },
                            c.slot,
                            c.width,
                            c.height,
                            c.fps,
                            c.rotation
                        ),
                        selection(i == ui.selected),
                    ));
                    lines.push(Line::raw(format!(
                        "    {}",
                        if c.device.is_empty() {
                            "unassigned".into()
                        } else {
                            plain(&c.device)
                        }
                    )));
                    lines.push(Line::raw(
                        "    Capture: unavailable   Calibration: unavailable",
                    ));
                }
            }
            2 => {
                lines.push(Line::raw(
                    "↑/↓ select · Enter edit · changes persist on the connected daemon",
                ));
                lines.push(Line::raw(""));
                for (i, key) in CONFIG_FIELDS.iter().enumerate() {
                    lines.push(Line::styled(
                        format!(
                            "{} {:28} {}",
                            if i == ui.selected { "›" } else { " " },
                            key,
                            plain(&config_value(s, key))
                        ),
                        selection(i == ui.selected),
                    ));
                }
                lines.push(Line::raw(""));
                lines.push(Line::raw(
                    "Standby: seconds; 0 disables. Status polling does not keep awake.",
                ));
                lines.push(Line::raw(
                    "Speed is saved for the future detector; it has no capture effect yet.",
                ));
                lines.push(Line::raw(
                    "Reducing camera count removes settings for the highest slots.",
                ));
                lines.push(Line::raw(
                    "Listen address/port changes require a daemon restart.",
                ));
                if s.restart_required {
                    lines.push(Line::styled(
                        "Restart required to apply saved listen settings",
                        Style::default().fg(Color::Yellow),
                    ));
                }
            }
            3 => {
                lines.push(Line::raw(
                    "Latest native events (up to 64 retained; newest first)",
                ));
                for event in view
                    .events
                    .iter()
                    .rev()
                    .take(sections[2].height.saturating_sub(4) as usize)
                {
                    lines.push(Line::raw(format!(
                        "#{:<5} {:24} {:?}",
                        event.sequence, event.kind, event.snapshot.state
                    )));
                }
                if view.events.is_empty() {
                    lines.push(Line::raw("No state changes yet."));
                }
            }
            _ => help(&mut lines),
        }
    } else if ui.tab == 4 {
        help(&mut lines);
    } else if view.error.is_none() {
        lines.push(Line::raw("Connecting to the native control service…"));
    }
    let mut body = Paragraph::new(lines).block(Block::default().borders(Borders::ALL));
    if !matches!(ui.tab, 1 | 2) {
        body = body.wrap(Wrap { trim: false });
    }
    frame.render_widget(body, sections[2]);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("Tab / 1–5", Style::default().fg(Color::Cyan)),
                Span::raw(" screens · s start · x stop · r reset · w wake · q quit"),
            ]),
            Line::raw(plain(&view.feedback)),
            Line::raw(plain(&ui.notice)),
        ])
        .block(Block::default().borders(Borders::TOP)),
        sections[3],
    );
    if let Some(editor) = &ui.editor {
        let width = area.width.saturating_sub(4).min(80);
        let popup = Rect::new(
            (area.width - width) / 2,
            area.height.saturating_sub(7) / 2,
            width,
            7,
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(&*editor.key),
                Line::raw(""),
                Line::styled(
                    format!("{}▏", editor.value),
                    Style::default().fg(Color::Cyan),
                ),
                Line::raw(if editor.choices.is_empty() {
                    "Type / Backspace · Ctrl-U clear · Enter save · Esc cancel"
                } else {
                    "←/→ choose · Enter save · Esc cancel"
                }),
            ])
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(" Edit configuration ")
                    .borders(Borders::ALL),
            ),
            popup,
        );
    }
}

fn selection(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn help(lines: &mut Vec<Line>) {
    for line in [
        "Keyboard",
        "Tab / Shift-Tab or 1–5: change screen",
        "s: start    x: stop    r: reset visit    w: wake    q: quit",
        "↑/↓: select a configuration field or camera",
        "←/→: select a camera field; choose options inside an editor",
        "Enter: edit/save    Esc: cancel    Ctrl-U: clear text",
        "Demo mode only: t adds T20; o completes takeout",
        "",
        "The daemon keeps running when this UI closes.",
        "Reset clears the logical visit, not physical darts.",
        "Calibration and vision will be added after the control foundation.",
    ] {
        lines.push(Line::raw(line));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_fourth_camera_remains_visible_in_an_ordinary_ssh_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let store = oche_engine::storage::Store::acquire(&dir.path().join("board.toml")).unwrap();
        let mut config = store.initialize().unwrap();
        config.cameras = (1..=4).map(oche_domain::CameraConfig::new).collect();
        let engine =
            oche_engine::Engine::new(config, store, Mode::ControlOnly, "127.0.0.1:5180".into());
        let view = View {
            snapshot: Some(engine.snapshot()),
            ..View::default()
        };
        let ui = Ui {
            tab: 1,
            selected: 3,
            ..Ui::default()
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render(frame, &ui, &view, "http://127.0.0.1:5180/api/v1"))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Camera 4"));
    }

    #[test]
    fn disconnected_screen_explains_how_to_start_and_small_terminals_render() {
        for (width, height) in [(100, 30), (48, 14), (25, 8)] {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            let view = View {
                error: Some("Connection refused".into()),
                ..View::default()
            };
            terminal
                .draw(|frame| render(frame, &Ui::default(), &view, "http://127.0.0.1:5180/api/v1"))
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(text.contains("OcheCore"));
            if width == 100 {
                assert!(text.contains("oche-core run"));
            }
        }
    }
}
