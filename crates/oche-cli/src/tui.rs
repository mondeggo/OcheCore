use crate::client::Client;
use anyhow::Result;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use futures_util::StreamExt;
use oche_domain::{
    ActivityLevel, CameraDevice, CameraInventory, CameraState, Command, ConfigChange, Mode,
    NativeEvent, ServiceState, Snapshot, Speed,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

const TABS: [&str; 2] = ["1 Board", "2 Events"];
const BOARD_FIELDS: [&str; 3] = ["board.name", "server.bind", "server.port"];
const BOARD_LABELS: [&str; 3] = ["Name", "IP address", "Port"];
/// Shown in this order under DETECTION SETTINGS; the last two apply to every camera.
const DETECTION_FIELDS: [&str; 4] = [
    "detection.speed",
    "detection.standby_seconds",
    "cameras.*.resolution",
    "cameras.*.fps",
];
const DETECTION_LABELS: [&str; 4] = ["Detection speed", "Standby after (s)", "Resolution", "FPS"];
const DART_START: usize = BOARD_FIELDS.len();
/// Fields with a button on each camera row; resolution and FPS are edited for
/// all cameras at once, the 20 reference by `orient`, and the rest by `config set`.
const CAMERA_VISIBLE: [usize; 2] = [0, 4];
const CAMERA_FIELDS: [&str; 7] = [
    "device",
    "resolution",
    "fps",
    "rotation",
    "calibrate",
    "exposure",
    "hint",
];
/// Manual exposure presets in 100 µs units; "auto" keeps the camera's own.
const EXPOSURES: [&str; 8] = ["auto", "50", "75", "100", "150", "200", "300", "500"];
const CAMERA_START: usize = DART_START + 3;

/// First selectable detection setting, after the camera rows.
fn detection_start(snapshot: &Snapshot) -> usize {
    CAMERA_START + snapshot.config.cameras.len()
}

enum Request {
    Command(Command),
    Configure(ConfigChange),
}

/// A request the daemon is working on, so the screen can say what is happening.
#[derive(Clone)]
struct Busy {
    label: String,
    since: std::time::Instant,
}

fn busy_label(request: &Request) -> String {
    match request {
        Request::Configure(_) => "Saving settings".into(),
        Request::Command(command) => match command {
            Command::Start => "Starting the board and opening the cameras".into(),
            Command::Stop => "Stopping the board".into(),
            Command::Reset => "Resetting the visit".into(),
            Command::Wake => "Waking the board".into(),
            Command::Calibrate { camera: Some(n) } => {
                format!("Calibrating camera {n}; keep the board empty and still")
            }
            Command::Calibrate { camera: None } => {
                "Calibrating all cameras; keep the board empty and still".into()
            }
            Command::OrientBegin => "Remembering the empty board".into(),
            Command::OrientFinish => "Finding the dart in each camera".into(),
            Command::CorrectThrow { .. } => "Correcting the dart".into(),
            Command::DemoThrow { .. } | Command::DemoTakeout { .. } => {
                "Sending a demo input".into()
            }
        },
    }
}

#[derive(Clone, Default)]
struct View {
    busy: Option<Busy>,
    snapshot: Option<Snapshot>,
    events: Vec<NativeEvent>,
    error: Option<String>,
    feedback: String,
    inventory: CameraInventory,
}

struct Editor {
    key: String,
    value: String,
    choices: Vec<String>,
    revision: u64,
    correction: Option<Command>,
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
    /// The board was remembered empty; the next press finds the thrown dart.
    orienting: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MouseAction {
    Tab(usize),
    Key(KeyCode),
    CameraField(usize, usize),
    Config(usize),
    Detection(usize),
    Dart(usize),
    Choice(usize),
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
    let camera_client = client.clone();
    let (camera_tx, mut camera_rx) = watch::channel(CameraInventory::default());
    let _camera_worker = AbortTask(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let inventory = camera_client
                .cameras()
                .await
                .unwrap_or_else(|error| CameraInventory {
                    devices: vec![],
                    warnings: vec![format!("Camera discovery: {error:#}")],
                });
            if camera_tx.send(inventory).is_err() {
                break;
            }
        }
    }));
    let _worker = AbortTask(tokio::spawn(network(client, request_rx, view_tx)));
    let mut input = EventStream::new();
    let mut ui = Ui::default();
    let mut view = View::default();
    let shutdown = crate::shutdown();
    tokio::pin!(shutdown);
    // Redraw twice a second so elapsed times of long operations keep moving.
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        let mut hits = Vec::new();
        terminal.draw(|frame| hits = render(frame, &mut ui, &view, &endpoint))?;
        tokio::select! {
            _ = &mut shutdown => break,
            _ = tick.tick() => {}
            result = view_rx.changed() => {
                if result.is_err() { break; }
                let inventory = std::mem::take(&mut view.inventory);
                view = view_rx.borrow_and_update().clone();
                view.inventory = inventory;
            }
            result = camera_rx.changed() => {
                if result.is_ok() { view.inventory = camera_rx.borrow_and_update().clone(); }
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
                ui.selected = CAMERA_START + camera;
                ui.camera_field = field;
                Some(KeyCode::Enter)
            }
            Some(MouseAction::Dart(index)) => {
                ui.selected = DART_START + index;
                Some(KeyCode::Enter)
            }
            Some(MouseAction::Choice(index)) => {
                if let Some(editor) = &mut ui.editor {
                    editor.value = editor.choices[index].clone();
                }
                None
            }
            Some(MouseAction::Config(index)) => {
                ui.selected = index;
                Some(KeyCode::Enter)
            }
            Some(MouseAction::Detection(index)) => view.snapshot.as_ref().map(|snapshot| {
                ui.selected = detection_start(snapshot) + index;
                KeyCode::Enter
            }),
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
                    view.busy = Some(Busy { label: busy_label(&Request::Command(command.clone())), since: std::time::Instant::now() });
                    let _ = updates.send(view.clone());
                    let result = client.command(&command).await;
                    view.busy = None;
                    view.feedback = match &result { Ok(_) => "Command applied".into(), Err(error) => format!("{error:#}") };
                    // A rejected command does not mean the daemon is disconnected.
                    if result.is_err() { let _ = updates.send(view.clone()); continue; }
                    result
                }
                Some(Request::Configure(change)) => {
                    view.busy = Some(Busy { label: busy_label(&Request::Configure(change.clone())), since: std::time::Instant::now() });
                    let _ = updates.send(view.clone());
                    let result = client.configure(&change).await;
                    view.busy = None;
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
                let request = if let Some(Command::CorrectThrow {
                    dart,
                    expected_revision,
                    expected_session_id,
                    ..
                }) = editor.correction
                {
                    Request::Command(Command::CorrectThrow {
                        dart,
                        label: editor.value,
                        expected_revision,
                        expected_session_id,
                    })
                } else if let Some(field) = editor.key.strip_prefix("cameras.*.") {
                    let mut queued = 0;
                    for camera in view
                        .snapshot
                        .iter()
                        .flat_map(|s| &s.config.cameras)
                        .filter(|c| !c.device.is_empty())
                    {
                        let change = ConfigChange {
                            key: format!("cameras.{}.{field}", camera.slot),
                            value: editor.value.clone(),
                            expected_revision: None,
                        };
                        if tx.try_send(Request::Configure(change)).is_ok() {
                            queued += 1;
                        }
                    }
                    ui.notice = format!("{field} change queued for {queued} cameras");
                    return;
                } else {
                    Request::Configure(ConfigChange {
                        key: editor.key,
                        value: editor.value,
                        expected_revision: Some(editor.revision),
                    })
                };
                send(ui, tx, request);
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
            ui.notice = "Tab: Board/Events | arrows: select | Enter: edit | s/x/r/w: start/stop/reset/wake | c: calibrate | o: find the 20 again | +/-: cameras".into();
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
    let cameras_end = detection_start(snapshot);
    let count = cameras_end + DETECTION_FIELDS.len();
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
        KeyCode::Left if ui.tab == 0 && (CAMERA_START..cameras_end).contains(&ui.selected) => {
            ui.camera_field = step_field(ui.camera_field, false)
        }
        KeyCode::Right if ui.tab == 0 && (CAMERA_START..cameras_end).contains(&ui.selected) => {
            ui.camera_field = step_field(ui.camera_field, true)
        }
        KeyCode::Enter if ui.tab == 0 => {
            if (DART_START..CAMERA_START).contains(&ui.selected) {
                let index = ui.selected - DART_START;
                let Some(dart) = snapshot.throws.get(index) else {
                    ui.notice = "No throw to correct in this slot".into();
                    return;
                };
                ui.editor = Some(Editor {
                    key: format!("Dart {} — S1–S20, D1–D20, T1–T20, SB, DB, MISS", index + 1),
                    value: dart.label.clone(),
                    choices: vec![],
                    revision: snapshot.revision,
                    correction: Some(Command::CorrectThrow {
                        dart: index as u8 + 1,
                        label: dart.label.clone(),
                        expected_revision: snapshot.revision,
                        expected_session_id: snapshot.session_id,
                    }),
                });
                return;
            }
            if (CAMERA_START..cameras_end).contains(&ui.selected) && ui.camera_field == 4 {
                let camera = (ui.selected - CAMERA_START + 1) as u8;
                calibrate_or_orient(ui, snapshot, tx, Some(camera));
                return;
            }
            match editable(snapshot, ui, &view.inventory) {
                Ok((key, value, choices)) => {
                    ui.editor = Some(Editor {
                        key,
                        value,
                        choices,
                        revision: snapshot.revision,
                        correction: None,
                    })
                }
                Err(message) => ui.notice = message,
            }
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
                correction: None,
            });
        }
        KeyCode::Char('s') => send(ui, tx, Request::Command(Command::Start)),
        KeyCode::Char('x') => send(ui, tx, Request::Command(Command::Stop)),
        KeyCode::Char('r') => send(ui, tx, Request::Command(Command::Reset)),
        KeyCode::Char('w') => send(ui, tx, Request::Command(Command::Wake)),
        KeyCode::Char('o') if snapshot.mode == Mode::Live => orient_step(ui, tx),
        KeyCode::Char('c') if snapshot.mode == Mode::Live => {
            calibrate_or_orient(ui, snapshot, tx, None)
        }
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

/// Choices every assigned camera supports, applied to each of them on save.
fn all_cameras_editor(
    snapshot: &Snapshot,
    inventory: &CameraInventory,
    field: &str,
) -> Result<Editor, String> {
    let cameras: Vec<_> = snapshot
        .config
        .cameras
        .iter()
        .filter(|c| !c.device.is_empty())
        .collect();
    if cameras.is_empty() {
        return Err("Assign a device to a camera first".into());
    }
    let mut common: Option<Vec<String>> = None;
    for camera in &cameras {
        let Some(device) = inventory.devices.iter().find(|d| d.matches(&camera.device)) else {
            return Err(format!("camera {} is not in the inventory", camera.slot));
        };
        let own: Vec<String> = device
            .modes
            .iter()
            .filter(|m| m.width <= 8192 && m.height <= 8192)
            .filter(|m| {
                field == "resolution" || (m.width, m.height) == (camera.width, camera.height)
            })
            .flat_map(|m| {
                if field == "resolution" {
                    vec![format!("{}x{}", m.width, m.height)]
                } else {
                    m.fps.iter().map(u32::to_string).collect()
                }
            })
            .collect();
        common = Some(match common {
            None => own,
            Some(previous) => previous.into_iter().filter(|v| own.contains(v)).collect(),
        });
    }
    let mut choices = common.unwrap_or_default();
    if field == "fps" {
        choices.sort_by_key(|v| v.parse::<u32>().unwrap_or(0));
    } else {
        choices.sort();
    }
    choices.dedup();
    let Some(first) = choices.first().cloned() else {
        return Err(format!("No {field} is supported by all cameras"));
    };
    let current = if field == "resolution" {
        format!("{}x{}", cameras[0].width, cameras[0].height)
    } else {
        cameras[0].fps.to_string()
    };
    Ok(Editor {
        key: format!("cameras.*.{field}"),
        value: if choices.contains(&current) {
            current
        } else {
            first
        },
        choices,
        revision: snapshot.revision,
        correction: None,
    })
}

fn step_field(current: usize, forward: bool) -> usize {
    let at = CAMERA_VISIBLE
        .iter()
        .position(|&f| f == current)
        .unwrap_or(0);
    let next = if forward {
        (at + 1) % CAMERA_VISIBLE.len()
    } else {
        (at + CAMERA_VISIBLE.len() - 1) % CAMERA_VISIBLE.len()
    };
    CAMERA_VISIBLE[next]
}

/// One step of finding the 20: remember the empty board, then, after a dart
/// is thrown at the 20, locate it in every camera and store the hints.
fn orient_step(ui: &mut Ui, tx: &mpsc::Sender<Request>) {
    if ui.orienting {
        ui.orienting = false;
        send(ui, tx, Request::Command(Command::OrientFinish));
        ui.notice = "Finding the dart… if the 20 is recorded, press Calibrate all again".into();
    } else {
        ui.orienting = true;
        send(ui, tx, Request::Command(Command::OrientBegin));
        ui.notice = "Board remembered empty. Throw ONE dart at the 20, let it settle, then press Calibrate all again".into();
    }
}

/// Calibration needs to know where the 20 is. The first time (no stored hint
/// and no calibration), or while an orientation is waiting for its dart, the
/// button walks through that; afterwards it only realigns the cameras.
fn calibrate_or_orient(
    ui: &mut Ui,
    snapshot: &Snapshot,
    tx: &mpsc::Sender<Request>,
    camera: Option<u8>,
) {
    if ui.orienting || !snapshot.cameras_needing_orientation().is_empty() {
        orient_step(ui, tx);
    } else {
        send(ui, tx, Request::Command(Command::Calibrate { camera }));
    }
}

fn editable(
    snapshot: &Snapshot,
    ui: &Ui,
    inventory: &CameraInventory,
) -> Result<(String, String, Vec<String>), String> {
    let c = &snapshot.config;
    let cameras_end = detection_start(snapshot);
    if (CAMERA_START..cameras_end).contains(&ui.selected) {
        let camera = &c.cameras[ui.selected - CAMERA_START];
        let field = CAMERA_FIELDS[ui.camera_field];
        let key = format!("cameras.{}.{}", camera.slot, field);
        if field == "hint" {
            // Free text: the pixel x,y of the 20 segment, or empty to clear.
            let value = camera
                .hint
                .map_or(String::new(), |[x, y]| format!("{x},{y}"));
            return Ok((key, value, vec![]));
        }
        let device = inventory
            .devices
            .iter()
            .find(|device| device.matches(&camera.device));
        let mut choices: Vec<String> = match field {
            "device" => std::iter::once(String::new())
                .chain(
                    inventory
                        .devices
                        .iter()
                        .filter(|device| {
                            !c.cameras.iter().any(|other| {
                                other.slot != camera.slot && device.matches(&other.device)
                            })
                        })
                        .map(|device| device.path.clone()),
                )
                .collect(),
            "resolution" => device
                .map(|d| {
                    d.modes
                        .iter()
                        .filter(|m| m.width <= 8192 && m.height <= 8192)
                        .map(|m| format!("{}x{}", m.width, m.height))
                        .collect()
                })
                .unwrap_or_default(),
            "fps" => device
                .map(|d| {
                    d.modes
                        .iter()
                        .filter(|m| m.width == camera.width && m.height == camera.height)
                        .flat_map(|m| m.fps.iter().map(u32::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            "rotation" => vec!["0".into(), "90".into(), "180".into(), "270".into()],
            "exposure" => EXPOSURES.map(String::from).to_vec(),
            _ => vec![],
        };
        if matches!(field, "fps" | "rotation" | "exposure") {
            choices.sort_by_key(|value| value.parse::<u32>().unwrap_or(0));
        } else {
            choices.sort();
        }
        choices.dedup();
        if choices.is_empty()
            || (field == "device" && choices.len() == 1 && camera.device.is_empty())
        {
            return Err(device
                .and_then(|d| d.error.clone())
                .or_else(|| inventory.warnings.first().cloned())
                .unwrap_or_else(|| format!("No available {field} choices reported by the board")));
        }
        let current = match field {
            "device" => camera.device.clone(),
            "resolution" => format!("{}x{}", camera.width, camera.height),
            "fps" => camera.fps.to_string(),
            "exposure" => camera.exposure.map_or("auto".into(), |e| e.to_string()),
            _ => camera.rotation.to_string(),
        };
        let value = if choices.contains(&current) {
            current
        } else {
            choices[0].clone()
        };
        return Ok((key, value, choices));
    }
    let key = match ui.selected.checked_sub(cameras_end) {
        Some(index) => DETECTION_FIELDS[index.min(DETECTION_FIELDS.len() - 1)],
        None => BOARD_FIELDS[ui.selected.min(BOARD_FIELDS.len() - 1)],
    };
    if let Some(field) = key.strip_prefix("cameras.*.") {
        let editor = all_cameras_editor(snapshot, inventory, field)?;
        return Ok((editor.key, editor.value, editor.choices));
    }
    let value = config_value(snapshot, key);
    let choices = match key {
        "detection.speed" => Speed::ALL
            .into_iter()
            .map(|s| s.as_str().to_string())
            .collect(),
        "detection.standby_seconds" => [0, 300, 600, 900, 1800, 3600]
            .map(|v| v.to_string())
            .to_vec(),
        _ => vec![],
    };
    Ok((key.into(), value, choices))
}

fn detection_summary(s: &Snapshot) -> String {
    match s.mode {
        Mode::Demo => "demo: synthetic throws (t / o keys)".into(),
        Mode::ControlOnly => "off: controls only (run without --no-capture)".into(),
        Mode::Live if s.state != ServiceState::Running => "cameras off: start the board (s)".into(),
        Mode::Live if !s.capabilities.calibration => {
            "needs calibration: empty the board, then Calibrate all".into()
        }
        Mode::Live if !s.capabilities.vision => "needs at least two calibrated cameras".into(),
        Mode::Live => format!(
            "live · {} analyses/s per camera · scene {:?}",
            s.config.detection.speed.analysis_hz(),
            s.scene
        ),
    }
}

fn camera_summary(s: &Snapshot, index: usize) -> String {
    let camera = &s.config.cameras[index];
    let state = match s.cameras.iter().find(|c| c.slot == camera.slot) {
        Some(c) => match c.state {
            CameraState::Streaming => format!("streaming {:.1} FPS", c.fps),
            CameraState::Starting => "starting".into(),
            CameraState::Off => "off".into(),
            CameraState::Error => format!("error: {}", c.error.as_deref().unwrap_or("unknown")),
        },
        None => "off".into(),
    };
    let calibration = s
        .calibration
        .as_ref()
        .and_then(|r| r.for_camera(camera))
        .map_or("not calibrated".into(), |c| {
            format!("calibrated, wire fit {:.2} mm", c.wire_rms_mm)
        });
    format!("{state} · {calibration}")
}

/// A camera as the user knows it: its name and the USB port it sits in.
fn device_label(device: &CameraDevice) -> String {
    let port = device
        .aliases
        .iter()
        .chain(std::iter::once(&device.path))
        .find_map(|alias| usb_port(alias));
    match port {
        Some(port) => format!("{} · USB {port}", device.name),
        None => format!("{} · {}", device.name, device.path),
    }
}

/// The `0:1.1` of `.../platform-…-usb-0:1.1:1.0-video-index0`.
fn usb_port(alias: &str) -> Option<String> {
    let rest = alias.rsplit('/').next()?.split("-usb-").nth(1)?;
    let ports = rest.split("-video").next()?;
    Some(ports.strip_suffix(":1.0").unwrap_or(ports).to_string())
}

/// The resolution or frame rate the assigned cameras share, or "mixed".
fn common_mode(s: &Snapshot, resolution: bool) -> String {
    let assigned: Vec<_> = s
        .config
        .cameras
        .iter()
        .filter(|c| !c.device.is_empty())
        .collect();
    let cameras = if assigned.is_empty() {
        s.config.cameras.iter().collect()
    } else {
        assigned
    };
    let values: std::collections::BTreeSet<String> = cameras
        .iter()
        .map(|c| {
            if resolution {
                format!("{}x{}", c.width, c.height)
            } else {
                c.fps.to_string()
            }
        })
        .collect();
    match values.len() {
        1 => values.into_iter().next().unwrap_or_default(),
        _ => "mixed".into(),
    }
}

fn config_value(s: &Snapshot, key: &str) -> String {
    match key {
        "board.name" => s.config.board.name.clone(),
        "detection.speed" => s.config.detection.speed.as_str().into(),
        "detection.standby_seconds" => s.config.detection.standby_seconds.to_string(),
        "server.bind" => s.config.server.bind.to_string(),
        "server.port" => s.config.server.port.to_string(),
        "cameras.*.resolution" => common_mode(s, true),
        "cameras.*.fps" => common_mode(s, false),
        _ => String::new(),
    }
}

fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}

fn render(frame: &mut Frame, ui: &mut Ui, view: &View, _endpoint: &str) -> Vec<Hit> {
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
        Constraint::Length(0),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(if ui.notice.is_empty() && view.feedback.is_empty() {
            0
        } else {
            2
        }),
    ])
    .split(area);
    let connected = view.snapshot.is_some() && view.error.is_none();
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
    let mut darts_row = None;
    let mut controls_row = None;
    let mut calibrate_row = None;
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
        let detection_at = detection_start(s);
        ui.selected = ui.selected.min(detection_at + DETECTION_FIELDS.len() - 1);
        lines.push(Line::styled("BOARD SETTINGS", selection(true)));
        for (index, key) in BOARD_FIELDS.iter().enumerate() {
            if ui.selected == index {
                selected_row = lines.len();
            }
            row_hits.push((lines.len(), MouseAction::Config(index)));
            lines.push(Line::styled(
                format!(
                    "{} {:27} {}",
                    if ui.selected == index { "›" } else { " " },
                    BOARD_LABELS[index],
                    plain(&config_value(s, key))
                ),
                selection(ui.selected == index),
            ));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled("DARTS", selection(true)));
        darts_row = Some(lines.len());
        if (DART_START..CAMERA_START).contains(&ui.selected) {
            selected_row = lines.len();
        }
        lines.push(Line::raw(""));
        // Why the last unplaced dart was uncertain, one fact per line.
        if let Some(note) = s.throws.iter().rev().find_map(|t| t.uncertainty.as_deref()) {
            for (i, part) in note.split("; ").enumerate() {
                lines.push(Line::styled(
                    format!("  {} {}", if i == 0 { "?" } else { " " }, plain(part)),
                    Style::default().fg(Color::Yellow),
                ));
            }
        }
        lines.push(Line::raw(format!(
            "  {:27} {}",
            "detection",
            plain(&detection_summary(s))
        )));
        lines.push(Line::raw(""));
        lines.push(Line::styled("CONTROLS", selection(true)));
        controls_row = Some(lines.len());
        lines.push(Line::raw(""));
        let activity = s.activity();
        let colour = match activity.level {
            ActivityLevel::Ready => Color::Green,
            ActivityLevel::Busy => Color::Yellow,
            ActivityLevel::Attention => Color::LightMagenta,
            ActivityLevel::Problem => Color::LightRed,
            ActivityLevel::Idle => Color::Gray,
        };
        lines.push(Line::from(vec![
            Span::raw("  State: "),
            Span::styled(
                activity.label,
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {}", plain(&activity.detail))),
        ]));
        // What the daemon is doing for a command that takes a while.
        lines.push(match &view.busy {
            Some(busy) => {
                let spin =
                    ["|", "/", "-", "\\"][(busy.since.elapsed().as_millis() / 250 % 4) as usize];
                Line::styled(
                    format!(
                        "  Doing: {spin} {} ({}s)",
                        plain(&busy.label),
                        busy.since.elapsed().as_secs()
                    ),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            }
            None => Line::raw(""),
        });
        lines.push(Line::raw(""));
        camera_heading = Some(lines.len());
        lines.push(Line::styled("CAMERAS", selection(true)));
        lines.push(Line::raw(""));
        for (index, _) in s.config.cameras.iter().enumerate() {
            if ui.selected == CAMERA_START + index {
                selected_row = lines.len();
            }
            camera_rows.push((lines.len(), index));
            lines.push(Line::raw(""));
            lines.push(Line::raw(format!(
                "          {}",
                plain(&camera_summary(s, index))
            )));
            lines.push(Line::raw(""));
        }
        calibrate_row = Some(lines.len());
        lines.push(Line::raw(""));
        lines.push(Line::raw(""));
        lines.push(Line::styled("DETECTION SETTINGS", selection(true)));
        for (index, key) in DETECTION_FIELDS.iter().enumerate() {
            let selected = ui.selected == detection_at + index;
            if selected {
                selected_row = lines.len();
            }
            row_hits.push((lines.len(), MouseAction::Detection(index)));
            lines.push(Line::styled(
                format!(
                    "{} {:27} {} v",
                    if selected { "›" } else { " " },
                    DETECTION_LABELS[index],
                    plain(&config_value(s, key))
                ),
                selection(selected),
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
    if let Some(y) = controls_row.and_then(visible_y) {
        let mut x = content.x + 2;
        for (label, key) in [("Start", 's'), ("Stop", 'x'), ("Reset", 'r'), ("Wake", 'w')] {
            let rect = Rect::new(x, y, label.len() as u16 + 2, 1);
            button(
                frame,
                &mut hits,
                rect,
                label,
                MouseAction::Key(KeyCode::Char(key)),
                connected,
                false,
            );
            x = rect.right() + 1;
        }
    }
    if let Some(y) = calibrate_row.and_then(visible_y) {
        let snapshot = view.snapshot.as_ref().unwrap();
        button(
            frame,
            &mut hits,
            Rect::new(content.x + 10, y, 15, 1),
            "Calibrate all",
            MouseAction::Key(KeyCode::Char('c')),
            connected && snapshot.mode == Mode::Live,
            false,
        );
    }
    if let Some(y) = darts_row.and_then(visible_y) {
        let snapshot = view.snapshot.as_ref().unwrap();
        let width = (content.width / 3).min(22);
        for index in 0..3 {
            let label = snapshot
                .throws
                .get(index)
                .map(|d| {
                    format!(
                        "{}: {}{}",
                        index + 1,
                        d.label,
                        if d.corrected { " *" } else { "" }
                    )
                })
                .unwrap_or_else(|| format!("{}: —", index + 1));
            button(
                frame,
                &mut hits,
                Rect::new(
                    content.x + index as u16 * width,
                    y,
                    width.saturating_sub(1),
                    1,
                ),
                &label,
                MouseAction::Dart(index),
                connected && index < snapshot.throws.len(),
                ui.selected == DART_START + index,
            );
        }
    }
    for (row, index) in camera_rows {
        let snapshot = view.snapshot.as_ref().unwrap();
        let c = &snapshot.config.cameras[index];
        if let Some(y) = visible_y(row) {
            frame.render_widget(
                Paragraph::new(format!("Camera {}", c.slot)),
                Rect::new(content.x, y, 9, 1),
            );
            let label = view
                .inventory
                .devices
                .iter()
                .find(|d| d.matches(&c.device))
                .map(device_label)
                .unwrap_or_else(|| {
                    if c.device.is_empty() {
                        "Select camera".into()
                    } else {
                        c.device.clone()
                    }
                });
            let device_width = (plain(&label).chars().count() as u16 + 4)
                .max(30)
                .min(content.width.saturating_sub(10 + 1 + 11 + 1));
            button(
                frame,
                &mut hits,
                Rect::new(content.x + 10, y, device_width, 1),
                &format!("{} v", plain(&label)),
                MouseAction::CameraField(index, 0),
                connected,
                ui.selected == CAMERA_START + index && ui.camera_field == 0,
            );
            button(
                frame,
                &mut hits,
                Rect::new(content.x + 11 + device_width, y, 11, 1),
                "Calibrate",
                MouseAction::CameraField(index, 4),
                connected && snapshot.mode == Mode::Live,
                ui.selected == CAMERA_START + index && ui.camera_field == 4,
            );
        }
    }
    // Show only feedback, not a permanent control/help panel.
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(plain(&view.feedback)),
            Line::raw(plain(&ui.notice)),
        ]),
        sections[3],
    );
    if let Some(editor) = &ui.editor {
        hits.clear();
        let width = area.width.saturating_sub(4).min(80);
        let list_height = if editor.choices.is_empty() {
            0
        } else {
            editor.choices.len().min(6) as u16
        };
        let popup_height = 7 + list_height;
        let popup = Rect::new(
            (area.width - width) / 2,
            area.height.saturating_sub(popup_height) / 2,
            width,
            popup_height,
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(&*editor.key),
                Line::raw(""),
                Line::styled(
                    format!("{}▏", editor.value),
                    Style::default().fg(Color::Yellow),
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
        if list_height > 0 {
            let selected = editor
                .choices
                .iter()
                .position(|choice| choice == &editor.value)
                .unwrap_or(0);
            let first = selected.saturating_sub(list_height as usize - 1);
            for (row, index) in (first..editor.choices.len())
                .take(list_height as usize)
                .enumerate()
            {
                let value = &editor.choices[index];
                let label = if value.is_empty() {
                    "Unassigned".into()
                } else if editor.key.ends_with(".device") {
                    view.inventory
                        .devices
                        .iter()
                        .find(|d| d.path == *value)
                        .map(device_label)
                        .unwrap_or_else(|| value.clone())
                } else {
                    value.clone()
                };
                button(
                    frame,
                    &mut hits,
                    Rect::new(popup.x + 2, popup.y + 5 + row as u16, popup.width - 4, 1),
                    &plain(&label),
                    MouseAction::Choice(index),
                    true,
                    selected == index,
                );
            }
        }
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
            let rect = Rect::new(x, popup.y + 5 + list_height, label.len() as u16 + 2, 1);
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
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Gray)
            .add_modifier(Modifier::BOLD)
    };
    frame.render_widget(Paragraph::new(format!(" {label} ")).style(style), area);
    if enabled {
        hits.push(Hit { area, action });
    }
}

fn selection(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::Yellow)
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
            "BOARD SETTINGS",
            "Name",
            "IP address",
            "Port",
            "CONTROLS",
            "State:",
            "DETECTION SETTINGS",
            "Detection speed",
            "Standby after",
            "Resolution",
            "FPS",
            "Camera 1",
            "+ Add",
            "CONTROLS",
            "Start",
            "Wake",
        ] {
            assert!(text.contains(label), "missing {label}");
        }
    }

    #[test]
    fn camera_dropdowns_use_only_matching_reported_modes() {
        let mut view = view(2);
        // Synthetic enumeration fixture, not evidence of a physical camera.
        view.inventory.devices.push(oche_domain::CameraDevice {
            path: "/dev/v4l/by-path/test".into(),
            aliases: vec!["/dev/video0".into()],
            name: "Fixture camera".into(),
            modes: vec![
                oche_domain::CameraMode {
                    width: 1280,
                    height: 720,
                    pixel_format: "MJPG".into(),
                    fps: vec![30, 60],
                },
                oche_domain::CameraMode {
                    width: 1920,
                    height: 1080,
                    pixel_format: "MJPG".into(),
                    fps: vec![15],
                },
            ],
            error: None,
        });
        view.snapshot.as_mut().unwrap().config.cameras[0].device = "/dev/video0".into();
        let mut ui = Ui {
            selected: CAMERA_START,
            camera_field: 1,
            ..Ui::default()
        };
        let (_, _, choices) =
            editable(view.snapshot.as_ref().unwrap(), &ui, &view.inventory).unwrap();
        assert_eq!(choices, ["1280x720", "1920x1080"]);
        ui.camera_field = 2;
        let (_, _, choices) =
            editable(view.snapshot.as_ref().unwrap(), &ui, &view.inventory).unwrap();
        assert_eq!(choices, ["30", "60"]);
        ui.selected = CAMERA_START + 1;
        ui.camera_field = 0;
        assert!(editable(view.snapshot.as_ref().unwrap(), &ui, &view.inventory).is_err());
        ui.selected = CAMERA_START;
        ui.camera_field = 1;
        let (tx, mut rx) = mpsc::channel(8);
        key(&mut ui, &view, KeyCode::Enter, &tx);
        click(&mut ui, &view, MouseAction::Choice(1), &tx);
        click(&mut ui, &view, MouseAction::Key(KeyCode::Enter), &tx);
        let Request::Configure(change) = rx.try_recv().unwrap() else {
            panic!("expected config");
        };
        assert_eq!(change.key, "cameras.1.resolution");
        assert_eq!(change.value, "1920x1080");
        let next = view
            .snapshot
            .as_ref()
            .unwrap()
            .config
            .changed(&change)
            .unwrap();
        assert_eq!(
            (next.cameras[0].width, next.cameras[0].height),
            (1920, 1080)
        );
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
        let mut view = view(4);
        // Synthetic enumeration fixture, not evidence of a physical camera.
        view.inventory.devices.push(oche_domain::CameraDevice {
            path: "/dev/v4l/by-path/test".into(),
            aliases: vec![],
            name: "Fixture camera".into(),
            modes: vec![],
            error: None,
        });
        let mut ui = Ui::default();
        let (tx, mut rx) = mpsc::channel(8);
        for _ in 0..12 {
            key(&mut ui, &view, KeyCode::Down, &tx);
        }
        let (_, text) = draw(&mut ui, &view, 80, 24);
        assert!(text.contains("Camera 4"));

        click(&mut ui, &view, MouseAction::CameraField(3, 0), &tx);
        assert_eq!(ui.editor.as_ref().unwrap().key, "cameras.4.device");
        key(&mut ui, &view, KeyCode::Right, &tx);
        key(&mut ui, &view, KeyCode::Enter, &tx);
        let Request::Configure(change) = rx.try_recv().unwrap() else {
            panic!("expected config");
        };
        assert_eq!(change.value, "/dev/v4l/by-path/test");
        assert_eq!(change.key, "cameras.4.device");
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
            assert!(text.contains(if width < 60 { "OcheCore" } else { "Board" }));
            if width == 110 {
                assert!(text.contains("oche-core run"));
            }
            if width < 60 {
                assert!(hits.is_empty());
            }
        }
    }

    #[test]
    fn sections_come_in_the_requested_order_with_dropdown_markers() {
        let view = view(1);
        let (_, text) = draw(&mut Ui::default(), &view, 110, 50);
        let at = |label: &str| {
            text.find(label)
                .unwrap_or_else(|| panic!("missing {label}"))
        };
        assert!(at("BOARD SETTINGS") < at("DARTS"));
        assert!(at("DARTS") < at("CONTROLS"));
        assert!(at("CONTROLS") < at("CAMERAS"));
        assert!(at("CAMERAS") < at("DETECTION SETTINGS"));
        assert!(at("Name") < at("IP address") && at("IP address") < at("Port"));
        assert!(
            at("Detection speed") < at("Standby after") && at("Standby after") < at("Resolution")
        );
        assert!(
            !text.contains("set 20 hint"),
            "the 20 is not set per camera"
        );
        assert!(
            !text.contains("Cameras  "),
            "the camera count is set with + and -"
        );
        assert!(text.contains("default v"), "dropdown rows are marked");
        assert!(text.contains("1280x720 v") && text.contains("30 v"));
    }

    #[test]
    fn camera_names_show_the_usb_port() {
        let device = oche_domain::CameraDevice {
            path: "/dev/video0".into(),
            aliases: vec![
                "/dev/v4l/by-path/platform-fd500000.pcie-pci-0000:01:00.0-usb-0:1.2:1.0-video-index0"
                    .into(),
            ],
            name: "USB Camera".into(),
            modes: vec![],
            error: None,
        };
        assert_eq!(device_label(&device), "USB Camera · USB 0:1.2");
    }

    #[test]
    fn calibrate_asks_for_the_dart_once_then_only_realigns() {
        let mut snapshot = view(2).snapshot.unwrap();
        snapshot.config.cameras[0].device = "/dev/video0".into();
        let mut ui = Ui::default();
        let (tx, mut rx) = mpsc::channel(8);
        assert_eq!(snapshot.cameras_needing_orientation(), [1]);
        calibrate_or_orient(&mut ui, &snapshot, &tx, None);
        assert!(matches!(
            rx.try_recv().unwrap(),
            Request::Command(Command::OrientBegin)
        ));
        assert!(ui.orienting);
        calibrate_or_orient(&mut ui, &snapshot, &tx, None);
        assert!(matches!(
            rx.try_recv().unwrap(),
            Request::Command(Command::OrientFinish)
        ));
        assert!(!ui.orienting);
        // With the 20 stored, the same button just calibrates.
        snapshot.config.cameras[0].hint = Some([100.0, 100.0]);
        calibrate_or_orient(&mut ui, &snapshot, &tx, None);
        assert!(matches!(
            rx.try_recv().unwrap(),
            Request::Command(Command::Calibrate { camera: None })
        ));
    }
}
