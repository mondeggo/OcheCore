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
  request channel, and terminal restoration. Two screens group controls and all
  settings in Board, with a separate scrollable Events history. Mouse capture routes clicks through
  the same commands and configuration editor as the keyboard. Click targets are
  rebuilt from each rendered layout; modal editors block background clicks and
  disconnected controls cannot submit changes. Image support can be added later.
- **TOML/UUID/atomic files:** schema version 1, board ID generated once, validated
  atomic replacement with file/directory sync, and a separate lifetime lock file.
  Native UUIDs do not predetermine a future compatibility adapter's representation.
- **Explicit initialization:** `init` (also `config init`) collects validated
  settings before saving; `--defaults` enables unattended provisioning. `run`
  automatically sets up missing configuration: a TTY gets the wizard;
  non-interactive startup saves defaults with an explanatory log,
  then both paths start the daemon. `--init-bind` only seeds new configuration.
  Existing or invalid files are never silently replaced. `config reset` uses
  the same exclusive lock and atomic save, requires confirmation or `--yes`, and deliberately generates a
  new board ID. Cancelling or invalid input cannot partially replace settings.
- **Explicit demo mode:** every throw is synthetic. Normal mode reports unknown
  occupancy and unavailable capture/calibration/vision. Camera modes are requested
  configuration, not verified hardware capabilities.
- **Static musl releases:** Linux AMD64/ARM64 executables without a host glibc
  requirement. The non-root Docker image packages the tested executable and
  license notices with a writable data volume.
- **Camera discovery:** `linux-video` 0.1.1 (MIT) wraps Linux V4L2 ioctls with
  safe Rust APIs. No workspace unsafe code, bindgen, external executable, or native
  library is added. The daemon queries capabilities/formats/sizes/intervals without
  setting formats or streaming. Inspection of the
  [library source](https://github.com/katyo/linux-video-rs) and the
  [kernel interval contract](https://cdn.kernel.org/doc/html/latest/userspace-api/media/v4l/vidioc-enum-frameintervals.html)
  informed this boundary. Prefer by-path, then by-id, then the video node. Worker,
  cache, and iteration bounds are independent of the command lock. Fractional FPS
  and ranged modes are not selectable yet; live hardware verification is pending.
- **Manual corrections:** existing throws only, guarded by session and revision.
  They preserve physical occupancy and synthetic provenance. Device dropdowns and
  corrections share the compact Board screen; debug information stays in Events.
- **Native boundaries:** workspace crates forbid unsafe Rust. Dependencies still
  call OS APIs for networking, terminal control, random IDs, and file locking.
  `fs2` and `tempfile` isolate locking/atomic-file operations; `linux-video`
  isolates the V4L2 discovery ioctl boundary. Musl is the release
  C runtime; there is no OpenCV, camera SDK, or decoder FFI yet.

- **Board geometry (`oche-domain::board`):** millimetres, bull at the origin,
  +y toward 20. Nominal radii come from the
  [WDF approved-equipment specification](https://site.dartswdf.com/rules/approved-by-wdf/):
  double outer edge 170.0 mm, treble 107.4 mm, ring inside width 8.0 mm, bull and
  outer-bull inside diameters 12.7 and 31.8 mm. An older 1995 WDF text gives a
  107.0 mm treble. Boundaries are zero-width circles and radials; every hit
  reports its distance to the nearest wire so callers can treat near-wire
  impacts as ambiguous. The installed board has not been measured yet.
- **Calibration (`oche-calibration`):** per-camera image-to-board homography by
  normalized DLT from named anchors (bull plus radial-wire crossings on the four
  ring boundaries). Rejects fewer than four points, non-finite input, rank
  deficiency, and points on both sides of the vanishing line; reports per-point
  residuals in millimetres for fitted and held-out anchors. The 9x9 symmetric
  eigen solve is a small in-crate Jacobi routine, so no linear-algebra dependency
  was added. Lens distortion is not modelled yet; real residuals will show
  whether it is needed.
- **Line fusion (`oche-vision`):** a shaft mapped through a homography gives a
  board line through the impact, not a point. Lines from at least two distinct
  cameras are intersected by weighted least squares with a covariance ellipse.
  It abstains when lines are too close to parallel. Three lines can detect
  disagreement but cannot attribute it, so a view is rejected only when at
  least three consistent views remain; otherwise it abstains. With exactly
  two lines, agreement is reported as unverified. Angle and residual limits are
  required inputs without defaults until recordings justify values.
- **Anchor detection (`oche-calibration::detect`):** from one empty-board frame,
  segments red/green pixels, bridges radial wires by dilation, identifies the
  double ring, treble ring, and bull as connected regions, and casts 1440 rays
  from the projected centre. Rays through the centre are images of board radii,
  so red/green changes along each band locate the radial wires. Anchors are the
  middles of the double and treble bands at each wire (40 per camera); band
  edges were rejected because colour classification eroded each band to roughly
  half its nominal width on real frames. A pixel near the 20 resolves the
  two-segment colour symmetry. The centre is re-estimated from the fit until it
  moves less than 0.05 px. Thresholds come from one lighting setup.
- **Board measurement (`oche-calibration::refine`):** colour-detected mappings
  are refined against wire centres. Each view is resampled in board polar
  coordinates; ring wires are found as bright luminance ridges beside black
  singles, radial wires as ridges inside the coloured bands. A ridge peak is the
  wire centre, so wire width does not bias the boundary. All views and the ring
  radii are fitted together by Levenberg-Marquardt with outlier trimming; the
  double's outer wire stays at 170 mm to fix scale. The measured radii replace
  the WDF nominal values for scoring. Dart scoring and fusion use shaft centre
  lines from slice edge midpoints, excluding the flight.
- **JPEG decoding (`oche-vision::image`):** `zune-jpeg` 0.5.15 with `zune-core`
  0.5.3 (MIT OR Apache-2.0 OR Zlib), pure Rust. Its SIMD paths use `unsafe`
  for platform intrinsics inside the dependency; workspace crates remain
  `unsafe`-free. Frame sides are capped at 4096 px. Chosen over
  `jpeg-decoder` for active maintenance; decode cost on the Pi is not yet
  measured. Only development tools depend on it so far.
- **Replay tooling (`oche-replay`):** reads discovery recordings (`camN.mjpeg`
  plus `v4l2-ctl --verbose` logs) and provides `info`, `frame`, `probe`, and
  `calibrate` commands. Overlays are written as uncompressed PNG by a small
  in-crate encoder. Development use only; not packaged in releases.
- **Exploratory dart analysis (`oche-replay darts`):** offline only. Motion at
  10 Hz on 4x-downsampled luma finds still periods common to all cameras; five
  averaged frames represent each period. Consecutive periods are differenced,
  pieces within 4 px are grouped, and the largest group's principal axis (flight
  included) becomes a board line. Arrival versus removal compares each change
  with the first, empty, still period. All thresholds are exploratory defaults
  in `darts::Settings`, not validated values. Live processing, partial-takeout
  logic, and robust shaft fitting are not implemented.

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
