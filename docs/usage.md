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
oche-core cameras available --json    # devices/modes on the daemon host
oche-core cameras list --json
oche-core cameras inspect --camera 1
oche-core cameras snapshot --camera 1 --out cam1.jpg   # save a frame
oche-core orient                      # record where the 20 is (one dart)
oche-core calibrate --all             # empty board; asks for the dart the first time
```

`config set` validates and atomically saves through the daemon; board ID is read
only. `init`, `config reset`, and `config validate` are local file operations. Camera count is 1–4;
decreasing it removes the highest slots. Assigned device paths must be unique.

To start fresh, stop the daemon with Ctrl+C (or stop its container), run
`oche-core config reset`, then `oche-core run`. Reset replaces all configuration
settings and generates a **new board ID**. It refuses while the configuration is
in use. Cancelling setup leaves the previous file untouched; the validated result
is saved atomically at the end. For an unattended reset, use
`oche-core config reset --yes --defaults`. To change just the port while keeping
your board identity, use `oche-core config set server.port 5180` with the daemon
running, then restart it. The separate `oche-core reset` command only resets the
logical dart visit.

The setup listen address belongs to this board: loopback permits local clients,
and `0.0.0.0` listens on all IPv4 interfaces. `--host` selects a daemon to manage;
it is not a persisted listen setting.
Linux camera discovery queries devices and advertised modes without starting capture.
Only discrete integer FPS values are selectable today; fractional rates and ranged
modes are reported as limitations. Advertised modes do not prove capture works.

Speed presets: `very_low`, `low`, `default`, `high`, `very_high` (3, 5, 10, 15 and
30 analyses per second per camera). Standby values: `0` (disabled), `300`, `600`,
`900`, `1800`, or `3600` seconds; standby closes the cameras and polling does not
keep the service awake. `calibrate --camera N` refits one camera against the board
already measured; `calibrate --all` measures the board's wire radii and fits every
assigned camera. Both need a running board, an empty and lit board, and the 20
recorded by `orient`.

JSON goes to stdout; errors/diagnostics go to stderr. Exit codes: `0` success,
`1` operational error, `2` command-line usage error. Ctrl-C/SIGTERM stops the daemon.

## Terminal UI

Two screens: **Board** and **Events**. Board begins with board/detection settings,
then three editable dart slots and compact camera rows. The permanent status,
help, and control panels have been removed. Errors and save feedback appear only
when needed.

Click an accepted dart to correct its label (`S1`–`S20`, `D1`–`D20`, `T1`–`T20`,
`SB`, `DB`, or `MISS`). Empty slots cannot invent throws. Corrections preserve
physical occupancy and synthetic provenance, and reject stale visits/restarts.
A `*` marks corrected darts. Live detection is deferred; test corrections in demo.

The first section holds **Start / Stop / Reset / Wake** and the board **State**.
Settings follow in this order: name, IP address, port, detection speed, standby,
and number of cameras. Each camera row shows the camera's name and USB port with a
**Calibrate** button; **Calibrate all**, **Resolution** and **FPS** (applied to
every assigned camera, showing the current values) sit below the cameras. Choices
appear in clickable lists; arrows/wheel select and Save applies. Devices assigned
to other slots are excluded, including aliases. **+ Add** / **- Remove last**
review the new count before saving (one to four). Rotation, exposure and the 20
reference are not on the screen; use `oche-core config set` and `oche-core orient`.

Missing devices, permissions, or mode data are reported when opening a picker.
Discovery runs on the daemon machine, refreshes every five seconds, and does not
block the control worker. There are no guessed fallback camera capabilities.

Events retains up to 64 native events with sequence/time, session/board IDs,
state, revision, scene, occupancy, and visit. Use `oche-core events --json` for
full payloads or an ongoing diagnostic recording.

| Key | Action |
| --- | --- |
| Tab / Shift-Tab, or 1 / 2 | Switch Board / Events |
| s / x / r / w | Start / stop / reset visit / wake |
| c / o | Calibrate all (asks for the dart first time) / record the 20 again |
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

Detection options and their calibration requirements are described in
[the detection proposal](detection-algorithms.md). Which of them is active,
and how well it works on real throws, is not yet established.

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
