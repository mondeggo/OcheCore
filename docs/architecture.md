# Control foundation decisions

Date: 2026-10-29

Controls and terminal interfaces can be implemented and verified before recordings
are available. Hardware discovery gates capture/calibration, scoring validation,
and compatibility claims; it does not gate the native control foundation.

- **Rust 1.90.0 / edition 2024:** pinned toolchain and committed lockfile. Builds
  use `--locked`; runtime performs no downloads.
- **Four crates:** domain owns validated configuration/wire types; engine owns
  persistence/serialized state; API owns HTTP/SSE; CLI owns both terminal clients.
  Future vision backends feed the engine independently of terminal/network code.
- **Axum/Tokio:** bounded admission and one command mutex. Persistent file I/O
  runs on blocking workers. Subscriber streams never hold the engine lock.
- **Native port 5180:** separate from Autodarts' 3180, with a nod to the maximum
  three-dart score of 180. Selected on 2026-10-29 after checking the
  [IANA service registry](https://www.iana.org/assignments/service-names-port-numbers/service-names-port-numbers.csv),
  where 5180 was unassigned. Saved ports and explicit overrides remain supported.
- **Ratatui/Crossterm:** keyboard navigation, responsive network worker, bounded
  request channel, and terminal restoration. Mouse capture routes clicks through
  the same commands and configuration editor as the keyboard. Click targets are
  rebuilt from each rendered layout; modal editors block background clicks and
  disconnected controls cannot submit changes. Image support can be added later.
- **TOML/UUID/atomic files:** schema version 1, board ID generated once, validated
  atomic replacement with file/directory sync, and a separate lifetime lock file.
  Native UUIDs do not predetermine a future compatibility adapter's representation.
- **Explicit initialization:** `init` (also `config init`) collects validated
  settings before saving; `--defaults` enables unattended provisioning. `run`
  requires an existing config. `config reset` uses the same exclusive lock and
  atomic save, requires confirmation or `--yes`, and deliberately generates a
  new board ID. Cancelling or invalid input cannot partially replace settings.
- **Explicit demo mode:** every throw is synthetic. Normal mode reports unknown
  occupancy and unavailable capture/calibration/vision. Camera modes are requested
  configuration, not verified hardware capabilities.
- **Static musl releases:** Linux AMD64/ARM64 executables without a host glibc
  requirement. The non-root Docker image packages the tested executable and
  license notices with a writable data volume.
- **Native boundaries:** workspace crates forbid unsafe Rust. Dependencies still
  call OS APIs for networking, terminal control, random IDs, and file locking.
  `fs2` and `tempfile` isolate locking/atomic-file operations. Musl is the release
  C runtime; there is no OpenCV, camera SDK, or decoder FFI yet.

Standby changes the control lifecycle only; speed presets have no processing
effect until capture/vision exists. No Autodarts adapter is advertised before
observed contracts and unchanged-client tests exist. These limits are visible
in both the UI and API.

## Oche terminal integration reference

Inspection of Oche's published `main` branch on 2026-10-29 shows that its
[service wrapper](https://github.com/mondeggo/oche/blob/8706ad2e0c42f5877a21f170a5b548941477ee70/app/services/autodarts.py)
starts `autodarts run`. Its process manager stops that subprocess using SIGTERM,
with a forced kill after a timeout. Its
[terminal wrapper](https://github.com/mondeggo/oche/blob/8706ad2e0c42f5877a21f170a5b548941477ee70/app/services/autodarts_terminal.py)
launches `autodarts remote -H 127.0.0.1` in a separate PTY. The browser's
xterm.js terminal forwards input through a WebSocket, including mouse reports
when the TUI enables them.

OcheCore follows the same daemon/client separation with `oche-core run` and
`oche-core remote -H 127.0.0.1 --port 5180`. Its TUI Start/Stop commands change
board activity inside the running daemon; a supervisor controls the process
itself. This documents the integration pattern; Oche's Autodarts-specific routes
have not been changed to launch OcheCore.
