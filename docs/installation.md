# Installation

Three ways to run OcheCore on a Linux board computer: the installer script (a
released binary or a Docker container), a build from source, or Docker Compose
by hand. All of them need no Internet after installation.

## Installer script

```sh
curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/install.sh | bash
```

This downloads the latest release for amd64 or arm64, checks its SHA-256, installs
`oche-core` to `~/.local/bin` (or `/usr/local/bin` as root), offers to put your
user in the `video` group (needed to open cameras), runs the first-time setup
questions, and can start the service at boot.

To run it in Docker instead, add `--docker`:

```sh
curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/install.sh | bash -s -- --docker
```

That installs Docker if it is missing, pulls the release image
(`ghcr.io/mondeggo/ochecore`), and starts it with Compose in `~/ochecore`, with the
cameras found at install time passed through. Use `./oche-core remote` in that
folder to reach the terminal UI. Re-run the script after plugging in cameras.

| Option | Meaning |
| --- | --- |
| `--docker` | Run the release image with Compose instead of installing a binary |
| `--version v0.1.0` | Install a specific release (default: latest) |
| `--dir DIR` | Docker: install folder (default `~/ochecore`) |
| `--port N` | Docker: host port to publish (default 5180) |
| `--lan` | Docker: publish on all interfaces (plain HTTP, no login; see [networking](networking.md)) |
| `--no-system` | Do not change groups or services |
| `-y`, `--yes` | Accept the default answer to every question |
| `--help` | Show usage |

Run the script again to update; settings are kept. It needs a published
[release](https://github.com/mondeggo/ochecore/releases). Docker image downloads
without a login also need the GHCR package to be public (see [releases](releases.md)).

## Build from source

Install Rust with [rustup](https://rustup.rs/). This repository pins Rust 1.90.0,
rustfmt, and Clippy. Development builds need a Linux C linker (for example,
`build-essential` on Debian/Ubuntu). No OpenCV or camera SDK is needed.

```sh
cargo build --locked --release -p oche-cli
./target/release/oche-core run
```

In another terminal, run `./target/release/oche-core remote`. Running without a
subcommand also opens the TUI. Closing the UI leaves the daemon running. If the
daemon is missing, the UI explains how to start it and reconnects automatically.

`init` asks for a board name, listen IP address, port, and camera count. Press Enter
to accept each default: `My dartboard`, `127.0.0.1`, `5180`, and one camera slot.
It creates `$XDG_CONFIG_HOME/ochecore/board.toml`, or
`~/.config/ochecore/board.toml`. On first use, `oche-core run` explains that no
configuration exists, opens the setup prompts, and starts the daemon immediately
after saving. No separate `init` command or second `run` is needed. Without an
interactive terminal, it logs that defaults are being saved and continues.
Existing configurations are reused unchanged. `init` remains available when you
want to configure a board without starting it. Use `--config FILE` for a separate
board. Remote management does not require a local configuration. Board ID
survives rename/restart; session ID changes on restart. A file lock prevents two
daemons from owning one configuration.

## Docker Compose

```sh
mkdir -p data
export OCHECORE_UID=$(id -u) OCHECORE_GID=$(id -g)
docker compose up --build -d
docker compose exec ochecore oche-core remote
docker compose exec ochecore oche-core status --json
docker compose down
```

Compose mounts the host folder `./data` at `/data` and publishes port 5180 on loopback.
On an empty data folder, the image explains that configuration is missing, runs
setup, then starts the daemon automatically. Detached `docker compose up -d`
creates default settings without waiting for input. With an interactive terminal,
setup asks for the board name, listen address, port, and camera count:

```sh
docker compose run --build --service-ports --rm ochecore
```

That command continues running the daemon after the prompts; no second command
is needed. Do not also start the normal Compose service at the same time.
Existing configuration is reused without prompts or replacement. Invalid
configuration still reports an error instead of resetting board identity.
The image's default command is `run --init-bind 0.0.0.0`: the initial address
allows container networking and is only used when creating configuration.
Subsequent starts honor the saved address. Explicit `--bind` / `--port` remain
runtime overrides. Keep container port 5180 unless you also update the mapping
and healthcheck; `OCHECORE_PORT` changes only the published host port.
Set `OCHECORE_DATA_DIR` to use another existing folder. The UID/GID variables above
run the container as your host user so it can write to that folder; keep them set
when running Compose, or save their numeric values in `.env`. Without overrides,
the container runs as UID/GID 65532 and the folder must be writable by that user.
Existing installations using the `board-data` named volume must copy their saved
configuration into the host folder before switching to preserve board identity.
Set `OCHECORE_PORT=5181` to coexist with a service already using that port. Set
`OCHECORE_BIND=0.0.0.0` explicitly for trusted LAN access.

Published releases provide an AMD64/ARM64 image. Replace OWNER, REPOSITORY, and
the version with the GitHub repository and release tag:

```sh
export OCHECORE_IMAGE=ghcr.io/OWNER/REPOSITORY:vX.Y.Z
docker compose up -d --no-build
```

The final image contains a static executable and notices, runs as UID/GID 65532,
and needs no shell, compiler, or Internet. Cameras need the device mapping below. `/data` must be
writable; the rest of the filesystem can be read only. `docker compose down -v`
does not delete the host data folder. For discovery, explicitly map devices with
`--device /dev/videoN` (Compose `devices`) and grant the container user the device
group using `--group-add` / `group_add`. Only accessible mapped devices can be
queried or captured; privileged mode is unnecessary. `scripts/install.sh --docker`
writes a Compose file that does this for the cameras it finds.
