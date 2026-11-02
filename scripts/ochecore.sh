#!/usr/bin/env bash
# Install and manage OcheCore on a Linux board computer (amd64 or arm64).
#
#   curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/ochecore.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/ochecore.sh | bash -s -- install --docker
#
# Installing copies this script to the binary folder as ochecore.sh, so later:
#   ochecore.sh start | stop | restart | status | logs | update | cameras | uninstall
#
# Native (default): the released oche-core binary and a systemd user service.
# Docker (--docker): the released container image with Docker Compose.
set -euo pipefail

REPO="${OCHECORE_REPO:-mondeggo/ochecore}"
RAW="https://raw.githubusercontent.com/$REPO"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/ochecore"
# How OcheCore was installed, so later commands need no options. Beside the
# board's settings; the daemon ignores it.
state_file="$config_dir/install.env"

mode=native
version=""
dir=""
lan=0
port=5180
assume_yes=0
system_changes=1
purge=0

usage() {
    cat <<'EOF'
Usage: ochecore.sh [command] [options]

Commands:
  install     Install, or reinstall with new options (the default command)
  update      Install the latest release (or --version) and restart OcheCore
  start       Start OcheCore
  stop        Stop OcheCore; settings and calibration are kept
  restart     Restart OcheCore
  status      Show whether OcheCore is running and what it reports
  logs        Follow OcheCore's log (Ctrl+C to stop)
  cameras     Docker: pass the cameras plugged in now to the container
  uninstall   Remove OcheCore; --purge also deletes its settings and data
  help        Show this help

Options for install and update:
  --docker          Run OcheCore in Docker (installs Docker if missing)
  --version TAG     A specific release, for example v0.1.3 (default: latest)
  --dir DIR         Docker: install folder (default: ~/ochecore)
  --port N          Docker: host port to publish (default: 5180)
  --lan             Docker: publish on all network interfaces
                    (plain HTTP, no login: only on a trusted network)
  -y, --yes         Accept the default answer to every question
  --no-system       Do not change groups or services (no video group, no boot service)

Native installs put oche-core and ochecore.sh in ~/.local/bin (or /usr/local/bin
as root) and keep settings, calibration and debug data in ~/.config/ochecore.
Docker installs keep them in DIR/data/ochecore.
EOF
}

die() { printf 'Error: %s\n' "$*" >&2; exit 1; }
section() { printf '\n%s\n\n' "$1"; }

command_name=install
case "${1:-}" in
    install|update|start|stop|restart|status|logs|cameras|uninstall) command_name="$1"; shift ;;
    help|-h|--help) usage; exit 0 ;;
    ""|-*) ;;
    *) usage >&2; die "unknown command: $1" ;;
esac

# Saved choices first, so options given now override them.
if [[ -f "$state_file" ]]; then
    # shellcheck disable=SC1090
    source "$state_file"
    mode="${OCHECORE_MODE:-$mode}"
    dir="${OCHECORE_DIR:-$dir}"
    port="${OCHECORE_PORT:-$port}"
    lan="${OCHECORE_LAN:-$lan}"
fi
installed=0
[[ -f "$state_file" ]] && installed=1

while (( $# )); do
    case "$1" in
        --docker) mode=docker ;;
        --version) shift; version="${1:-}"; [[ -n "$version" ]] || die "--version needs a value" ;;
        --dir) shift; dir="${1:-}"; [[ -n "$dir" ]] || die "--dir needs a value" ;;
        --port) shift; port="${1:-}"; [[ "$port" =~ ^[0-9]+$ ]] || die "--port needs a number" ;;
        --lan) lan=1 ;;
        -y|--yes) assume_yes=1 ;;
        --no-system) system_changes=0 ;;
        --purge) purge=1 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
    shift
done

[[ "$(uname -s)" == Linux ]] || die "this script requires Linux"
dir="${dir:-$HOME/ochecore}"

# Questions are read from the terminal, because stdin carries the script when
# it is piped from curl. Without a terminal the default answer is used.
ask() {
    local reply=""
    if (( ! assume_yes )) && { : </dev/tty; } 2>/dev/null; then
        read -r -p "$1 " reply </dev/tty || reply=""
    fi
    reply="${reply:-$2}"
    [[ "$reply" == [yY]* ]]
}

sudo_cmd=()
if (( EUID != 0 )); then
    sudo_cmd=(sudo)
fi
as_root() {
    if (( EUID != 0 )); then
        command -v sudo >/dev/null || die "install sudo or run as root"
    fi
    "${sudo_cmd[@]}" "$@"
}

if (( EUID == 0 )); then
    bin_dir=/usr/local/bin
    doc_dir=/usr/local/share/doc/ochecore
else
    bin_dir="$HOME/.local/bin"
    doc_dir="$HOME/.local/share/doc/ochecore"
fi
unit_file="$HOME/.config/systemd/user/ochecore.service"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

latest_tag() {
    local url
    url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") \
        || die "could not reach GitHub"
    [[ "$url" == */releases/tag/* ]] \
        || die "no release has been published for $REPO yet"
    printf '%s\n' "${url##*/}"
}

resolve_version() {
    command -v curl >/dev/null || die "install curl first, then rerun this script"
    case "$(uname -m)" in
        x86_64|amd64) arch=amd64 ;;
        aarch64|arm64) arch=arm64 ;;
        *) die "unsupported CPU architecture: $(uname -m)" ;;
    esac
    [[ -n "$version" ]] || version=$(latest_tag)
    [[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]] \
        || die "version must look like v0.1.0, got: $version"
}

save_state() {
    mkdir -p "$config_dir"
    cat > "$state_file.new" <<EOF
# Written by ochecore.sh: how OcheCore is installed on this machine.
OCHECORE_MODE=$mode
OCHECORE_VERSION=$version
OCHECORE_DIR=$dir
OCHECORE_PORT=$port
OCHECORE_LAN=$lan
EOF
    mv -f "$state_file.new" "$state_file"
}

# Installs this script as $bin_dir/ochecore.sh: from the file being run, or
# from the release when it was piped from curl. A new file replaces the old
# one, so a running copy is never written over.
install_self() {
    local source="${BASH_SOURCE[0]:-}"
    mkdir -p "$bin_dir"
    if [[ -f "$source" && "$(readlink -f "$source")" != "$(readlink -f "$bin_dir/ochecore.sh" 2>/dev/null)" ]]; then
        cp "$source" "$bin_dir/ochecore.sh.new"
    elif [[ ! -f "$source" ]]; then
        curl -fsSL "$RAW/$version/scripts/ochecore.sh" -o "$bin_dir/ochecore.sh.new" 2>/dev/null \
            || curl -fsSL "$RAW/main/scripts/ochecore.sh" -o "$bin_dir/ochecore.sh.new" \
            || { echo "Could not save ochecore.sh; download it from $RAW/main/scripts/ochecore.sh"; return; }
    else
        return
    fi
    chmod 0755 "$bin_dir/ochecore.sh.new"
    mv -f "$bin_dir/ochecore.sh.new" "$bin_dir/ochecore.sh"
}

# After an update, the manager from the same release (when it has one).
refresh_self() {
    if curl -fsSL "$RAW/$version/scripts/ochecore.sh" -o "$work/ochecore.sh" 2>/dev/null \
        && bash -n "$work/ochecore.sh"; then
        install -m 0755 "$work/ochecore.sh" "$bin_dir/ochecore.sh.new"
        mv -f "$bin_dir/ochecore.sh.new" "$bin_dir/ochecore.sh"
    fi
}

path_hint() {
    case ":$PATH:" in
        *":$bin_dir:"*) ;;
        *) echo "Add $bin_dir to your PATH:  echo 'export PATH=\"$bin_dir:\$PATH\"' >> ~/.bashrc" ;;
    esac
}

video_gid() { getent group video | cut -d: -f3; }

have_user_systemd() {
    command -v systemctl >/dev/null && [[ -d /run/systemd/system ]] && (( EUID != 0 ))
}

# ---------------------------------------------------------------- native ----

install_binary() {
    local base="${OCHECORE_RELEASE_BASE:-https://github.com/$REPO/releases/download/$version}"
    local name="ochecore-$version-linux-$arch"
    echo "Downloading $name…"
    curl -fsSL "$base/$name.tar.gz" -o "$work/$name.tar.gz" || die "download failed: $base/$name.tar.gz"
    curl -fsSL "$base/$name.tar.gz.sha256" -o "$work/$name.tar.gz.sha256" \
        || die "download failed: $base/$name.tar.gz.sha256"
    (cd "$work" && sha256sum -c "$name.tar.gz.sha256") || die "checksum mismatch; nothing was installed"
    tar -xzf "$work/$name.tar.gz" -C "$work"
    local src="$work/$name"
    [[ -x "$src/oche-core" ]] || die "the archive does not contain oche-core"
    mkdir -p "$bin_dir" "$doc_dir"
    # Replace atomically so a running daemon is not written over.
    install -m 0755 "$src/oche-core" "$bin_dir/oche-core.new"
    mv -f "$bin_dir/oche-core.new" "$bin_dir/oche-core"
    install -m 0644 "$src/LICENSE" "$src/NOTICE" "$src/THIRD_PARTY_NOTICES.txt" "$doc_dir/"
    echo "Installed $("$bin_dir/oche-core" --version 2>/dev/null || echo oche-core) to $bin_dir/oche-core"
}

install_native() {
    section "OcheCore $version for linux-$arch (on this machine)"
    install_binary
    install_self
    save_state
    path_hint

    if (( system_changes )) && ! id -nG | tr ' ' '\n' | grep -qx video; then
        echo
        echo "Your user is not in the 'video' group, so it cannot open cameras."
        if ask "Add $USER to the video group now? [Y/n]" y; then
            as_root usermod -aG video "$USER"
            echo "Added. Log out and back in for it to take effect."
        fi
    fi

    if [[ ! -f "$config_dir/board.toml" ]] && { : </dev/tty; } 2>/dev/null && (( ! assume_yes )); then
        echo
        echo "Setting up this board (press Enter to accept each default)."
        "$bin_dir/oche-core" init </dev/tty || echo "Setup skipped; run 'oche-core init' later."
    fi

    if (( system_changes )) && have_user_systemd && [[ ! -f "$unit_file" ]]; then
        echo
        if ask "Start OcheCore automatically at boot? [Y/n]" y; then
            mkdir -p "$(dirname "$unit_file")"
            cat > "$unit_file" <<UNIT
[Unit]
Description=OcheCore board service

[Service]
ExecStart=$bin_dir/oche-core run
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
UNIT
            systemctl --user daemon-reload
            systemctl --user enable --now ochecore.service
            as_root loginctl enable-linger "$USER" || echo "Could not enable lingering; the service starts at login."
        fi
    elif [[ -f "$unit_file" ]] && have_user_systemd; then
        systemctl --user restart ochecore.service
        echo "Restarted the OcheCore service."
    fi

    section "Next steps"
    cat <<EOF
  ochecore.sh status        # is it running?
  oche-core remote          # terminal UI: assign cameras, press Start
  http://127.0.0.1:5180/    # browser UI on this machine

Empty the board, then calibrate (Setup in the browser, or "Calibrate all" in the
terminal UI). Settings and calibration are in $config_dir.
Manage it with: ochecore.sh start | stop | restart | logs | update
EOF
}

update_native() {
    section "Updating OcheCore to $version (on this machine)"
    install_binary
    refresh_self
    save_state
    if [[ -f "$unit_file" ]] && have_user_systemd; then
        systemctl --user restart ochecore.service
        echo "Restarted the OcheCore service."
    else
        echo "Restart oche-core run to use the new version."
    fi
}

# Runs systemctl for the service, or explains how to run OcheCore without one.
native_service() {
    if [[ -f "$unit_file" ]] && have_user_systemd; then
        systemctl --user "$@" ochecore.service
    else
        die "OcheCore has no boot service here. Run 'oche-core run' yourself, or run 'ochecore.sh install' again and accept the service"
    fi
}

# ---------------------------------------------------------------- docker ----

ensure_docker() {
    if command -v docker >/dev/null && docker compose version >/dev/null 2>&1; then
        return
    fi
    section "Docker is missing"
    ask "Install Docker with the official get.docker.com script? [Y/n]" y || die "Docker is required for --docker"
    command -v systemctl >/dev/null || die "installing Docker requires systemd"
    curl -fsSL https://get.docker.com -o "$work/get-docker.sh"
    as_root sh "$work/get-docker.sh"
    as_root systemctl enable --now docker
    as_root docker compose version >/dev/null || die "Docker Compose is not available after installing Docker"
}

docker_cmd() {
    if docker info >/dev/null 2>&1; then
        docker "$@"
    else
        "${sudo_cmd[@]}" docker "$@"
    fi
}

compose() {
    [[ -f "$dir/compose.yaml" ]] || die "no OcheCore Compose file in $dir; run 'ochecore.sh install --docker'"
    docker_cmd compose -f "$dir/compose.yaml" "$@"
}

# The Compose file for this release, with the cameras plugged in now.
write_compose() {
    local image="${OCHECORE_IMAGE:-ghcr.io/${REPO,,}:$version}"
    local bind=127.0.0.1
    (( lan )) && bind=0.0.0.0
    mkdir -p "$dir/data"
    local devices=() node
    for node in /dev/video[0-9]*; do
        [[ -c "$node" ]] && devices+=("$node")
    done
    if (( ${#devices[@]} == 0 )); then
        echo "No camera devices found now. Plug the cameras in and run 'ochecore.sh cameras'."
    fi
    {
        echo "name: ochecore"
        echo "services:"
        echo "  ochecore:"
        echo "    image: $image"
        echo "    restart: unless-stopped"
        echo "    user: \"$(id -u):$(id -g)\""
        if [[ -n "$(video_gid)" ]]; then
            echo "    group_add:"
            echo "      - \"$(video_gid)\""
        fi
        echo "    ports:"
        echo "      - \"$bind:$port:5180\""
        echo "    volumes:"
        echo "      - ./data:/data"
        [[ -d /dev/v4l ]] && echo "      - /dev/v4l:/dev/v4l:ro"
        if (( ${#devices[@]} )); then
            echo "    devices:"
            for node in "${devices[@]}"; do echo "      - \"$node:$node\""; done
        fi
        cat <<'YAML'
    read_only: true
    cap_drop:
      - ALL
    security_opt:
      - no-new-privileges:true
    healthcheck:
      test: ["CMD", "/usr/local/bin/oche-core", "status", "--json"]
      interval: 15s
      timeout: 5s
      start_period: 5s
      retries: 3
    stop_grace_period: 10s
YAML
    } > "$dir/compose.yaml.new"
    mv -f "$dir/compose.yaml.new" "$dir/compose.yaml"
    COMPOSE_IMAGE="$image"
}

pull_image() {
    echo "Pulling $COMPOSE_IMAGE…"
    if ! compose pull; then
        docker_cmd image inspect "$COMPOSE_IMAGE" >/dev/null 2>&1 || die "could not pull $COMPOSE_IMAGE"
        echo "Using the image already on this machine."
    fi
}

install_docker_mode() {
    section "OcheCore $version in Docker"
    ensure_docker
    (( lan )) && echo "Publishing on all interfaces: the API has no login. Use a trusted network."
    write_compose
    cat > "$dir/oche-core" <<'WRAPPER'
#!/usr/bin/env bash
# Runs oche-core inside the container: ./oche-core remote, ./oche-core status…
cd "$(dirname "$0")"
if docker info >/dev/null 2>&1; then
    exec docker compose exec ochecore oche-core "$@"
fi
exec sudo docker compose exec ochecore oche-core "$@"
WRAPPER
    chmod +x "$dir/oche-core"
    pull_image
    compose up -d
    install_self
    save_state
    path_hint

    section "Next steps"
    cat <<EOF
  ochecore.sh status        # is it running?
  $dir/oche-core remote     # terminal UI: assign cameras, press Start
  http://127.0.0.1:$port/   # browser UI

Empty the board, then calibrate (Setup in the browser, or "Calibrate all" in the
terminal UI). The container restarts after a reboot. Settings and calibration
are in $dir/data/ochecore.
Manage it with: ochecore.sh start | stop | restart | logs | update | cameras
EOF
}

update_docker() {
    section "Updating OcheCore to $version in Docker"
    write_compose
    pull_image
    compose up -d --force-recreate
    refresh_self
    save_state
}

# ------------------------------------------------------------- commands ----

require_installed() {
    (( installed )) || die "OcheCore was not installed with this script here ($state_file is missing). Run: ochecore.sh install"
}

show_status() {
    if [[ "$mode" == docker ]]; then
        compose ps
        [[ -x "$dir/oche-core" ]] && "$dir/oche-core" status 2>/dev/null || true
    else
        if [[ -f "$unit_file" ]] && have_user_systemd; then
            systemctl --user --no-pager status ochecore.service || true
        fi
        "$bin_dir/oche-core" status || true
    fi
    echo "Installed: ${OCHECORE_VERSION:-unknown} ($mode)"
}

uninstall() {
    section "Removing OcheCore ($mode)"
    if [[ "$mode" == docker ]]; then
        [[ -f "$dir/compose.yaml" ]] && compose down || true
        rm -f "$dir/compose.yaml" "$dir/oche-core"
    else
        if [[ -f "$unit_file" ]] && have_user_systemd; then
            systemctl --user disable --now ochecore.service || true
            rm -f "$unit_file"
            systemctl --user daemon-reload
        fi
        rm -f "$bin_dir/oche-core"
        rm -rf "$doc_dir"
    fi
    local data="$config_dir"
    [[ "$mode" == docker ]] && data="$dir/data"
    if (( purge )) && { (( assume_yes )) || ask "Delete all settings, calibration and debug data in $data? [y/N]" n; }; then
        rm -rf "$data"
        [[ "$mode" == docker ]] && rmdir "$dir" 2>/dev/null || true
        echo "Deleted $data."
    else
        echo "Kept the settings and calibration in $data."
    fi
    rm -f "$state_file" "$bin_dir/ochecore.sh"
    echo "OcheCore is removed."
}

case "$command_name" in
    install)
        resolve_version
        case "$mode" in
            native) install_native ;;
            docker) install_docker_mode ;;
        esac
        ;;
    update)
        require_installed
        resolve_version
        case "$mode" in
            native) update_native ;;
            docker) update_docker ;;
        esac
        ;;
    start|stop|restart)
        require_installed
        if [[ "$mode" == docker ]]; then
            case "$command_name" in
                start) compose up -d ;;
                stop) compose stop ;;
                restart) compose restart ;;
            esac
        else
            native_service "$command_name"
        fi
        ;;
    status) require_installed; show_status ;;
    logs)
        require_installed
        if [[ "$mode" == docker ]]; then
            compose logs -f --tail 200
        elif [[ -f "$unit_file" ]] && have_user_systemd; then
            journalctl --user -u ochecore.service -f -n 200
        else
            die "OcheCore has no boot service here, so its log is the terminal that runs 'oche-core run'"
        fi
        ;;
    cameras)
        require_installed
        [[ "$mode" == docker ]] || die "not needed natively: oche-core opens the cameras directly"
        version="${OCHECORE_VERSION:-}"
        [[ -n "$version" ]] || resolve_version
        write_compose
        compose up -d --force-recreate
        echo "The container now sees the cameras plugged in."
        ;;
    uninstall) require_installed; uninstall ;;
esac
