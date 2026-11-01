<p align="center">
  <img src="docs/brand/ochecore-icon.svg" width="128" height="128" alt="OcheCore: a dartboard and dart framed by terminal brackets">
</p>

<h1 align="center">OcheCore</h1>

<p align="center">
  <b>Offline dartboard control, built in Rust.</b><br>
  One daemon. A command line and an interactive terminal UI.<br>
  Your board, managed locally or across your LAN.
</p>

<p align="center">
  <a href="https://github.com/mondeggo/ochecore/actions/workflows/ci.yml"><img alt="App build status" src="https://img.shields.io/github/actions/workflow/status/mondeggo/ochecore/ci.yml?branch=main&amp;event=push&amp;style=flat-square&amp;label=app%20build&amp;logo=rust"></a>
  <a href="https://github.com/mondeggo/ochecore/actions/workflows/docker.yml"><img alt="Docker build status" src="https://img.shields.io/github/actions/workflow/status/mondeggo/ochecore/docker.yml?branch=main&amp;event=push&amp;style=flat-square&amp;label=docker%20build&amp;logo=docker"></a>
  <a href="https://github.com/mondeggo/ochecore/releases/latest"><img alt="Latest stable release" src="https://img.shields.io/github/v/release/mondeggo/ochecore?display_name=tag&amp;sort=date&amp;style=flat-square&amp;label=release"></a>
</p>

<p align="center">
  <img alt="Built with Rust" src="https://img.shields.io/badge/built_with-Rust-b7410e?style=flat-square&amp;logo=rust">
  <img alt="Platform: Linux" src="https://img.shields.io/badge/platform-Linux-2f7bf5?style=flat-square&amp;logo=linux&amp;logoColor=white">
  <img alt="Interface: CLI and TUI" src="https://img.shields.io/badge/interface-CLI%20%2B%20TUI-0d9488?style=flat-square">
  <a href="LICENSE"><img alt="License: AGPL v3" src="https://img.shields.io/badge/license-AGPL%20v3-3a3a3a?style=flat-square"></a>
  <img alt="Status: early development" src="https://img.shields.io/badge/status-early%20development-d69e2e?style=flat-square">
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#first-run">First run</a> ·
  <a href="#documentation">Documentation</a> ·
  <a href="docs/native-api.md">Native API</a> ·
  <a href="oche-core-roadmap.md">Roadmap</a>
</p>

<br>

Offline Rust board service, command-line tools, and an interactive terminal UI for
a dartboard watched by one to four USB cameras. Game rules and player management
belong to the applications that use it.

**Implemented:** foreground daemon, local/LAN management, persistent board ID and
configuration, start/stop/reset/wake, logical standby, bounded native events,
camera discovery and V4L2 capture, board calibration from the wire grid,
single-dart orientation, dart detection from camera frame changes, multi-camera
fusion, a board-state indicator, diagnostics, and an explicit synthetic demo mode.

**Not verified:** scoring accuracy with real throws. The detection and
calibration methods are tested on synthetic images and a few recordings only;
there are no published accuracy numbers. The Autodarts compatibility adapter is
not implemented yet. See [the roadmap](oche-core-roadmap.md) and
[discovery notes](docs/discovery.md).

## Install

On the board computer (Linux, amd64 or arm64):

```sh
curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/install.sh | bash
```

To run it in Docker instead, add `--docker`:

```sh
curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/install.sh | bash -s -- --docker
```

Options, building from source, and Docker Compose are in
[Installation](docs/installation.md).

### First run

1. Start the service with `oche-core run` (the installer's boot service already
   does this), then open the terminal UI with `oche-core remote`, or the browser
   UI at <http://127.0.0.1:5180/>, which walks through the same steps
   ([Browser UI](docs/usage.md#browser-ui)).
2. Choose each camera (its name and USB port are shown) and press **Start**. The
   same can be done from the command line with `oche-core cameras available`,
   `oche-core config set cameras.1.device …` and `oche-core start`; see
   [Using OcheCore](docs/usage.md#commands).
3. Empty the board and press **Calibrate all** (`oche-core calibrate --all`). The
   first time, it asks you to throw **one dart at the middle of the 20**. It
   records where the 20 is in each camera and keeps it; later calibrations only
   realign the cameras. Run `oche-core orient` to record the 20 again, for example
   after moving a camera.
4. When the **State** line shows `READY`, you're set. Throw as usual: every dart
   is detected on this machine, with no account or Internet, and shows up live in
   the UI and in `oche-core events`. Pull the darts out when the visit is done.
   The states and what they mean are listed in
   [Using OcheCore](docs/usage.md#board-state).

## Documentation

| Topic | Page |
| --- | --- |
| Install, build from source, Docker | [Installation](docs/installation.md) |
| Commands, terminal and browser UI, demo mode | [Using OcheCore](docs/usage.md) |
| LAN access and security | [Networking and security](docs/networking.md) |
| Native HTTP API and events | [Native API](docs/native-api.md) |
| Design decisions and boundaries | [Architecture](docs/architecture.md) |
| Detection methods (proposed) | [Detection algorithms](docs/detection-algorithms.md) |
| Open questions and evidence | [Discovery](docs/discovery.md) |
| Build, test, release | [Development](docs/development.md), [Releases](docs/releases.md) |
| Plan | [Roadmap](oche-core-roadmap.md) |

## License

OcheCore is licensed under the [GNU Affero General Public License version 3](LICENSE)
(`AGPL-3.0-only`). Personal, educational, and commercial use are permitted under
its terms. See [NOTICE](NOTICE). Dependencies retain their own licenses.
