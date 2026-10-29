# OcheCore — Product and Engineering Roadmap

Date: 2026-10-29

Status: target specification; native service, CLI/TUI, persistence, and synthetic
demo foundation implemented. Capture, calibration, vision, and verified Autodarts
compatibility remain future work. No real-world performance claims yet.

## 1. Goal

Build a lightweight, fully offline Rust service that turns one to four USB cameras into automatic steel-tip dart scoring. Operators must be able to configure, calibrate, monitor, and control the board from terminal commands or an interactive terminal UI. Existing applications should be able to replace their local Autodarts board endpoint with OcheCore without code changes, within an explicitly tested compatibility profile.

Project name: **OcheCore**. Executable: `oche-core`. Keep the vision engine independent from the Oche game UI.

The release has three equally important requirements: reliable scoring, bounded resource use, and verified API compatibility. A fast detector without correct event behavior is not a drop-in replacement.

## 2. Product requirements

| Area | Required behavior |
| --- | --- |
| Offline operation | Capture, calibration, detection, control, configuration, and event delivery work without Internet, login, license checks, telemetry, or runtime downloads. |
| Language | Rust application and vision pipeline; isolate and document any native dependencies and unsafe boundaries. |
| Camera count | Support 1, 2, 3, or 4 configured cameras, with independent calibration and quality reporting. |
| Terminal operation | Foreground daemon, command-based control, interactive TUI, and remote management on the LAN. |
| Identity | Editable board name and persistent, locally generated board ID. Renaming must not change the ID. |
| Networking | Configurable listen address and port for board control and events; remote target configured separately. |
| Controls | Start, stop, reset current visit, calibrate, inspect status, and wake from standby. |
| Detection | Track up to three darts per visit; detect motion, stabilization, hand presence, partial removal, and completed takeout. |
| Configuration | Camera device selection, resolution, FPS, rotation, lens correction, per-camera calibration, standby timeout, and detection-speed presets. |
| Integration | Versioned Autodarts compatibility adapter plus a separate native API for Oche-specific features. |
| Diagnostics | Actual camera FPS, state, errors, latency, queue drops, geometry residuals, and resource metrics. |

The detector does not need to implement game rules such as 501, cricket, busts, or player rotation. Those belong to consuming applications. Early removal after one or two darts must still work.

## 3. Compatibility scope: establish the contract first

“Same API as Autodarts” must mean the same observable protocol for the target integrations, not merely similarly named endpoints.

Autodarts documentation now distinguishes earlier local Board Manager releases from version 2. The target installed version and the APIs actually used by Oche and other clients must be recorded before the compatibility contract is frozen. Do not assume legacy and current APIs are identical.

### Initial evidence

The operator supplied this help output from the installed executable:

```text
Autodarts headless board
Usage: autodarts [OPTIONS] [SUBCOMMAND]

Options:
  -h,--help                   Print this help message and exit
  -v,--version                Display program version information and exit
  -p,--port PORT              the board's /api port
  -H,--host HOST              manage the board at HOST, not this machine
  -c,--config FILE            config file (default: the user's own)

Subcommands:
  run                         run the board in the foreground
  remote                      manage a board elsewhere on your network
  update                      install the latest release
```

This confirms a useful terminal interaction model, but does not establish the installed version or the HTTP/WebSocket contract. Obtain `autodarts --version` during discovery.

### Contract discovery checklist

- [ ] Record the installed Autodarts version and versions of each integration.
- [ ] Inventory HTTP routes, methods, payloads, status codes, headers, and error bodies used by those clients.
- [ ] Capture WebSocket connection behavior, event subscriptions, envelopes, initial snapshots, event order, and reconnect behavior.
- [ ] Capture startup, stop, reset, calibration success/failure, three throws, early takeout, partial takeout, and camera failure.
- [ ] Record coordinate units, axes, sector numbering, bull/miss encoding, throw ordering, and board-ID format.
- [ ] Check whether camera previews, configuration, discovery, authentication, or heartbeat behavior are required.
- [ ] Save sanitized fixtures and create automated contract tests before implementing the adapter.

A public local-API client provides starting evidence for `PUT /api/start`, `PUT /api/stop`, `POST /api/reset`, and `WS /api/events?type=state`. These are discovery candidates, not a complete or guaranteed contract for the installed version. Inspect the linked source for details rather than guessing missing schemas.

### Compatibility rules

- Preserve exact field names, types, null handling, event labels, and ordering for the chosen profile.
- Keep native Oche extensions outside compatibility payloads unless client tolerance is verified.
- Support polling and streaming paths actually used by the target integrations.
- Advertise Oche identity honestly; document the emulated protocol profile separately.
- Run unmodified target clients against OcheCore in integration tests.
- Track compatibility per client and version: untested, partial, or verified.
- Offline local compatibility does not imply compatibility with cloud authentication, hosted matches, or the Autodarts cloud service.
- Release the compatibility layer as a required milestone, not a deferred enhancement.

## 4. Terminal experience

### Proposed command structure

Mirror the supplied headless CLI where practical. The commands below are proposed Oche commands, not claims about existing Autodarts commands.

```bash
oche-core --help
oche-core --version
oche-core --config ./board.toml --port 3180 run
oche-core --host raspy-darts.local --port 3180 remote

# Non-interactive management
oche-core status --json
oche-core start
oche-core stop
oche-core reset
oche-core wake
oche-core events --json

# Setup and diagnostics
oche-core cameras list --json
oche-core cameras inspect --camera 1
oche-core config validate
oche-core config export
oche-core config set board.name "Basement board"
oche-core calibrate --camera 1
oche-core calibrate --all
oche-core doctor
oche-core record --output ./session
oche-core replay ./session
oche-core benchmark ./session
```

`run` starts the foreground service; `remote` opens the interactive management TUI. Running with no subcommand opens local management or setup and explains how to start a missing service. Do not silently start a second camera-owning process.

`--host` / `-H` selects the board to manage. `--port` / `-p` selects the service port for `run` and the target port for management. `--config` / `-c` selects a config file. Add `--bind` for the service listen address: do not overload `--host` with two meanings.

Commands and the TUI use the same control service. Only the daemon owns camera devices. Emit machine-readable JSON on stdout when requested, diagnostics on stderr, and documented exit codes. Remote management must expose the same board controls and calibration workflow as local management.

Provide an explicit `update` command after release packaging exists. Updates require operator action and may use the Internet; normal operation never does. Also support installing an offline release artifact.

### TUI screens

1. **Overview:** board name/ID, endpoint, engine state, camera health, current three-dart visit, last event, and processing latency.
2. **Cameras:** dynamic rows for camera slots 1–4; device selector, supported resolution/FPS selectors, negotiated format, actual FPS, stopped/running/standby/disconnected/error state.
3. **Calibration:** select camera, edit reference lines/points, lens parameters, inspect residuals, save or cancel.
4. **Motion and detection:** current motion/stability/hand/takeout state, changed area, accepted axes, estimated impacts, and rejected observations.
5. **Configuration:** identity, binding, speed preset, standby, camera assignments, and configuration validation.
6. **Events and diagnostics:** event history, reconnects, frame drops, errors, and replay controls.

Use keyboard-selectable lists as the terminal equivalent of dropdowns. Basic control must work over ordinary SSH. For visual calibration, support optional terminal image protocols and an offline frame export/import workflow. Export an annotated image with pixel coordinates, accept reference coordinates through commands or TUI fields, and show a verification overlay. Do not make a browser or image-capable terminal mandatory.

## 5. Camera setup and calibration

### Camera configuration

Allow an explicit active camera count of 1–4. Each slot has a stable logical ID, selected physical device, resolution, capture FPS, pixel format, rotation, optional crop, lens profile, and calibration status.

Enumerate valid device modes instead of offering arbitrary combinations. Show requested and negotiated modes separately. Avoid silently substituting an unsupported resolution or FPS.

For the current hardware, multiple devices share the same USB serial number and each exposes multiple video nodes. Prefer USB topology/device paths, identify actual capture nodes, and warn when an assignment changes. Never assume `/dev/video0` remains the same camera after reboot.

### Calibration layers

1. **Lens calibration:** estimate intrinsics and distortion from a calibration target or load a saved profile. Support enabling/disabling correction per camera.
2. **Board mapping:** use known wire intersections and reference lines to map image observations onto the board plane. Support manual line-by-line editing and later automatic assistance.
3. **Multi-camera consistency:** place known test points across the board and verify that independent views agree.
4. **Verification:** show reprojection residuals and independent holdout-point errors; reject degenerate geometry.

Version calibration data and bind it to device identity, resolution, rotation, crop, focus settings, and lens parameters. Invalidate or explicitly revalidate affected calibration after changes. Activate a new calibration atomically only after validation; retain the previous valid calibration for rollback.

## 6. Detection design for 1–4 cameras

### Shared pipeline

1. Capture timestamped frames into bounded, reusable buffers.
2. Monitor a low-resolution luminance image inside a calibrated region.
3. Compare against a stable reference and track motion over time.
4. When the scene stabilizes, inspect selected full-resolution frames around the change.
5. Extract candidate dart features and robustly fit the visible shaft/needle axis.
6. Convert observations into board-plane constraints and fuse usable views.
7. Estimate impact uncertainty, map to score, and emit one accepted throw event.
8. Update the per-view stable reference only after acceptance; retain an empty-board reference for removal checks.

Correct distortion before straight-line geometric reasoning, either on selected regions or feature coordinates. Do not assume distorted image lines remain straight. Avoid rectifying every full frame continuously.

### Camera-count behavior

| Cameras | Intended method | Limitation |
| --- | --- | --- |
| 1 | Locate the actual visible entry point and map it onto the board plane; use temporal changes and local shape evidence. | A shaft axis alone does not uniquely determine the impact. A hidden tip may require abstention/manual correction. |
| 2 | Combine two well-conditioned axis constraints or matched visible tip observations. | Occlusion or near-parallel board-plane constraints can make the result uncertain. |
| 3 | Fuse observations robustly and reject inconsistent views. | Calibration and placement still limit precision. |
| 4 | Use additional views to improve coverage and redundancy when they add useful geometry. | More USB/decode work; a poor fourth view must not reduce an otherwise good estimate. |

Do not promise that four cameras always outperform three. Weight observations by calibration quality, feature visibility, residuals, and geometric conditioning. Report usable view count and disagreement. Require corroborating views when available; never apply a universal two-camera requirement that makes one-camera mode unusable.

When a camera fails, enter an explicit degraded mode only if the remaining calibrated configuration is supported. Publish the health change. Do not silently keep claiming the configured accuracy.

### Visit and removal state machine

Keep service lifecycle, scene interpretation, and visit count as separate state domains.

- Service: stopped, starting, running, calibrating, standby, fault.
- Scene: waiting, motion, stabilizing, dart candidate, hand/obstruction, takeout, partial takeout, empty.
- Visit: zero, one, two, or three accepted throws.

A fourth apparent arrival must not silently overwrite a three-dart visit. Support manual corrections and explicit visit reset. Reset clears the logical visit but must not pretend physically present darts have disappeared. Early takeout after one or two darts is valid. Takeout finishes only after a stable empty-board check; partial removal preserves a truthful physical occupancy state.

Handle bounced darts, falling darts, lighting changes, vibrating shafts, grouped impacts, and hands without publishing duplicate throws. Define and test event semantics for ambiguous cases before freezing the API adapter.

## 7. Low-resource architecture

### Proposed Rust workspace

| Component | Responsibility |
| --- | --- |
| `oche-domain` | Board identity, coordinates, scoring geometry, commands, states, and native events. |
| `oche-capture` | V4L2 devices, buffers, timestamping, format negotiation, health, and reconnect. |
| `oche-calibration` | Lens models, board mapping, validation, and persisted calibration. |
| `oche-vision` | Change masks, feature extraction, robust axis fitting, impact estimation, and uncertainty. |
| `oche-engine` | Scheduling, scene/visit state machines, reference management, and orchestration. |
| `oche-api` | Native control/events and isolated Autodarts compatibility adapters. |
| `oche-cli` | CLI and TUI client. |
| `oche-replay` | Recorded-session playback, labels, benchmarks, and regression evaluation. |

Favor Rust implementations for the continuous hot path. Evaluate native dependencies by measured benefit. OpenCV may be used behind a narrow interface for calibration if justified; calling it from Rust does not remove memory-safety risks inside native code. Keep application-level unsafe code small, reviewed, and documented. Select exact crates and versions during the initial technical spike.

### Scheduling and memory rules

- Dedicated capture workers; bounded processing workers; an async runtime for networking/control rather than CPU-heavy image work.
- Latest-frame selection for live processing, plus a small bounded ring for event analysis.
- Preserve a stable pre-change reference independently of the short ring buffer.
- Reuse images and masks; avoid allocation, image cloning, and logging per pixel/frame.
- Do not retain driver-owned buffers indefinitely or starve camera capture while processing.
- Bound subscriber queues and recording storage; slow clients cannot block scoring.
- Use monotonic timestamps for timing and wall-clock timestamps for display.
- Commodity USB cameras are not assumed hardware-synchronized. Match stable observations within a measured age/skew bound and reject stale views.
- Precompute reusable geometric transforms; optimize with SIMD only after profiling.
- Camera previews are on demand, rate-limited, and disabled when no client needs them.

### Capture format experiments

Benchmark YUYV versus MJPEG on the target USB layout. YUYV allows direct luminance access but uses more USB bandwidth; MJPEG reduces transfer bandwidth but requires decoding. Skipping vision work does not automatically avoid capture or decode costs. Measure those separately, including one through four cameras sharing a hub.

### Detection-speed presets

Expose **Very low, Low, Default, High, Very high**. Treat these as scheduling/latency presets, not different score geometries or undocumented accuracy settings.

Candidate initial monitoring rates are 5, 10, 15, 20, and 30 analyses per second per camera, capped by actual capture FPS. These are tuning starting points, not established operating requirements. Stability checks use elapsed time rather than fixed frame counts. Publish the resolved settings and measure missed changes/false events at each preset before shipping defaults.

### Standby

Offer **5, 10, 15, 30 minutes, 1 hour, and disabled**. Define inactivity as absence of meaningful board interaction or explicit keep-awake control; API status polling must not keep the cameras awake indefinitely.

Standby releases or suspends capture according to verified driver behavior. The control API remains available. Explicit start/wake resumes capture, rechecks calibration validity and physical board contents, and establishes stable references before accepting new throws. Fully stopped cameras cannot detect a wake-up dart: automatic visual wake requires a separate low-rate capture mode and should be labeled accordingly.

## 8. Identity, persistence, and API ownership

One daemon instance owns one board. Multiple boards initially use separate instances, configurations, IDs, and ports. External apps can aggregate them by stable board ID without adding multi-board capture complexity to the first release.

Persist configuration and calibration using versioned schemas and atomic file replacement. Generate the board ID once during initialization; preserve it across restart, rename, and upgrade. Regeneration must be an explicit command. Importing a configuration onto another board must offer a deliberate preserve/new-ID choice. Compatibility profiles may require a specific ID representation; verify before selecting the native format.

Use a single command authority so simultaneous CLI, TUI, and API requests cannot race start/reset/calibrate operations. Native events should include board ID, session ID, sequence number, timestamps, state revision, and relevant throw data. Map these to the exact compatibility schema rather than adding arbitrary fields there. Native clients receive an initial snapshot and can recover after reconnect; emulate compatibility-client behavior separately.

Default to loopback binding. LAN exposure is explicit. Keep native authentication options configurable, and determine compatibility authentication behavior from real clients rather than imposing an incompatible handshake.

## 9. Delivery milestones

Effort depends on captured footage, target hardware, and protocol discovery. Use exit criteria rather than promising calendar dates before these are available.

### M0 — Hardware and protocol discovery

- [ ] Record CPU model, RAM, architecture, OS, camera modes, USB topology, and installed Autodarts version.
- [ ] Capture representative raw sessions and independently label impacts and events.
- [ ] Discover target integration contracts and create sanitized fixtures.
- [ ] Benchmark basic capture/decode for 1–4 cameras.
- [ ] Decide initial native dependencies and document unsafe/FFI boundaries.

**Exit:** reproducible hardware report, baseline resource measurements, compatibility matrix, and a replay corpus. Obtain access to a fourth camera before claiming tested four-camera support.

### M1 — Service, CLI, and compatibility scaffold

- [x] Implement Rust workspace, configuration, board identity, foreground service, and bounded native event delivery.
- [x] Implement terminal status/start/stop/reset and remote TUI with configuration editing.
- [ ] Build native API and Autodarts adapter against synthetic/replayed events.
- [ ] Validate the supplied CLI interaction model and port-conflict diagnostics.

**Exit:** an unmodified target integration can connect, receive a synthetic three-dart visit, observe takeout, and reconnect correctly. This milestone does not claim live scoring.

### M2 — Camera acquisition and calibration tools

- [ ] Dynamic camera slots, device selectors, valid mode lists, and health reporting.
- [ ] Stable device assignment and duplicate-serial handling.
- [ ] Per-camera lens correction, line/point editing, verification, persistence, and rollback.
- [ ] Offline calibration workflow usable from ordinary SSH.

**Exit:** restart preserves assignments and identity; independent holdout points satisfy an agreed calibration error budget across the board.

### M3 — Detection baseline and replay

- [ ] Change detection, stability, candidate extraction, and robust line fitting.
- [ ] Implement native-resolution feature inspection and uncertainty reporting.
- [ ] Implement one-camera visible-entry-point baseline with explicit abstention.
- [ ] Implement two-view constraints and three/four-view robust fusion.
- [ ] Integrate live and recorded sources through the same engine interface.

**Exit:** annotated replay results for each camera count, including grouped darts and wire-adjacent impacts. Geometry is tested against independent physical labels, not its own predictions.

### M4 — Complete visit lifecycle and real integrations

- [ ] Three-dart visits, early/partial takeout, hand obstruction, falling/bounced darts, reset, and corrections.
- [ ] Exact event ordering and snapshots through compatibility adapters.
- [ ] Speed presets, standby/wake, and degraded-camera behavior.
- [ ] Validate Oche and selected third-party clients unchanged.

**Exit:** complete real visits run through target clients with no known duplicate throws or false takeout completion in the release corpus; document remaining ambiguous cases.

### M5 — Performance and operational reliability

- [ ] Profile capture, decode, monitoring, event analysis, previews, and API delivery separately.
- [ ] Tune buffers, copies, worker counts, and presets using the target ARM64 machine.
- [ ] Test USB contention, unplug/replug, slow subscribers, full recording storage, and configuration errors.
- [ ] Conduct a 24-hour soak with repeated replay and camera reconnects.
- [ ] Verify no-network operation from process start through calibration, scoring, and restart.

**Exit:** agreed CPU/RAM/latency budgets met, bounded queues and memory, no unexplained memory-growth trend, and successful offline operation.

### M6 — Release and installation

- [ ] Linux ARM64 and x86-64 release artifacts, checksums, reproducible build instructions, and dependency inventory.
- [ ] systemd service and optional container packaging with explicit camera access.
- [ ] Config migration, backup/restore, offline installation, controlled update, and rollback instructions.
- [ ] Publish CLI reference, calibration guide, compatibility matrix, known limitations, and benchmark results.

**Exit:** a clean machine can be installed, configured, calibrated, and operated using documented terminal workflows.

## 10. Measurement and release gates

| Metric | How to evaluate |
| --- | --- |
| Position error | Median and p95 millimeter error against independent labels, stratified by board region and camera count. |
| Score accuracy | Correct sector/multiplier, with wire-adjacent impacts reported separately. |
| Coverage | Fraction automatically accepted versus abstained/manual cases; do not inflate accuracy by hiding abstentions. |
| Event correctness | Duplicate throws, missed throws, false throws, false/unfinished takeouts, and removal detection latency. |
| Latency | Impact-to-event and stable-scene-to-event p50/p95/p99; state the event timing reference. |
| Resources | Idle, monitoring, event burst, preview, recording, and standby CPU/RSS; document whether CPU percentages mean one core or the whole system. |
| Capture health | Delivered FPS, dropped frames, frame age/skew, decode time, and USB failures. |
| Power | Measure wall power if available; do not equate CPU percentage directly with watts. |
| Compatibility | Contract fixtures plus unchanged clients tested per version/profile. |
| Reliability | Soak results, recovery behavior, queue bounds, and restart identity persistence. |

Set numerical release thresholds after M0 on the actual hardware. Require the same fixed evaluation corpus for changes, plus held-out sessions under different lighting and dart placements. Synthetic tests validate geometry and protocol edge cases but cannot establish real-world scoring accuracy.

## 11. Later enhancements

- Assisted automatic board calibration and calibration-drift detection.
- Small local model invoked only for ambiguous crops, if recordings show a measurable benefit.
- Better one-camera tip detection, with separate published accuracy/coverage results.
- Additional camera backends and optional hardware-accelerated decode where beneficial.
- Multi-board supervisor, discovery, and aggregate monitoring without changing per-board identity.
- Optional web visualizer consuming the native API; terminal operation remains complete.

## 12. Decisions to resolve during M0

1. Exact target Autodarts version and named integrations requiring drop-in behavior.
2. CPU/RAM and camera modes of `raspy-darts`, including available USB bandwidth.
3. Whether one-camera mode must prioritize acceptance coverage or conservative abstention.
4. Initial calibration workflow and acceptable manual setup effort.
5. Physical measurement method for ground-truth impacts near wires.
6. Resource and latency budgets for each supported camera count.

## 13. Reference material

These sources guide discovery; none substitutes for testing the installed instance.

- [Autodarts Desktop documentation](https://docs.autodarts.com/getting-started/detection/autodarts-desktop/) — distinguishes earlier Board Manager releases from version 2.
- [Legacy Board Manager states and controls](https://autodarts.diy/how-to-play/BoardManager/) — operator-visible state vocabulary.
- [Legacy Board Manager navigation and configuration](https://autodarts.diy/how-to-play/navigation-tab/) — camera, motion, distortion, and configuration features.
- [Local API client source](https://github.com/tnolle/autodarts-local-api/blob/main/main.py) — concrete integration evidence for selected HTTP/WebSocket operations; validate against the installed version.
- [Linux V4L2 memory-mapped streaming](https://cdn.kernel.org/doc/html/latest/userspace-api/media/v4l/mmap.html) — capture-buffer lifecycle and I/O design.

Implementation proceeds in two tracks: native service, terminal controls, and
release packaging can be verified without recordings; hardware and protocol
discovery supplies evidence for later compatibility, calibration, and vision.
M1 still requires its unchanged-client compatibility exit. Build and measure
vision against real recordings when the camera installation is available.
