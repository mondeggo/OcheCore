use crate::client::Client;
use anyhow::Result;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use futures_util::StreamExt;
use oche_domain::{Command, ConfigChange, Mode, NativeEvent, Snapshot, Speed};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

const TABS: [&str; 2] = ["1 Board", "2 Events"];
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
    scroll: usize,
    follow_selection: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MouseAction {
    Tab(usize),
    Key(KeyCode),
    CameraField(usize, usize),
    Config(usize),
}

struct Hit {
    area: Rect,
    action: MouseAction,
}

struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
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
    crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
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
        let mut hits = Vec::new();
        terminal.draw(|frame| hits = render(frame, &mut ui, &view, &endpoint))?;
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
                Some(Ok(Event::Mouse(mouse))) => {
                    if handle_mouse(&mut ui, &view, mouse, &hits, &request_tx) { break; }
                }
                Some(Err(error)) => return Err(error.into()),
                None => break,
                _ => {}
            }
        }
    }
    Ok(())
}

fn handle_mouse(
    ui: &mut Ui,
    view: &View,
    mouse: MouseEvent,
    hits: &[Hit],
    tx: &mpsc::Sender<Request>,
) -> bool {
    let position = (mouse.column, mouse.row).into();
    let action = hits.iter().find(|hit| hit.area.contains(position));
    let key = match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => match action.map(|hit| hit.action) {
            Some(MouseAction::Tab(tab)) => Some(KeyCode::Char(char::from(b'1' + tab as u8))),
            Some(MouseAction::Key(KeyCode::Char('q'))) => return true,
            Some(MouseAction::Key(key)) => Some(key),
            Some(MouseAction::CameraField(camera, field)) => {
                ui.selected = CONFIG_FIELDS.len() + camera;
                ui.camera_field = field;
                Some(KeyCode::Enter)
            }
            Some(MouseAction::Config(index)) => {
                ui.selected = index;
                Some(KeyCode::Enter)
            }
            None => None,
        },
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            if ui.editor.is_some() {
                action.map(|_| {
                    if mouse.kind == MouseEventKind::ScrollUp {
                        KeyCode::Up
                    } else {
                        KeyCode::Down
                    }
                })
            } else {
                ui.scroll = if mouse.kind == MouseEventKind::ScrollUp {
                    ui.scroll.saturating_sub(3)
                } else {
                    ui.scroll.saturating_add(3)
                };
                ui.follow_selection = false;
                None
            }
        }
        _ => None,
    };
    if let Some(key) = key {
        handle_key(ui, view, KeyEvent::new(key, KeyModifiers::NONE), tx);
    }
    false
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
                if view.snapshot.is_none() || view.error.is_some() {
                    ui.notice = "Disconnected; reconnect before saving".into();
                    return;
                }
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
        KeyCode::Tab | KeyCode::BackTab => {
            ui.tab = (ui.tab + 1) % TABS.len();
            ui.scroll = 0;
        }
        KeyCode::Char(ch @ '1'..='2') => {
            ui.tab = ch as usize - '1' as usize;
            ui.scroll = 0;
        }
        KeyCode::PageUp => {
            ui.scroll = ui.scroll.saturating_sub(8);
            ui.follow_selection = false;
            return;
        }
        KeyCode::PageDown => {
            ui.scroll = ui.scroll.saturating_add(8);
            ui.follow_selection = false;
            return;
        }
        KeyCode::Home => {
            ui.scroll = 0;
            ui.follow_selection = false;
            return;
        }
        KeyCode::End => {
            ui.scroll = usize::MAX;
            ui.follow_selection = false;
            return;
        }
        KeyCode::Up | KeyCode::Down if ui.tab == 1 => {
            ui.scroll = if key.code == KeyCode::Up {
                ui.scroll.saturating_sub(1)
            } else {
                ui.scroll.saturating_add(1)
            };
            return;
        }
        KeyCode::Char('?') => {
            ui.notice = "Tab: Board/Events | arrows: select | Enter: edit | +/-: cameras | PgUp/PgDn: scroll".into();
            return;
        }
        _ => {}
    }
    let Some(snapshot) = &view.snapshot else {
        return;
    };
    if view.error.is_some() {
        return;
    }
    let count = CONFIG_FIELDS.len() + snapshot.config.cameras.len();
    ui.selected = ui.selected.min(count.saturating_sub(1));
    match key.code {
        KeyCode::Up if ui.tab == 0 => {
            ui.selected = (ui.selected + count - 1) % count;
            ui.follow_selection = true;
        }
        KeyCode::Down if ui.tab == 0 => {
            ui.selected = (ui.selected + 1) % count;
            ui.follow_selection = true;
        }
        KeyCode::Left if ui.tab == 0 && ui.selected >= CONFIG_FIELDS.len() => {
            ui.camera_field = (ui.camera_field + CAMERA_FIELDS.len() - 1) % CAMERA_FIELDS.len()
        }
        KeyCode::Right if ui.tab == 0 && ui.selected >= CONFIG_FIELDS.len() => {
            ui.camera_field = (ui.camera_field + 1) % CAMERA_FIELDS.len()
        }
        KeyCode::Enter if ui.tab == 0 => {
            let (key, value, choices) = editable(snapshot, ui);
            ui.editor = Some(Editor {
                key,
                value,
                choices,
                revision: snapshot.revision,
            });
        }
        KeyCode::Char('+') | KeyCode::Char('-') if ui.tab == 0 => {
            let count = snapshot.config.cameras.len();
            let next = if key.code == KeyCode::Char('+') {
                (count + 1).min(4)
            } else {
                count.saturating_sub(1).max(1)
            };
            if next == count {
                ui.notice = "Choose between one and four cameras".into();
                return;
            }
            // Review the new count before saving; decreasing removes the highest slot.
            ui.editor = Some(Editor {
                key: "cameras.count".into(),
                value: next.to_string(),
                choices: vec!["1".into(), "2".into(), "3".into(), "4".into()],
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
    if ui.selected >= CONFIG_FIELDS.len() {
        let camera = &c.cameras[ui.selected - CONFIG_FIELDS.len()];
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

fn render(frame: &mut Frame, ui: &mut Ui, view: &View, endpoint: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    let area = frame.area();
    if area.width < 60 || area.height < 20 {
        frame.render_widget(
            Paragraph::new("OcheCore\nResize to at least 60×20.\nq: quit")
                .wrap(Wrap { trim: false }),
            area,
        );
        return hits;
    }
    let sections = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
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
    let tabs = sections[1];
    let mut x = tabs.x;
    for (index, label) in TABS.iter().enumerate() {
        let rect = Rect::new(
            x,
            tabs.y,
            (label.len() as u16 + 2).min(tabs.right().saturating_sub(x)),
            1,
        );
        button(
            frame,
            &mut hits,
            rect,
            label,
            MouseAction::Tab(index),
            true,
            ui.tab == index,
        );
        x = rect.right().saturating_add(1);
    }
    // One scrollable board dashboard; only diagnostic events have a separate tab.
    let content = Block::default().borders(Borders::ALL).inner(sections[2]);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut row_hits: Vec<(usize, MouseAction)> = Vec::new();
    let mut camera_rows = Vec::new();
    let mut selected_row = 0;
    let mut camera_heading = None;
    if let Some(error) = &view.error {
        lines.push(Line::raw("OFFLINE — last known state; controls disabled"));
        lines.push(Line::raw(plain(error)));
        lines.push(Line::raw(
            "Start `oche-core run` in another terminal. Reconnecting automatically.",
        ));
    }
    if ui.tab == 1 {
        lines.push(Line::styled(
            format!(
                "EVENTS — {} retained / 64 maximum · newest first",
                view.events.len()
            ),
            selection(true),
        ));
        lines.push(Line::raw(
            "PgUp/PgDn, Home/End or wheel: scroll all retained events",
        ));
        for event in view.events.iter().rev() {
            lines.push(Line::raw(format!(
                "#{}  {}  {:?}  revision {}",
                event.sequence,
                plain(&event.kind),
                event.snapshot.state,
                event.snapshot.revision
            )));
            lines.push(Line::raw(format!(
                "  Unix ms: {}  session: {}",
                event.unix_time_ms, event.session_id
            )));
            lines.push(Line::raw(format!(
                "  Board: {}  scene: {:?}  physical: {:?}",
                event.board_id, event.snapshot.scene, event.snapshot.physical_darts
            )));
            lines.push(Line::raw(format!("  Visit: {}", visit(&event.snapshot))));
        }
        if view.events.is_empty() {
            lines.push(Line::raw("No state changes yet."));
        }
        lines.push(Line::raw(
            "Full live JSON: oche-core events --json (use this board's host/port)",
        ));
    } else if let Some(s) = &view.snapshot {
        ui.selected = ui
            .selected
            .min(CONFIG_FIELDS.len() + s.config.cameras.len() - 1);
        lines.push(Line::styled(
            format!(
                "{} · {:?} · {:?}",
                plain(&s.config.board.name),
                s.state,
                s.mode
            ),
            selection(true),
        ));
        lines.push(Line::raw(format!("DARTS  {}", visit(s))));
        lines.push(Line::raw(format!(
            "Scene: {:?} · Physical: {:?} · Uptime: {}s",
            s.scene,
            s.physical_darts,
            s.uptime_ms / 1000
        )));
        lines.push(Line::styled("BOARD & DETECTION SETTINGS", selection(true)));
        for (index, key) in CONFIG_FIELDS.iter().enumerate() {
            if ui.selected == index {
                selected_row = lines.len();
            }
            row_hits.push((lines.len(), MouseAction::Config(index)));
            lines.push(Line::styled(
                format!(
                    "{} {:27} {}",
                    if ui.selected == index { "›" } else { " " },
                    key,
                    plain(&config_value(s, key))
                ),
                selection(ui.selected == index),
            ));
        }
        camera_heading = Some(lines.len());
        lines.push(Line::styled("CAMERAS", selection(true)));
        for (index, c) in s.config.cameras.iter().enumerate() {
            if ui.selected == CONFIG_FIELDS.len() + index {
                selected_row = lines.len() + 1;
            }
            row_hits.push((lines.len(), MouseAction::CameraField(index, 0)));
            lines.push(Line::styled(
                format!(
                    "{} Camera {} · Device: {}",
                    if ui.selected == CONFIG_FIELDS.len() + index {
                        "›"
                    } else {
                        " "
                    },
                    c.slot,
                    if c.device.is_empty() {
                        "unassigned (click to set)".into()
                    } else {
                        plain(&c.device)
                    }
                ),
                selection(ui.selected == CONFIG_FIELDS.len() + index),
            ));
            camera_rows.push((lines.len(), index));
            lines.push(Line::raw(""));
        }
        lines.push(Line::raw(
            "Capture / calibration / vision unavailable; modes are requested.",
        ));
        lines.push(Line::raw(
            "Speed: future detector. Standby: seconds (0 = off).",
        ));
        lines.push(Line::raw(
            "Reducing camera count removes the highest slots' settings.",
        ));
        lines.push(Line::raw(if s.restart_required {
            "RESTART REQUIRED: saved listen address/port changed."
        } else {
            "Listen address/port changes require restart."
        }));
        lines.push(Line::raw(format!("Board ID: {}", s.config.board.id)));
        lines.push(Line::raw(
            "Arrows: select field · Enter: edit · +/-: cameras · ? help",
        ));
        lines.push(Line::raw(
            "PgUp/PgDn, Home/End or wheel: scroll · Tab: events",
        ));
        if s.mode == Mode::Demo {
            lines.push(Line::raw(
                "DEMO: t adds synthetic T20; o completes takeout.",
            ));
        }
    } else if view.error.is_none() {
        lines.push(Line::raw("Connecting to the native control service…"));
    }
    let height = content.height as usize;
    if ui.follow_selection && ui.tab == 0 {
        if selected_row < ui.scroll {
            ui.scroll = selected_row;
        }
        if selected_row >= ui.scroll.saturating_add(height) {
            ui.scroll = selected_row + 1 - height;
        }
        ui.follow_selection = false;
    }
    ui.scroll = ui.scroll.min(lines.len().saturating_sub(height));
    let title = format!(
        " {} · rows {}–{} / {} ",
        if ui.tab == 0 { "Board" } else { "Events" },
        ui.scroll + 1,
        (ui.scroll + height).min(lines.len()),
        lines.len()
    );
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((ui.scroll as u16, 0))
            .block(Block::default().borders(Borders::ALL).title(title)),
        sections[2],
    );
    let visible_y = |row: usize| -> Option<u16> {
        (row >= ui.scroll && row < ui.scroll + height).then(|| content.y + (row - ui.scroll) as u16)
    };
    if connected {
        for (row, action) in row_hits {
            if let Some(y) = visible_y(row) {
                hits.push(Hit {
                    area: Rect::new(content.x, y, content.width, 1),
                    action,
                });
            }
        }
    }
    if let Some(y) = camera_heading.and_then(visible_y) {
        for (offset, label, key) in [(10, "+ Add", '+'), (19, "- Remove last", '-')] {
            button(
                frame,
                &mut hits,
                Rect::new(content.x + offset, y, label.len() as u16 + 2, 1),
                label,
                MouseAction::Key(KeyCode::Char(key)),
                connected,
                false,
            );
        }
    }
    for (row, index) in camera_rows {
        if let Some(y) = visible_y(row) {
            let c = &view.snapshot.as_ref().unwrap().config.cameras[index];
            let mut x = content.x + 2;
            for (field, label) in [
                (1, format!("W {}", c.width)),
                (2, format!("H {}", c.height)),
                (3, format!("FPS {}", c.fps)),
                (4, format!("Rotate {}", c.rotation)),
            ] {
                let width = label.len() as u16 + 2;
                button(
                    frame,
                    &mut hits,
                    Rect::new(x, y, width.min(content.right().saturating_sub(x)), 1),
                    &label,
                    MouseAction::CameraField(index, field),
                    connected,
                    ui.selected == CONFIG_FIELDS.len() + index && ui.camera_field == field,
                );
                x += width + 1;
            }
        }
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::raw(plain(&view.feedback)),
            Line::raw(plain(&ui.notice)),
        ])
        .block(Block::default().borders(Borders::TOP)),
        sections[3],
    );
    let mut x = sections[3].x + 1;
    for (label, key) in [
        ("s Start", 's'),
        ("x Stop", 'x'),
        ("r Reset", 'r'),
        ("w Wake", 'w'),
        ("q Quit", 'q'),
    ] {
        let rect = Rect::new(x, sections[3].y + 1, label.len() as u16 + 2, 1);
        button(
            frame,
            &mut hits,
            rect,
            label,
            MouseAction::Key(KeyCode::Char(key)),
            connected || key == 'q',
            false,
        );
        x = rect.right() + 1;
    }
    if let Some(editor) = &ui.editor {
        hits.clear();
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
                    "Type / Backspace · Ctrl-U clear"
                } else {
                    "←/→ or mouse wheel to choose"
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
        let mut x = popup.x + 2;
        let controls: &[(&str, KeyCode)] = if editor.choices.is_empty() {
            &[("Save", KeyCode::Enter), ("Cancel", KeyCode::Esc)]
        } else {
            &[
                ("< Prev", KeyCode::Left),
                ("Next >", KeyCode::Right),
                ("Save", KeyCode::Enter),
                ("Cancel", KeyCode::Esc),
            ]
        };
        for (label, key) in controls {
            let rect = Rect::new(x, popup.y + 5, label.len() as u16 + 2, 1);
            button(
                frame,
                &mut hits,
                rect,
                label,
                MouseAction::Key(*key),
                connected || *key != KeyCode::Enter,
                false,
            );
            x = rect.right() + 1;
        }
        // Let the wheel change choices anywhere inside the modal, without
        // allowing clicks on its empty space to reach the screen behind it.
        hits.push(Hit {
            area: popup,
            action: MouseAction::Key(KeyCode::Null),
        });
    }
    hits
}

fn button(
    frame: &mut Frame,
    hits: &mut Vec<Hit>,
    area: Rect,
    label: &str,
    action: MouseAction,
    enabled: bool,
    selected: bool,
) {
    if area.is_empty() {
        return;
    }
    let style = if !enabled {
        Style::default().fg(Color::DarkGray)
    } else if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan).bg(Color::DarkGray)
    };
    frame.render_widget(Paragraph::new(format!(" {label} ")).style(style), area);
    if enabled {
        hits.push(Hit { area, action });
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

fn visit(snapshot: &Snapshot) -> String {
    (0..3)
        .map(|index| {
            snapshot
                .throws
                .get(index)
                .map(|dart| format!("{}: {} ({})", index + 1, dart.label, dart.score))
                .unwrap_or_else(|| format!("{}: —", index + 1))
        })
        .collect::<Vec<_>>()
        .join("   ")
}
#[cfg(test)]
mod tests {
    use super::*;

    fn view(cameras: u8) -> View {
        let dir = tempfile::tempdir().unwrap();
        let store = oche_engine::storage::Store::acquire(&dir.path().join("board.toml")).unwrap();
        let mut config = store.initialize().unwrap();
        config.cameras = (1..=cameras).map(oche_domain::CameraConfig::new).collect();
        let engine =
            oche_engine::Engine::new(config, store, Mode::ControlOnly, "127.0.0.1:5180".into());
        View {
            snapshot: Some(engine.snapshot()),
            ..View::default()
        }
    }

    fn draw(ui: &mut Ui, view: &View, width: u16, height: u16) -> (Vec<Hit>, String) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        let mut hits = Vec::new();
        terminal
            .draw(|frame| hits = render(frame, ui, view, "http://127.0.0.1:5180/api/v1"))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        (hits, text)
    }

    fn key(ui: &mut Ui, view: &View, code: KeyCode, tx: &mpsc::Sender<Request>) {
        handle_key(ui, view, KeyEvent::new(code, KeyModifiers::NONE), tx);
    }

    fn click(ui: &mut Ui, view: &View, action: MouseAction, tx: &mpsc::Sender<Request>) {
        let (hits, _) = draw(ui, view, 110, 30);
        let hit = hits.iter().find(|hit| hit.action == action).unwrap();
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.area.x,
            row: hit.area.y,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(ui, view, mouse, &hits, tx);
    }

    #[test]
    fn dashboard_shows_visit_settings_and_camera_together() {
        let view = view(1);
        let (_, text) = draw(&mut Ui::default(), &view, 110, 30);
        for label in [
            "1 Board",
            "2 Events",
            "DARTS",
            "1: —",
            "2: —",
            "3: —",
            "board.name",
            "server.port",
            "Camera 1",
            "+ Add",
            "FPS 30",
        ] {
            assert!(text.contains(label), "missing {label}");
        }
    }

    #[test]
    fn mouse_editing_preserves_revision_and_blocks_background_commands() {
        let mut view = view(1);
        let mut ui = Ui::default();
        let (tx, mut rx) = mpsc::channel(8);
        click(&mut ui, &view, MouseAction::Config(0), &tx);
        let editor = ui.editor.as_mut().unwrap();
        editor.value = "Mouse board".into();
        let revision = editor.revision;
        let (hits, _) = draw(&mut ui, &view, 110, 30);
        assert!(
            !hits
                .iter()
                .any(|hit| hit.action == MouseAction::Key(KeyCode::Char('s')))
        );
        view.error = Some("Disconnected".into());
        key(&mut ui, &view, KeyCode::Enter, &tx);
        assert!(rx.try_recv().is_err());
        assert!(ui.editor.is_some());
        view.error = None;
        click(&mut ui, &view, MouseAction::Key(KeyCode::Enter), &tx);
        let Request::Configure(change) = rx.try_recv().unwrap() else {
            panic!("expected config");
        };
        assert_eq!(change.key, "board.name");
        assert_eq!(change.value, "Mouse board");
        assert_eq!(change.expected_revision, Some(revision));
        view.error = Some("Disconnected".into());
        let (hits, _) = draw(&mut ui, &view, 110, 30);
        assert!(!hits.iter().any(|hit| matches!(
            hit.action,
            MouseAction::Config(_) | MouseAction::CameraField(_, _)
        )));
    }

    #[test]
    fn fourth_camera_scrolls_into_view_and_edit_uses_its_slot() {
        let view = view(4);
        let mut ui = Ui::default();
        let (tx, mut rx) = mpsc::channel(8);
        for _ in 0..9 {
            key(&mut ui, &view, KeyCode::Down, &tx);
        }
        let (_, text) = draw(&mut ui, &view, 80, 24);
        assert!(text.contains("Camera 4"));
        assert!(ui.scroll > 0);
        click(&mut ui, &view, MouseAction::CameraField(3, 4), &tx);
        assert_eq!(ui.editor.as_ref().unwrap().key, "cameras.4.rotation");
        key(&mut ui, &view, KeyCode::Right, &tx);
        key(&mut ui, &view, KeyCode::Enter, &tx);
        let Request::Configure(change) = rx.try_recv().unwrap() else {
            panic!("expected config");
        };
        assert_eq!(change.value, "90");
        assert_eq!(change.key, "cameras.4.rotation");
    }

    #[test]
    fn camera_count_changes_require_save_and_respect_limits() {
        let mut view = view(1);
        let mut ui = Ui::default();
        let (tx, mut rx) = mpsc::channel(8);
        click(&mut ui, &view, MouseAction::Key(KeyCode::Char('+')), &tx);
        assert_eq!(ui.editor.as_ref().unwrap().value, "2");
        assert!(rx.try_recv().is_err());
        key(&mut ui, &view, KeyCode::Esc, &tx);
        assert!(rx.try_recv().is_err());
        key(&mut ui, &view, KeyCode::Char('-'), &tx);
        assert!(ui.editor.is_none());
        view.snapshot.as_mut().unwrap().config.cameras =
            (1..=4).map(oche_domain::CameraConfig::new).collect();
        key(&mut ui, &view, KeyCode::Char('+'), &tx);
        assert!(ui.editor.is_none());
        key(&mut ui, &view, KeyCode::Char('-'), &tx);
        key(&mut ui, &view, KeyCode::Enter, &tx);
        let Request::Configure(change) = rx.try_recv().unwrap() else {
            panic!("expected config");
        };
        assert_eq!(change.key, "cameras.count");
        assert_eq!(change.value, "3");
    }

    #[test]
    fn events_scroll_to_oldest_retained_entry_and_resize_clamps() {
        let mut view = view(1);
        let snapshot = view.snapshot.as_ref().unwrap().clone();
        view.events = (1..=64)
            .map(|sequence| NativeEvent {
                kind: format!("event_{sequence}"),
                board_id: snapshot.config.board.id,
                session_id: snapshot.session_id,
                sequence,
                unix_time_ms: 1000 + sequence,
                snapshot: snapshot.clone(),
            })
            .collect();
        let mut ui = Ui {
            tab: 1,
            ..Ui::default()
        };
        let (tx, _) = mpsc::channel(8);
        let (_, newest) = draw(&mut ui, &view, 80, 24);
        assert!(newest.contains("event_64"));
        key(&mut ui, &view, KeyCode::End, &tx);
        let (_, oldest) = draw(&mut ui, &view, 80, 24);
        assert!(oldest.contains("event_1 "));
        view.events.clear();
        draw(&mut ui, &view, 110, 30);
        assert_eq!(ui.scroll, 0);
    }

    #[test]
    fn disconnected_and_small_terminals_remain_usable() {
        let view = View {
            error: Some("Connection refused".into()),
            ..View::default()
        };
        for (width, height) in [(110, 30), (60, 20), (40, 12)] {
            let (hits, text) = draw(&mut Ui::default(), &view, width, height);
            assert!(text.contains("OcheCore"));
            if width == 110 {
                assert!(text.contains("oche-core run"));
            }
            if width < 60 {
                assert!(hits.is_empty());
            }
        }
    }
}
