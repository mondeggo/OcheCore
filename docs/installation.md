# Installation

Three ways to run OcheCore on a Linux board computer: `ochecore.sh` (a released
binary or a Docker container, installed and then managed by the same script), a
build from source, or Docker Compose by hand. All of them need no Internet after
installation, except to update.

## ochecore.sh: install and manage

```sh
curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/ochecore.sh | bash
```

This downloads the latest release for amd64 or arm64, checks its SHA-256, installs
`oche-core` and `ochecore.sh` to `~/.local/bin` (or `/usr/local/bin` as root),
offers to put your user in the `video` group (needed to open cameras), runs the
first-time setup questions, and can start OcheCore at boot as a systemd user
service.

To run it in Docker instead:

```sh
curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/ochecore.sh | bash -s -- install --docker
```

That installs Docker if it is missing, pulls the release image
(`ghcr.io/mondeggo/ochecore`), and starts it with Compose in `~/ochecore`, with the
cameras found at install time passed through. `~/ochecore/oche-core remote` reaches
the terminal UI inside the container.

Afterwards, manage it the same way in both modes:

| Command | Native | Docker |
| --- | --- | --- |
| `ochecore.sh start`, `stop`, `restart` | the systemd user service | the container |
| `ochecore.sh status` | service state and `oche-core status` | container state and `oche-core status` |
| `ochecore.sh logs` | `journalctl --user -u ochecore` | Compose logs |
| `ochecore.sh update [--version vX.Y.Z]` | new binary, service restarted | new image, container recreated |
| `ochecore.sh cameras` | not needed | pass the cameras plugged in now to the container |
| `ochecore.sh uninstall [--purge]` | removes the binary and service | removes the container and Compose file |

The script remembers how it installed OcheCore in `install.env` beside the
settings, so later commands need no options; `install` again with new options
(for example `--lan` or `--port`) changes them. `update` and `install` keep
settings and calibration; `uninstall` keeps them unless `--purge` (it asks
first). Without a systemd user session, a native install has no service: run
`oche-core run` yourself, and `start`, `stop` and `logs` explain this.

| Option | Meaning |
| --- | --- |
| `--docker` | Run the release image with Compose instead of installing a binary |
| `--version v0.1.3` | Install or update to a specific release (default: latest) |
| `--dir DIR` | Docker: install folder (default `~/ochecore`) |
| `--port N` | Docker: host port to publish (default 5180) |
| `--lan` | Docker: publish on all interfaces (plain HTTP, no login; see [networking](networking.md)) |
| `--no-system` | Do not change groups or services |
| `-y`, `--yes` | Accept the default answer to every question |
| `--purge` | `uninstall`: also delete settings, calibration and debug data |

`scripts/install.sh` still works and runs `ochecore.sh install` with the same
options. Installing needs a published
[release](https://github.com/mondeggo/ochecore/releases). Docker image downloads
without a login also need the GHCR package to be public (see [releases](releases.md)).

### Where the data is

| | Native | Docker |
| --- | --- | --- |
| Settings and board identity | `~/.config/ochecore/board.toml` | `~/ochecore/data/ochecore/board.toml` |
| Calibration (and the previous one) | `board.calibration.json`, `board.calibration.previous.json` beside it | same, beside it |
| Saved corrections | `debug/` beside it | same, beside it |
| Program | `~/.local/bin/oche-core` | the image |
| Manager | `~/.local/bin/ochecore.sh` | `~/.local/bin/ochecore.sh` |
| Boot service | `~/.config/systemd/user/ochecore.service` | Docker's `restart: unless-stopped` |

`$XDG_CONFIG_HOME/ochecore` replaces `~/.config/ochecore` when set; installs as
root use `/root/.config/ochecore` and `/usr/local/bin`.

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
to accept each default: `My dartboard`, `127.0.0.1`, `5180`, and two camera slots.
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
and healthcheck; `OCHECORE_PORT` changes only the published host port. The image
sets `OCHECORE_CONTAINER=1`, so the daemon refuses `server.bind` and
`server.port` changes from the browser UI, terminal UI and `config set`: change
`OCHECORE_BIND` or `OCHECORE_PORT` and recreate the container instead.
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
queried or captured; privileged mode is unnecessary. `ochecore.sh install --docker`
writes a Compose file that does this for the cameras it finds.
