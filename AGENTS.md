# Repository instructions

## Scope and source of truth

These instructions apply throughout this repository. Read
`oche-core-roadmap.md` before implementation and `docs/discovery.md` for open
inputs and evidence. The roadmap is a proposed specification, not proof of
implemented behavior. Keep documentation explicit about what is planned,
implemented, and verified.

OcheCore is an offline Rust board service with CLI/TUI control, one to four USB
cameras, a native API, and a versioned Autodarts compatibility adapter. Game rules
and player management belong to consuming applications.

## Implementation order

- Prioritize M0 discovery and the M1 service/compatibility scaffold before advanced
  detection. Independent native service work can proceed while hardware evidence
  is being collected.
- Follow the workspace boundaries in the roadmap. Introduce crates when they have
  a concrete responsibility; avoid empty placeholder implementations.
- Record significant toolchain, dependency, protocol, and geometry decisions with
  their evidence. Do not invent API payloads, compatibility results, hardware
  measurements, accuracy numbers, or release thresholds.
- Keep raw recordings and private captures outside Git. Store small, sanitized
  fixtures with provenance and distinguish synthetic inputs from real captures.

## Engineering invariants

- Normal operation requires no Internet, login, telemetry, or runtime downloads.
- The daemon alone owns cameras. CLI, TUI, and API controls share one serialized
  command authority. Bind to loopback by default; LAN exposure must be explicit.
- Persist board identity across restart and rename. Version configuration and
  calibration data and replace them atomically after validation.
- Separate service, scene, and visit state. Resetting a logical visit must not
  erase physical occupancy. Partial takeout is not an empty board; a fourth
  arrival must not overwrite an accepted three-dart visit.
- Bound queues, frame retention, recording storage, and worker counts. Slow
  subscribers must not block scoring. Use monotonic time for elapsed durations.
- Keep native events and compatibility payloads separate. Implement a compatibility
  profile from observed contracts and test it with unchanged target clients.
- Treat ambiguous geometry and camera failures explicitly. One-camera shaft axes
  alone do not determine an impact. Report uncertainty and abstain when necessary.
- Minimize and document unsafe Rust and native/FFI boundaries. Justify native
  dependencies and performance changes with measurements.

## Development and verification

The control workspace, CLI, TUI, and the browser UI embedded in `oche-api` are
implemented. Capture, calibration, vision, and Autodarts compatibility remain
deferred. Keep the pinned toolchain and
`Cargo.lock` current, and document native build prerequisites. Hardware recordings
are not required for work on native controls or terminal interfaces.

For Rust changes, use the following checks when the workspace supports them:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

For CLI/TUI behavior, build `oche-cli` and run
`python3 scripts/smoke.py --binary target/debug/oche-core`. For container changes,
build the image and run `bash scripts/smoke_container.sh IMAGE`. Release workflows
must package tested binaries and preserve license/dependency notices.

Add feature-specific and hardware checks where relevant. Test observable behavior:
state transitions, score boundaries, persistence, event order, reconnects, slow
subscribers, and calibration rejection. Keep replay labels independent of the
detector's predictions. Synthetic tests do not establish live scoring accuracy.
For documentation-only changes, inspect links and run `git diff --check`; a test
suite is unnecessary. Report skipped checks and missing prerequisites honestly.

## Git workflow

- Use Conventional Commits:
  `<type>[optional scope][!]: <short imperative description>`.
- Typical types are `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`,
  and `chore`. Use scopes such as `domain`, `capture`, `calibration`, `vision`,
  `engine`, `api`, `cli`, or `replay` when useful.
- Mark breaking changes with `!` and explain them in a `BREAKING CHANGE:` footer.
- Keep each commit focused. Include relevant verification in the commit body or
  review description. Never include unrelated changes, credentials, machine-local
  configuration, or raw recordings.
- Use the `codex/` prefix when creating agent branches unless directed otherwise.
- Verify that the Git root is this directory before staging or committing; this
  checkout is nested under another repository. Stage explicit paths.
