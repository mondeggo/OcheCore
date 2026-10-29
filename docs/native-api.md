# Native API v1

This is OcheCore's native protocol, separate from future Autodarts compatibility.
Default address: `127.0.0.1:3180`. HTTP authentication and TLS are not implemented;
expose it only on a trusted network or behind appropriate access controls.

| Method | Path | Behavior |
| --- | --- | --- |
| GET | `/api/v1/status` | Complete current snapshot |
| GET | `/api/v1/config` | Current persisted configuration |
| PUT | `/api/v1/config` | Validate, atomically save, and apply one setting |
| POST | `/api/v1/command` | Run a serialized control command |
| GET | `/api/v1/events` | SSE stream with initial snapshot and subsequent events |
| GET | `/api/v1/events/recent` | Up to 64 events, oldest first |

Commands use JSON, for example `{"command":"start"}`. Available commands:
`start`, `stop`, `reset`, `wake`, `demo_throw` with string `label`, and
`demo_takeout` with integer `remaining`. `calibrate` with optional `camera` returns
HTTP 501 until that backend exists. Demo commands require `run --demo`. Success
returns the resulting snapshot.

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
occupancy, and capability flags. In control-only mode, occupancy is `null` and
scene is `unknown`. `running` describes the control lifecycle, not active vision.

## Events and reconnect

SSE events have an event name, ID `<session UUID>:<sequence>`, and a single JSON
`data` line. Envelopes contain `kind`, `board_id`, `session_id`, `sequence`,
`unix_time_ms`, and a complete `snapshot` with the state revision. The first event
is always `snapshot`. Subscription and snapshot are atomic with respect to commands.

Mutation events: `started`, `stopped`, `visit_reset`, `throw_accepted`,
`partial_takeout`, `takeout_completed`, `config_changed`, `standby`. Repeated
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
404 unknown route, 409 conflict/stale edit, 501 unavailable feature, 503 busy,
500 storage/internal failure. Rejected operations do not publish success events.
Shutdown closes streams and gives HTTP connections at most three seconds to finish.
