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

To try the app without a Rust toolchain, `scripts/dev.sh` (or
`scripts\dev.bat` on Windows) builds the image and runs it in demo mode at
<http://localhost:5180>; arguments go to `docker compose`:

```sh
scripts/dev.sh                    # build and start in the foreground
scripts/dev.sh up --build -d      # in the background
scripts/dev.sh logs -f
scripts/dev.sh exec ochecore oche-core demo throw T20
scripts/dev.sh down               # keeps the development data volume
```

It uses a separate `ochecore-dev` Compose project, image and named data volume
(`compose.dev.yaml`), so a production `./data` is never touched, and runs as root
inside the container so the new volume is writable. `OCHECORE_DEV_PORT` and
`OCHECORE_DEV_BIND` override `5180` and `127.0.0.1`. Demo mode is the default
because Docker Desktop cannot pass USB cameras through. Every change to `crates/`
rebuilds the release binary from scratch inside the image, which takes a few
minutes.

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
`oche-autodarts` (Autodarts board state from native events),
`oche-api` (native HTTP API, browser UI and Autodarts routes), `oche-cli` (CLI and TUI), and `oche-replay`
(offline analysis of recordings). See the
[roadmap](roadmap.md), [deferred discovery](discovery.md),
[engineering decisions](architecture.md), and [AGENTS.md](../AGENTS.md).
Use Conventional Commits, such as `feat(cli): add board controls`.
