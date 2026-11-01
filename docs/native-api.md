# Native API v1

This is OcheCore's native protocol, separate from future Autodarts compatibility.
Default address: `127.0.0.1:5180`. HTTP authentication and TLS are not implemented;
expose it only on a trusted network or behind appropriate access controls.

| Method | Path | Behavior |
| --- | --- | --- |
| GET | `/api/v1/status` | Complete current snapshot |
| GET | `/api/v1/cameras/available` | Query devices/modes on the daemon host |
| GET | `/api/v1/cameras/{slot}/snapshot` | Latest JPEG of a streaming camera (live mode) |
| GET | `/api/v1/config` | Current persisted configuration |
| PUT | `/api/v1/config` | Validate, atomically save, and apply one setting |
| POST | `/api/v1/command` | Run a serialized control command |
| GET | `/api/v1/events` | SSE stream with initial snapshot and subsequent events |
| GET | `/api/v1/events/recent` | Up to 64 events, oldest first |

Other `GET` paths serve the browser UI embedded in the binary (`/` and the
`/setup/` and `/debug/` pages, see [Using OcheCore](usage.md#browser-ui)). It uses
only the routes above; unknown paths return the same 404 JSON error.

Commands use JSON, for example `{"command":"start"}`. Available commands:
`start`, `stop`, `reset`, `wake`, `demo_throw` with string `label`, and
`demo_takeout` with integer `remaining`, `calibrate` with optional `camera`, `orient_begin`, and `orient_finish`.
Demo commands require `run --demo`. Success returns the resulting snapshot.

`calibrate` needs live mode, a running board, an empty and lit board, and an
orientation for each camera: `cameras.<slot>.hint` (pixel `x,y` of the 20
segment) or a previous calibration of that slot. Without `camera` it measures
the board's wire radii and fits every assigned camera; with `camera` it refits
that camera against the board already measured. It runs outside the command
lock, rejects a camera whose wire points fit worse than 1 mm RMS, and only then
saves the record atomically (the previous one is kept) and emits
`calibration_updated`. Failures return 409 (not running, not streaming), 400
(no device or hint), or 422 `calibration_failed`; nothing is changed.

`orient_begin` and `orient_finish` record where the 20 is, once, so calibration
can later realign without being told. With the board empty and running,
`orient_begin` remembers each assigned camera's empty frame (it is kept in
memory only). After one dart is thrown at the 20 and has settled,
`orient_finish` finds the changed barrel in every camera, maps both of its ends
onto the board using the empty frame's wire fit, takes the nearest red double
segment (using the end nearer the board centre when the two ends disagree), and
stores `cameras.<slot>.hint` for each camera. It refuses and changes nothing
unless every camera yields a hint that labels as D20. Failures return 409 (not
running, no orientation started), 400, or 422 `calibration_failed` with the
measurement that failed. This procedure is implemented and unit-tested on
synthetic images only; it has not been validated on real captures.

`correct_throw` requires `dart` (1–3), `label`, `expected_revision`, and
`expected_session_id` from a current snapshot. Only existing throws can be
corrected. It preserves occupancy, scene, and `synthetic`, sets `corrected: true`,
and emits `throw_corrected`; stale edits or invalid labels are rejected.

`cameras.<slot>.resolution` accepts `WIDTHxHEIGHT` as an atomic config change.
Discovery returns `devices` (preferred path, aliases, name, modes, optional error)
and warnings. Each mode has pixel format, width, height, and integer FPS choices.
UI choices aggregate modes across formats. Capture requests MJPG at the chosen
size and FPS and reports a negotiated mismatch as a camera error.
`cameras.<slot>.exposure` is `auto` or a manual exposure (2–10000, 100 µs units);
`cameras.<slot>.hint` is `x,y` or empty. Discovery itself does not configure devices.

Queries use a separate single-worker permit and five-second cache, never the
command lock. Scans limit work to 32 nodes, 16 formats, 64 sizes, 32 intervals,
and 256 modes per node, with a two-second budget between ioctls. A stuck driver
can hold that discovery permit, but does not hold the command authority. Ranged
modes and fractional FPS are reported as limitations. Per-device errors and
partial results remain explicit.

Configuration changes use:

```json
{"key":"board.name","value":"Club board","expected_revision":4}
```

`expected_revision` is optional; a mismatch returns 409 without changing state.
Unknown/read-only keys or invalid values return 400. Board ID/schema version are
read only. Listening changes require restart; `restart_required` reports this.
`listening_on` reports the effective listener including command-line overrides.

Snapshots include configuration, API version, session ID, state revision, event
sequence, monotonic uptime, service/scene state, mode, accepted throws, physical
occupancy, capability flags, per-camera status (`off`, `starting`, `streaming`,
`error` with delivered FPS and message), the current calibration record, and `motion`: whether the cameras currently see
movement (not published as an event; read it from the snapshot). Clients derive a
board activity from these fields (stopped, standby, starting, error, needs
setup, needs calibration, motion, ready, taking out, take out darts); the
`oche-domain` crate provides `Snapshot::activity` for this.
Modes: `live` (default `run`: cameras open while running), `control_only`
(`run --no-capture`), and `demo`. Outside live mode occupancy is `null` and scene
is `unknown`. Detection needs at least two calibrated cameras; a throw whose
position cannot be determined is added as `?` with `uncertain: true` for
correction, and an `uncertainty` text saying why (for example, how many usable
dart lines were found and what each camera saw).

## Events and reconnect

SSE events have an event name, ID `<session UUID>:<sequence>`, and a single JSON
`data` line. Envelopes contain `kind`, `board_id`, `session_id`, `sequence`,
`unix_time_ms`, and a complete `snapshot` with the state revision. The first event
is always `snapshot`. Subscription and snapshot are atomic with respect to commands.

Mutation events: `started`, `stopped`, `visit_reset`, `throw_accepted`,
`partial_takeout`, `takeout_completed`, `throw_corrected`, `config_changed`, `standby`,
and in live mode `board_ready`, `throw_uncertain`, `arrival_ignored` (a fourth
dart; the visit is kept), `scene_disturbed`, `cameras_changed` (state or error
changes, not FPS), and `calibration_updated`. Repeated
start/stop requests that do not change state do not duplicate transition events.
Start/wake keeps the service awake; status reads and configuration edits do not.

Keepalives arrive every 10 seconds. Each subscriber has a bounded 64-event window.
On lag, `resync` supplies a current snapshot and stale queued events are skipped.
Reconnect always supplies a fresh snapshot; `Last-Event-ID` does not request replay.
Clients replace state from snapshots and detect changed session IDs. This is not
a durable event log. CLI `events` reconnects automatically; the TUI polls snapshots
and recent history once per second.

Limits: 16 SSE clients, 32 concurrent control tasks, 16 KiB JSON request bodies.
Excess admission load returns 503. Slow subscribers never hold the command lock.
Persistent writes run on blocking workers under the single command authority.
Visit/event history is bounded and held in memory.

Errors contain `code` and `message`. Relevant statuses: 400 invalid request,
404 unknown route, 409 conflict/stale edit, 422 calibration failed, 501
unavailable feature, 503 busy,
500 storage/internal failure. Rejected operations do not publish success events.
Shutdown closes streams and gives HTTP connections at most three seconds to finish.
