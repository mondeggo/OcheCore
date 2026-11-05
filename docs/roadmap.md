# Roadmap

Updated: 2026-11-05 (v0.1.5). This is the remaining plan; what exists is
described in the [README](../README.md), [usage](usage.md),
[architecture](architecture.md) and [native API](native-api.md), and the
evidence behind it in [discovery](discovery.md).

## Goal

A lightweight, fully offline Rust service that turns USB cameras into automatic
steel-tip dart scoring, controlled from the terminal, a browser or other
applications. Existing applications should be able to replace their local
Autodarts board endpoint with OcheCore without code changes, within an explicitly
tested compatibility profile. Game rules (501, cricket, busts, players) belong to
those applications.

The release has three equally important requirements: reliable scoring, bounded
resource use, and verified API compatibility. A fast detector without correct
event behavior is not a drop-in replacement.

## Where things stand

| Area | State |
| --- | --- |
| Service, identity, configuration, native API and events | Implemented |
| CLI, terminal UI, browser UI, installer and manager (`ochecore.sh`) | Implemented |
| Camera discovery and capture (V4L2, MJPEG) | Implemented on the board's three cameras |
| Calibration from the wire grid, orientation of the 20, measured board radii | Implemented; checked on recordings, not against physical measurements |
| Dart detection, multi-camera fusion, takeout, off-board masking | Implemented; replayed on four labelled sessions, no accuracy figures |
| Corrections with saved evidence for analysis | Implemented |
| Autodarts local board state (`/api/state`, `/api/events`) | Implemented from one captured session; contract-tested, not yet run with an unchanged client |
| Lens correction, one-camera scoring, measured performance budgets | Not started |

Scoring needs at least two calibrated cameras (decided 2026-11-02): one camera
sees a dart only as a line along its shaft, so setup asks for two to four.

## Autodarts compatibility

"Same API as Autodarts" must mean the same observable protocol for the target
integrations, not merely similarly named endpoints. Autodarts documentation
distinguishes earlier local Board Manager releases from version 2; the board's
supervisor reported Autodarts 2.0.2 on 2026-10-29, and its local page showed 2.1.1
when the board state was captured on 2026-11-05 (see [discovery](discovery.md)). The installed
headless board offers `run`, `remote` and `update` with `--port`, `--host` and
`--config`, which OcheCore's CLI mirrors.

A public local-API client gives starting evidence for `PUT /api/start`,
`PUT /api/stop`, `POST /api/reset` and `WS /api/events?type=state`. These are
discovery candidates, not a complete or guaranteed contract.

Contract discovery, before writing the adapter:

- [ ] Record the installed Autodarts version and the versions of each integration.
- [ ] Inventory HTTP routes, methods, payloads, status codes, headers and errors
  used by those clients.
- [ ] Capture WebSocket connection behavior, subscriptions, envelopes, initial
  snapshots, event order and reconnects. The state envelope and visit order are
  captured (2026-11-05).
- [ ] Capture start, stop, reset, calibration success/failure, three throws, early
  takeout, partial takeout and camera failure. Three throws, early takeout and
  reset are captured; bulls, misses and bounce-outs are still needed.
- [ ] Record coordinate units, axes, sector numbering, bull/miss encoding, throw
  ordering and board-ID format.
- [ ] Check whether previews, configuration, discovery, authentication or heartbeat
  behavior are required.
- [x] Save sanitized fixtures and automated contract tests before the adapter
  (one session, 2026-11-05).

Rules for the adapter:

- Preserve exact field names, types, null handling, event labels and ordering for
  the chosen profile; keep native extensions out of compatibility payloads unless
  client tolerance is verified.
- Support the polling and streaming paths the target integrations actually use.
- Advertise Oche identity honestly; document the emulated profile separately.
- Run unmodified target clients against OcheCore in integration tests, and track
  compatibility per client and version: untested, partial or verified.
- Offline local compatibility does not imply compatibility with the Autodarts
  cloud, hosted matches or cloud authentication.

## Remaining milestones

Exit criteria rather than dates: progress depends on recordings, hardware and
protocol discovery.

### Discovery (M0)

- [ ] A reproducible hardware/software report of the board machine.
- [ ] Capture and decode benchmarks for one to three cameras (four once a fourth
  camera is available; four-camera support is untested until then).
- [ ] Independently labelled sessions covering grouped darts, wire-adjacent
  impacts, bounced/falling darts, hands, partial takeout, lighting changes and
  movement beside the board.
- [ ] A physical labelling method with uncertainty, particularly at wires.

### Compatibility (M1 exit)

- [x] The Autodarts board state, built against synthetic and replayed events.
- [ ] Oche's local-game parser reading the observed throws (an Oche change).
- [ ] An unmodified target integration connects, receives a synthetic three-dart
  visit, observes takeout and reconnects correctly.

### Calibration (M2 remainder)

- [ ] Lens correction per camera, if recordings show straight-line errors that
  matter.
- [ ] Independent holdout points across the board, checked against an agreed
  calibration error budget.

### Detection (M3 remainder)

- [ ] Accuracy on held-out labelled sessions, per camera count, reported as the
  measurements below.
- [ ] One-camera scoring from a visible entry point, with explicit abstention, if
  wanted later.

### Visit lifecycle and integrations (M4)

- [ ] Hand obstruction, falling and bounced darts handled without duplicate throws.
- [ ] Exact event ordering and snapshots through the compatibility adapter.
- [ ] Degraded-camera behavior when a camera fails: published, never silently
  claiming the configured accuracy.
- [ ] Oche and selected third-party clients validated unchanged.

Exit: real visits run through target clients with no known duplicate throws or
false takeout completion in the release corpus; remaining ambiguous cases are
documented.

### Performance and reliability (M5)

- [ ] Profile capture, decode, monitoring, event analysis, previews and API
  delivery separately on the board machine; tune buffers, workers and presets.
- [ ] Test USB contention, unplug/replug, slow subscribers, full storage and
  configuration errors.
- [ ] A 24-hour soak with repeated replay and camera reconnects.
- [ ] Verify no-network operation from start through calibration, scoring and
  restart.

Exit: agreed CPU/RAM/latency budgets met, bounded queues and memory, no
unexplained memory growth, offline operation confirmed.

### Release (M6 remainder)

- [ ] Offline installation from a release archive, configuration migration and
  rollback instructions.
- [ ] A CLI reference, calibration guide, compatibility matrix, known limitations
  and benchmark results.

## Measurement and release gates

| Metric | How to evaluate |
| --- | --- |
| Position error | Median and p95 millimetre error against independent labels, by board region and camera count |
| Score accuracy | Correct sector and multiplier, wire-adjacent impacts reported separately |
| Coverage | Automatically accepted versus abstained (`?`) or corrected; abstentions are not hidden |
| Event correctness | Duplicate, missed and false throws; false or unfinished takeouts; removal latency |
| Latency | Impact-to-event and stable-scene-to-event p50/p95/p99, with the timing reference stated |
| Resources | CPU/RSS idle, monitoring, event bursts, previews and standby; say whether CPU means one core or the system |
| Capture health | Delivered FPS, dropped frames, frame age/skew, decode time, USB failures |
| Compatibility | Contract fixtures plus unchanged clients per version/profile |
| Reliability | Soak results, recovery, queue bounds, identity across restarts |

Numerical thresholds come from measurements on the actual hardware. Changes are
compared on one fixed evaluation corpus plus held-out sessions under different
lighting and dart placements. Synthetic tests validate geometry and protocol edge
cases; they do not establish real-world accuracy.

## Open decisions

1. The exact Autodarts version and the integrations that need drop-in behavior.
2. Calibration, accuracy, coverage, CPU/RAM and latency budgets per camera count.
3. The physical measurement method for ground truth near wires.

## Later enhancements

- Calibration-drift detection.
- A small local model for ambiguous crops, only if recordings show a measurable
  benefit; licenses and offline bundling reviewed first.
- Additional camera backends and hardware-accelerated decode where measured
  worthwhile.
- A multi-board supervisor with discovery and aggregate monitoring, keeping
  per-board identity.

## References

These guide discovery; none substitutes for testing the installed instance.

- [Autodarts Desktop documentation](https://docs.autodarts.com/getting-started/detection/autodarts-desktop/):
  earlier Board Manager releases versus version 2.
- [Legacy Board Manager states and controls](https://autodarts.diy/how-to-play/BoardManager/):
  operator-visible state vocabulary.
- [Legacy Board Manager navigation and configuration](https://autodarts.diy/how-to-play/navigation-tab/):
  camera, motion, distortion and configuration features.
- [Local API client source](https://github.com/tnolle/autodarts-local-api/blob/main/main.py):
  evidence for selected HTTP/WebSocket operations.
