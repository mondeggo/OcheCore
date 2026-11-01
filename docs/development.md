# Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --locked -p oche-cli
python3 scripts/smoke.py --binary target/debug/oche-core
```

The smoke test exercises CLI/TUI editing, terminal restoration, events,
persistence, port conflicts, and shutdown with connected clients. It needs Linux,
Python 3, and permission to bind loopback sockets.

The browser UI lives in `crates/oche-api/ui` and is compiled into the binary, so
rebuild `oche-cli` after editing it. A new file must also be listed in
`crates/oche-api/src/ui.rs`. Check scripts with `node --check` and try the pages
against `oche-core run --demo` at <http://127.0.0.1:5180/>; the calibration page
needs a live daemon with cameras.

Run `./clean.sh` to remove local `target/`, `dist/` (including compiled binaries
and release archives), and `scripts/__pycache__/`. Preview with
`./clean.sh --dry-run`. Stop builds before cleaning. Source, configuration,
data, recordings, and Docker resources are preserved; custom Cargo target
directories outside the checkout are untouched.

GitHub Actions checks pushes/PRs. A `vX.Y.Z` tag matching Cargo.toml builds and tests
static Linux AMD64/ARM64 binaries, creates archives/checksums, and publishes a
GitHub Release and matching Docker image. See [release setup](releases.md),
including optional Docker Hub publishing with Oche's credentials convention.

Crates: `oche-domain` (types and board geometry), `oche-capture` (V4L2),
`oche-calibration`, `oche-vision`, `oche-engine` (state, storage, live pipeline),
`oche-api` (native HTTP API and browser UI), `oche-cli` (CLI and TUI), and `oche-replay`
(offline analysis of recordings). See the
[roadmap](../oche-core-roadmap.md), [deferred discovery](discovery.md),
[engineering decisions](architecture.md), and [AGENTS.md](../AGENTS.md).
Use Conventional Commits, such as `feat(cli): add board controls`.
