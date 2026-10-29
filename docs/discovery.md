# M0 discovery and implementation inputs

This checklist turns the [roadmap](../oche-core-roadmap.md) into collectable
inputs. An empty item means unknown, not unsupported. Record dates, software
versions, and the source of evidence when filling it in.

## Current evidence

As of 2026-10-29:

- The repository implements the native service, CLI, TUI, configuration, identity,
  event stream, and synthetic demo mode. Recordings are not a prerequisite for
  this foundation. Capture, calibration, vision, and verified Autodarts
  compatibility remain later work.
- The roadmap includes supplied Autodarts CLI help. The operator-provided HTTP
  host was reachable during discovery; its Oche Supervisor page and
  `GET /autodarts/status` both reported Autodarts **2.0.2**, installed and running
  in headless mode. This is a supervisor report; direct binary version output
  and the board HTTP/WebSocket contract still need verification.
- The roadmap refers to `raspy-darts` and cameras with duplicate serial numbers;
  no hardware inventory or connection details have been verified here.
- Rust 1.90.0, Cargo, rustfmt, and Clippy are installed on the development host;
  the repository pins the toolchain and commits its resolved dependencies.
- Linux device/mode discovery is implemented using query-only V4L2 calls. This
  development host has no video devices; dropdown tests use explicitly synthetic
  fixtures. Verification on the board hardware is still needed.
- Board host inventory (operator's Raspberry Pi, read locally on 2026-10-29):
  Raspberry Pi 4 Model B Rev 1.1, Debian 13 (trixie), kernel 6.18.50+rpt-rpi-v8
  aarch64, 3.7 GiB RAM, 4 cores. Three cameras report the card name
  "Autodarts DIY Cam" and sit on USB ports 1.1, 1.2, and 1.4 of one 480 Mbit/s
  hub. Each exposes `video-index0` (capture) and `video-index1` under
  `/dev/v4l/by-path`. `v4l2-ctl --list-formats-ext` lists only MJPG, at
  1920x1080, 1280x1024, 1280x960, 1280x720, 848x480, and 800x600 with 10–30 FPS,
  plus 160x120. Controls include manual exposure (2–1250, units of 100 µs),
  gain, and auto/manual white balance at 2800–6500 K.
- Capture observations on that host, 1280x720 MJPG at 30 FPS on all three
  cameras with `v4l2-ctl` streaming: delivered 29.98 FPS per camera with
  kernel-monotonic start-of-exposure timestamps. Writing all three streams
  (about 5.4 MB/s) directly to the SD card lost about 11% of frames in
  simultaneous bursts; recording into RAM (`/dev/shm`) lost none in 5 s and 90 s
  sessions. The SD card is the suspected cause; this is not yet measured.
- Under the board's LED light ring, every manual white-balance temperature
  tested (4600, 5500, 6500 K) produced a magenta or orange cast, and turning
  auto white balance off reverts to the stored manual temperature instead of
  holding the auto result. Auto white balance with manual exposure 100 and gain 64
  gave neutral images. Control values reset when a camera re-enumerates, so the
  capture service must apply them at start.
- Recordings and human labels exist outside Git (see below). No calibration
  data, accuracy measurements, or third-party compatibility test results are
  present.

## Inputs from the operator

| Input | What to supply or establish | Why it is needed |
| --- | --- | --- |
| Board access | Target host, SSH user/access method, or locally collected reports; do not commit credentials | Inventory and test the actual installation |
| Autodarts version | Exact output of `autodarts --version` and installation source | Select the protocol profile |
| Client integrations | Names, versions/commits, source or reproducible client setup, and connection configuration | Identify the API behavior that must remain compatible |
| Hardware | CPU, RAM, architecture, OS/kernel, camera count/models, supported modes, and USB topology | Choose capture settings and measure resource limits |
| Camera data | Timestamped sessions for each available camera count, capture settings, and independent labels | Develop and evaluate calibration, detection, and removal |
| Product priorities | One-camera coverage versus abstention, acceptable manual calibration effort, and desired latency/resource limits | Resolve tradeoffs and set measured release gates |

HTTP access is established. SSH access, the target machine's inventory, and the
installed Oche source revision remain unknown. The host address and raw responses
are intentionally kept out of version control;
the evidence above records the relevant paths and sanitized observations. Only
HTTP GET requests were used; no service controls were invoked.

Board access can replace manually collecting many of these items. Use only the
available camera count in test claims; four-camera verification requires four
physical cameras.

## Hardware and software inventory

Run available read-only commands on the board machine and save a sanitized report
outside Git first. Missing commands are discovery gaps; do not interrupt the
running installation just to gather this report.

```sh
autodarts --version
uname -a
cat /etc/os-release
lscpu
free -h
lsusb
lsusb -t
v4l2-ctl --list-devices
ls -l /dev/v4l/by-id /dev/v4l/by-path
```

For each actual capture node found by enumeration, inspect supported formats:

```sh
v4l2-ctl --device /dev/videoN --list-formats-ext
```

Replace `/dev/videoN` with the enumerated node; it is a placeholder, not a stable
camera assignment. Record persistent topology paths, actual capture capabilities,
requested/negotiated modes, and shared USB links. Plan capture/decode benchmarks
when the operator can release the devices from the current service.

- [ ] Save a reproducible hardware/software report.
- [ ] Benchmark YUYV and MJPEG capture/decode for the available camera counts.
- [ ] Record delivered FPS, CPU/RSS, drops, frame age/skew, and USB failures.
- [x] Choose and pin the Rust toolchain and initial dependencies; document FFI.
  See [engineering decisions](architecture.md).

## Compatibility evidence

The roadmap's candidate routes are discovery leads. Verify the installed service
and client code before treating any route or payload as a contract.

| Integration | Version/commit | Autodarts profile | Fixtures | Verification |
| --- | --- | --- | --- | --- |
| Oche | Unknown | Autodarts 2.0.2 reported by Supervisor; contract pending | Missing | Untested |
| Additional clients | To identify | Unknown | Missing | Untested |

- [ ] Inventory routes, methods, headers, payloads, status codes, and errors.
- [ ] Capture WebSocket handshake, subscriptions, initial state, event order,
  disconnection, and reconnect behavior.
- [ ] Capture start, stop, reset, calibration success/failure, three throws,
  early/partial/completed takeout, and camera failure.
- [ ] Establish coordinates, score encoding, board ID, null semantics,
  authentication, previews, discovery, and heartbeat requirements.
- [ ] Sanitize captures and document provenance before adding fixtures to Git.
- [ ] Create contract tests and a reproducible unchanged-client integration run.

## Replay corpus and calibration

Keep bulk footage under ignored `recordings/` or in external storage. Each session
needs camera identities/settings, timestamps and synchronization limits, calibration
version, lighting/placement notes, and independently measured impact/event labels.

Sessions recorded on 2026-10-29 with all three cameras (1280x720 MJPG 30 FPS),
stored outside Git with per-frame timestamp logs and a `meta.txt` of devices,
controls, and lighting:

| Session | Content | Labels |
| --- | --- | --- |
| `first-visit` | Light ring off, several visits and takeouts, ~11% dropped frames | None; debug only |
| `empty-calib-ring-on` | 5 s empty board, fixed 5500 K (magenta cast) | Not applicable |
| `visit1-labelled` | Four visits incl. bounce-outs with re-throws, a dart outside the scoring area, and a flight occluding one camera; fixed 5500 K | Thrower's visit labels typed after the session; singles do not record inner/outer |
| `empty-calib-autowb` | 5 s empty board, auto white balance | Not applicable |

First automatic calibration (2026-10-29, `oche-replay calibrate` on frame 100 of
`empty-calib-autowb`, WDF nominal radii, no lens model, hints at the printed 20):

| Camera | Anchors | Fit RMS / max | Trebles vs doubles-only fit, RMS / max |
| --- | --- | --- | --- |
| 1 | 40 | 1.29 / 2.56 mm | 1.90 / 3.27 mm |
| 2 | 40 | 1.42 / 2.99 mm | 2.25 / 3.78 mm |
| 3 | 40 | 1.24 / 2.76 mm | 1.86 / 3.48 mm |

All three overlays place S20 next to the printed 20. Residuals are mostly a
radial bias repeated on every camera: double-band middles map about 0.8 mm
outside their nominal radius and treble-band middles about 1.3 mm inside, with
about 0.7 mm scatter. Either the installed board's ring radii differ from the
WDF nominal values or colour erosion differs between the rings. Measuring the
board will separate the two. These are fit statistics against detected anchors,
not independent physical holdout points or scoring accuracy.

First exploratory dart analysis (2026-10-29, `oche-replay darts` on
`visit1-labelled` with the calibration above). Still periods common to all
cameras separate arrivals; each arrival's change mask is grouped, its principal
axis mapped to a board line, and lines fused (sigma 2 mm, minimum angle 10°,
residual limit 3 sigma):

| Visit | Thrower's labels | Fused result (cameras used) |
| --- | --- | --- |
| 1 | T1, 11, 20 | T1 (3), S11 (2), S20 (3) |
| 2 | 18, 20, T1 | S18 (3), S20 (3), T1 (2) |
| 3 | miss, 8, miss picked up, 5 | MISS 186 mm below centre (2), S8 (3), abstained ill-conditioned then reversed by pickup, S5 (3) |
| 4 | 13, miss picked up, 25, 1 | S13 (3), no change seen, SB (2), S1 (3) |

All 12 fused results agree with the labels; the bounce-out abstained. Where
three cameras were fused, lines agreed within 0.1–1.9 mm. This is not an
accuracy result: the session is small, it was used while developing the method
(grouping of mask pieces was added after inspecting its failures), labels for
singles do not record the ring, and no impact lies near a wire. Earlier
cameras-off-axis failures in the 20/5 area came from fitting only the flight.

Held-out check (2026-10-29, `visit2-heldout`, 180 s, auto white balance). Labels
with inner/outer rings were written before analysis; commit `d996f16` ran
unchanged with the `empty-calib-autowb` calibration. Only visits 1–6 fit in the
recording; visits 7–8 were thrown after it ended.

| Visit | Labels | Fused result |
| --- | --- | --- |
| 1 | T1, T10, 25 | T1, T10, SB (1.1 mm from the bull wire) |
| 2 | 4, 5, T12 | S4, S5, T12 |
| 3 | outer 18, outer 12, D20 | outer S18, outer S12, D20 |
| 4 | T3, outer 19, inner 19 | **inner S3, 3.4 mm inside the treble**, outer S19, inner S19 |
| 5 | outer 13, miss, D13 | outer S13, MISS (181 mm), D13 |
| 6 | inner 15, inner 15, inner 10 | inner S15, inner S15, inner S10 |

17 of 18 recorded darts match including the ring; one T3 was scored as S3, and
nothing abstained. Every fused result used at least two cameras and 16 used
three. The miss lies on the side expected from the known calibration bias
(treble anchors mapping about 1.3 mm inward) and from fitting axes that include
the flight. Eighteen darts from one thrower and board do not establish an
accuracy rate.

Each 180 s three-camera session uses about 1.4 GB; uncompressed debug overlays
add about 2.6 MB per image. Saving overlays filled the 15 GB SD card once, so
keep recordings and debug output on larger external storage.

- [ ] Measure the installed board's double and treble wire diameters.
- [ ] Record more held-out labelled sessions, including grouped darts and
  partial takeout.
- [ ] Record empty board, single throws, three-dart visits, grouped darts,
  wire-adjacent impacts, hands, early/partial/full takeout, and lighting changes.
- [ ] Include bounced/falling darts, vibrating shafts, and occlusion where possible.
- [ ] Define a physical labeling method and label uncertainty, particularly at wires.
- [ ] Collect lens/board calibration references and independent holdout points.
- [ ] Keep held-out sessions separate from tuning data.

## Decisions and gates

Resolve these from the collected evidence and operator priorities:

- [ ] Exact compatibility profile and clients required for the first release.
- [ ] One-camera acceptance/abstention policy.
- [ ] Initial manual calibration workflow and acceptable setup effort.
- [ ] Numerical calibration, accuracy, coverage, CPU/RAM, and latency budgets per
  camera count, with a defined measurement method.

M0 exits when the hardware report, baseline measurements, compatibility matrix,
dependency decision, and replay corpus are reproducible. M1's native foundation
is implemented independently; its compatibility exit still requires an unchanged
target client to receive a synthetic visit, observe takeout, and reconnect correctly.
