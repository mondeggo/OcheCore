# Using OcheCore

The daemon (`oche-core run`) owns the cameras. The command line and the terminal
UI are clients of it; they share one serialized command authority. See the
[native API](native-api.md) for the wire contract.

## Commands

```sh
oche-core status --json
oche-core start
oche-core stop
oche-core reset
oche-core wake
oche-core events --json
oche-core events --json --count 5
oche-core doctor
oche-core init                        # interactive setup; never overwrites config
oche-core config init                 # alias for init
oche-core init --defaults             # unattended setup with default values
oche-core init --defaults --name "Basement" --bind 127.0.0.1 --port 5180
oche-core config reset                # confirm replacement, then run setup again
oche-core config validate             # local TOML validation
oche-core config export               # connected daemon's configuration
oche-core config set board.name "Basement board"
oche-core config set detection.speed low
oche-core config set detection.standby_seconds 1800
oche-core config set cameras.count 3
oche-core config set cameras.1.device /dev/v4l/by-path/REPLACE_WITH_DEVICE
oche-core config set cameras.1.width 1280
oche-core config set cameras.1.height 720
oche-core config set cameras.1.fps 30
oche-core config set cameras.1.rotation 90
oche-core correct 1 D20              # correct an existing first dart
oche-core accuracy                    # detected darts kept, corrected, uncertain
oche-core accuracy --reset            # start the accuracy count again
oche-core cameras available --json    # devices/modes on the daemon host
oche-core cameras list --json
oche-core cameras inspect --camera 1
oche-core cameras snapshot --camera 1 --out cam1.jpg   # save a frame
oche-core orient                      # record where the 20 is (one dart)
oche-core calibrate --all             # empty board; asks for the dart the first time
```

`config set` validates and atomically saves through the daemon; board ID is read
only. `init`, `config reset`, and `config validate` are local file operations.
Camera count is 1–4 in the configuration, but scoring needs at least two cameras,
so setup in the terminal and browser asks for two to four. A third or fourth
camera is welcome: it adds views for the fusion. Decreasing the count removes the
highest slots. Assigned device paths must be unique.

To start fresh, stop the daemon with Ctrl+C (or stop its container), run
`oche-core config reset`, then `oche-core run`. Reset replaces all configuration
settings and generates a **new board ID**. It refuses while the configuration is
in use. Cancelling setup leaves the previous file untouched; the validated result
is saved atomically at the end. For an unattended reset, use
`oche-core config reset --yes --defaults`. To change just the port while keeping
your board identity, use `oche-core config set server.port 5180` with the daemon
running, then restart it. The separate `oche-core reset` command only resets the
logical dart visit.

### Accuracy count

Every dart the cameras report counts once: **right**, **corrected** while you
have changed it to a different score, or **uncertain** (`?`, counted against the
detector even after you fill it in). Accuracy is right darts out of all of them,
halves rounded up. Correcting a dart back to the detector's score, or to the same
score, does not count as a correction. Demo darts are synthetic and never count.
The count is saved in `board.accuracy.json` beside the configuration and
survives restarts. It starts again from zero with `oche-core accuracy --reset`,
**Reset count** in the terminal UI, **Reset** in Settings → Accuracy, or a full
`oche-core config reset` (which deletes the file; a new board ID never inherits
an old count). Darts on the board at a reset are left out of the new count. It
reflects your own corrections, so a dart nobody corrected counts as right; it
is not an independent accuracy measurement.

The setup listen address belongs to this board: loopback permits local clients,
and `0.0.0.0` listens on all IPv4 interfaces. `--host` selects a daemon to manage;
it is not a persisted listen setting.
Linux camera discovery queries devices and advertised modes without starting capture.
Only discrete integer FPS values are selectable today; fractional rates and ranged
modes are reported as limitations. Advertised modes do not prove capture works.

Speed presets: `very_low`, `low`, `default`, `high`, `very_high`: 3, 5, 10, 15 and
30 analysed frames per second per camera, never more than the camera's FPS.
Capture always runs at the configured FPS; the preset only sets how many frames
are decoded and compared. A 1280x720 decode took about 17 ms on one Raspberry Pi 4
core (measured 2026-10-30), so `default` with three cameras costs roughly half a
core. Whether higher speeds detect darts better or faster has not been measured. Standby values: `0` (disabled), `300`, `600`,
`900`, `1800`, or `3600` seconds; standby closes the cameras and polling does not
keep the service awake. `calibrate --camera N` refits one camera against the board
already measured; `calibrate --all` measures the board's wire radii and fits every
assigned camera. Both need a running board, an empty and lit board, and the 20
recorded by `orient`.

To watch what the daemon does, run it with `--log`: `oche-core run --log info`
prints every native event as it happens (the same events as `oche-core events`,
with the reason when a dart cannot be placed), and `--log debug` adds what the
detector sees: moving and still scenes, changed pixels per camera, whether a dart
line was found, the fused impact, and abstentions. Without `--log` the daemon
stays quiet. `oche-core run --help` lists the options.

To see why a dart was or was not placed, add `--debug-images DIR`: each arrival
saves `arrival-NNNN-cameraK.png` per camera into `DIR` (the newest 30 arrivals are
kept), showing the frame with the newly covered pixels (the ones the line is fitted to;
darts already on the board are excluded even if the new dart moved them) in red, the
fitted dart line in cyan, and the tip in green when a flight in view settled it. The files are
uncompressed PNGs of the full frame, so keep `DIR` outside the repository.

JSON goes to stdout; errors/diagnostics go to stderr. Exit codes: `0` success,
`1` operational error, `2` command-line usage error. Ctrl-C/SIGTERM stops the daemon.

## Browser UI

While the daemon runs, open <http://127.0.0.1:5180/> (or the address and port it
listens on). The pages are built into the binary and talk only to this daemon, so
they work offline. They show the daemon's own state: changes made in the terminal
UI or with commands appear in the browser, and the other way round.

The top bar links **Live**, **Settings** and **Debug**. Setup is not in it: it
opens by itself on the first run, and later from **Run setup again** in Settings.
While setup runs, only its steps are shown; once the board is set up, **Exit
setup** leads back to Settings.

- **Live** (`/`): the current visit's three darts and total on a board (Start/Stop
  and Reset are in the status bar), and a feed of game events headed by the
  [accuracy count](#accuracy-count) (hover for the details). **Hide** in the
  events panel gives the board the whole width, and **Game events** in the top
  right corner brings the panel back; each browser remembers the choice. The drawn board
  uses the calibration's measured ring radii, as scoring does (WDF nominal ones
  before calibration). Until a live board has a device for every
  camera and a calibration for each, it opens setup instead. The switch under the
  score row, flush with the total, shows the view in use (a board icon for the drawn
  board, a camera icon and number for a camera); hover over it, or tap it, to
  pick another. It shows either the drawn board or a calibrated camera's picture
  flattened through its calibration: face-on with the 20 up, refreshed every few seconds
  and whenever the visit changes. Cameras that are not calibrated or not
  streaming are greyed out and say why. Darts are drawn where the detector placed
  them (at the middle of their segment when it could not). To correct a dart on
  Live, Test or Debug, click it in the strip, then click where it really is on
  the board or on the camera picture; with **Save corrected darts** on in
  Settings → Diagnostics, each correction is saved for analysis (see
  [Native API](native-api.md)).
- **Settings** (`/settings/`): the terminal UI's settings. **Board** (its name)
  and **Network** (listen address and port), each saved with **Save**; each
  camera's device, state and calibration; and the detection speed, standby time,
  resolution and frame rate, each saved when chosen. **Change** and **Calibrate**
  open those setup steps. A saved address waits for a restart, and Network says so
  until then. In Docker the address is greyed out: the port mapping decides it.
  **Accuracy** shows the [accuracy count](#accuracy-count) and when it started;
  **Reset** asks before starting it again.
  **Backup** exports the settings as JSON (as `oche-core config export --json`)
  and imports such a file after listing the changes it makes; the board keeps its
  identity and calibration, and in Docker its address.
- **Setup** (`/setup/`), four steps:
  1. **Board:** name, listen address and port (the address applies after a restart).
  2. **Cameras:** a device, rotation and live preview per camera (two to four), and
     the resolution, FPS, detection speed and standby time for all of them. Devices
     are named as in the terminal UI (name and USB port). Each change is saved at
     once, and choosing a device starts the board so its preview appears.
  3. **Calibrate:** each camera shows a still picture (the refresh button takes a
     new one). **On screen:** with the board empty, **Fit the wires**; a camera
     with no 20 reference yet starts from a guess near the top of its picture.
     The fitted wires are drawn in white with the 20 in red, 6 in blue, 3 in
     orange and 11 in purple. Turn each camera's colours with its arrows until they
     sit on those numbers, then **Save**, which moves that camera's 20 reference
     and refits it. The arrows turn two segments at a time: calibration takes the
     20 to be a segment with a red double, so only every other segment can be it.
     **Dart at D20** works like `oche-core orient`, then calibrates. **Start over**
     repeats the steps; the saved calibration is kept until a new one succeeds.
  4. **Test:** throw a few darts and check the scores. In demo mode, buttons and
     clicks on the board inject throws.
- **Debug** (`/debug/`): service commands, demo throws, the saved configuration
  as config keys, and the raw event log. The Visit panel switches between the
  board and **Cameras**: a still picture from each camera, retaken only after a
  dart, a takeout, the board becoming ready or a calibration, or with **Take
  pictures**, so the cameras are not streamed to the browser.

Every page's status bar shows the board state with Start/Stop, Reset and (live
mode) Calibrate buttons beside it, then the board name and the address OcheCore
listens on (not on phones), and on the right each camera's FPS (`off`
when not capturing), OcheCore's CPU (share of one core) and memory, the mode and
the version. The GitHub mark in the header opens the repository.

Rotation turns the preview only; frames, hints and calibration stay in camera
pixels. Changing a camera's device clears its 20 reference. A resolution with the
same aspect ratio scales the reference, and any other resolution clears it.
Removing a camera moves later cameras up one slot, together with their settings
and 20 reference. The on-screen method and the arrows have been tested on
synthetic frames only, not yet on real captures.

## Terminal UI

Two screens: **Board** and **Events**. Board begins with board/detection settings,
then three editable dart slots and compact camera rows. The permanent status,
help, and control panels have been removed. Errors and save feedback appear only
when needed.

Click an accepted dart to correct its label (`S1`–`S20`, `D1`–`D20`, `T1`–`T20`,
`SB`, `DB`, or `MISS`). Empty slots cannot invent throws. Corrections preserve
physical occupancy and synthetic provenance, and reject stale visits/restarts.
A `*` marks corrected darts. A `?` dart was seen but could not be placed; the reason
and what each camera saw are listed under the dart slots.

The screen has five sections, top to bottom:

1. **Board settings:** name, IP address and port (saved ports and addresses take
   effect after a restart).
2. **Darts:** the three slots of the current visit, notes on any unplaced dart,
   a one-line detection summary, and the [accuracy count](#accuracy-count) with
   a **Reset count** button that asks before resetting.
3. **Controls:** **Start / Stop / Reset / Wake**, the board **State**, and a
   **Doing** line with an elapsed timer while a command runs (for example
   "Calibrating all cameras; keep the board empty and still").
4. **Cameras:** each row shows the camera's name and USB port (`▼` marks a
   dropdown) with a **Calibrate** button. **+ Add** / **- Remove last** change the
   camera count (two to four: scoring needs two) after a review; **Calibrate
   all** sits below.
5. **Detection settings:** detection speed, standby time, and the resolution and FPS
   applied to every assigned camera (they show `mixed` when the cameras differ).
   Each is a dropdown.

Choices appear in clickable lists; arrows or the wheel select and Save applies.
Devices assigned to other slots are excluded, including aliases. Rotation,
exposure and the 20 reference are not on the screen; use `oche-core config set`
and `oche-core orient`.

Missing devices, permissions, or mode data are reported when opening a picker.
Discovery runs on the daemon machine, refreshes every five seconds, and does not
block the control worker. There are no guessed fallback camera capabilities.

### Board state

The **State** line under the control buttons (and `oche-core status`) reports what
the board is doing:

| State | Meaning |
| --- | --- |
| `STOPPED` | The board is stopped and the cameras are closed. Press Start. |
| `STANDBY` | Standby closed the cameras after inactivity. Press Wake. |
| `STARTING` | Waiting for a camera to deliver frames, or for the first still scene. |
| `NEEDS SETUP` | A camera has no device, or the 20 has not been recorded yet. |
| `NEEDS CALIBRATION` | Cameras are streaming but not calibrated (at least two are needed to score). |
| `MOTION` | The cameras see movement; the scene is waiting to settle. |
| `READY` | Calibrated and still; the board is empty or holds darts that count. |
| `TAKING OUT` | Darts are being removed; the visit ends when the board is empty. |
| `TAKE OUT DARTS` | Three darts are accepted; remove them. |
| `ERROR` | A camera failed; the line names the camera and the cause. |
| `CONTROLS ONLY` | Running without cameras (`--no-capture`). |

Events retains up to 64 native events with sequence/time, session/board IDs,
state, revision, scene, occupancy, and visit. Use `oche-core events --json` for
full payloads or an ongoing diagnostic recording.

| Key | Action |
| --- | --- |
| Tab / Shift-Tab, or 1 / 2 | Switch Board / Events |
| s / x / r / w | Start / stop / reset visit / wake |
| c / o | Calibrate all (asks for the dart first time) / record the 20 again |
| a | Reset the accuracy count (asks first) |
| Up / Down | Select setting, dart, or camera; scroll in Events |
| PageUp / PageDown, Home / End | Scroll |
| + / - | Add camera / remove last |
| Left / Right | Select camera field or dropdown value |
| Enter / Escape | Edit or save / cancel |
| Ctrl-U | Clear text while editing |
| t / o | Synthetic T20 / full takeout, demo only |
| q / Ctrl-C | Close UI |

The daemon is launched with `oche-core run`; the TUI is opened separately with
`oche-core remote`. Start/Stop inside the TUI controls board activity, while
closing the TUI leaves the daemon running.

Edits check the state revision to detect concurrent changes. Keyboard input stays
responsive during network failures. Stale data is labeled disconnected and
controls are disabled until connectivity returns.

How detection and calibration work is described in
[architecture](architecture.md); how well they work on real throws is not yet
established (see [discovery](discovery.md)).

## Try a visit without cameras

Start `oche-core run --demo`, then use the UI or another terminal:

```sh
oche-core start
oche-core demo throw T20
oche-core demo throw D20
oche-core demo throw SB
oche-core demo takeout --remaining 1
oche-core demo takeout
```

Labels: `S1`–`S20`, `D1`–`D20`, `T1`–`T20`, `SB`, `DB`, `MISS`. Every throw is
marked synthetic. A fourth dart is rejected; early and partial takeout work.
Reset clears the logical visit while preserving simulated physical occupancy.
Only configuration/identity persist across restart; visit state is in memory.
