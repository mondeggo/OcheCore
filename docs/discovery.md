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
- No recordings, ground-truth labels, calibration data, hardware measurements,
  or third-party compatibility test results are present. Native service and
  CLI/TUI integration tests run independently of those inputs.

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
