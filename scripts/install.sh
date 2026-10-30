#!/usr/bin/env bash
# Install OcheCore on a Linux board computer (amd64 or arm64).
#
#   curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/install.sh | bash -s -- --docker
#
# Default: install the released oche-core binary on this machine.
# --docker: run the released container image with Docker Compose instead.
# Local:  bash scripts/install.sh [options]
set -euo pipefail

REPO="${OCHECORE_REPO:-mondeggo/ochecore}"
mode=native
version=""
dir=""
lan=0
port=5180
assume_yes=0
system_changes=1

usage() {
    cat <<'EOF'
Usage: install.sh [options]

  --docker          Run OcheCore in Docker (installs Docker if missing)
  --version TAG     Install a specific release, for example v0.1.0 (default: latest)
  --dir DIR         Docker mode: install folder (default: ~/ochecore)
  --port N          Docker mode: host port to publish (default: 5180)
  --lan             Docker mode: publish the API on all network interfaces
                    (plain HTTP, no login: only on a trusted network)
  -y, --yes         Accept the default answer to every question
  --no-system       Do not change groups or services (no video group, no boot service)
  -h, --help        Show this help

Without --docker the binary goes to ~/.local/bin (or /usr/local/bin as root).
Run the script again to update.
EOF
}

die() { printf 'Error: %s\n' "$*" >&2; exit 1; }
section() { printf '\n%s\n\n' "$1"; }

while (( $# )); do
    case "$1" in
        --docker) mode=docker ;;
        --version) shift; version="${1:-}"; [[ -n "$version" ]] || die "--version needs a value" ;;
        --dir) shift; dir="${1:-}"; [[ -n "$dir" ]] || die "--dir needs a value" ;;
        --port) shift; port="${1:-}"; [[ "$port" =~ ^[0-9]+$ ]] || die "--port needs a number" ;;
        --lan) lan=1 ;;
        -y|--yes) assume_yes=1 ;;
        --no-system) system_changes=0 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
    shift
done

[[ "$(uname -s)" == Linux ]] || die "this script requires Linux"
command -v curl >/dev/null || die "install curl first, then rerun this script"

case "$(uname -m)" in
    x86_64|amd64) arch=amd64 ;;
    aarch64|arm64) arch=arm64 ;;
    *) die "unsupported CPU architecture: $(uname -m)" ;;
esac

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

latest_tag() {
    local url
    url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") \
        || die "could not reach GitHub"
    [[ "$url" == */releases/tag/* ]] \
        || die "no release has been published for $REPO yet"
    printf '%s\n' "${url##*/}"
}

[[ -n "$version" ]] || version=$(latest_tag)
[[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]] \
    || die "version must look like v0.1.0, got: $version"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

video_gid() { getent group video | cut -d: -f3; }

# ---------------------------------------------------------------- native ----

install_native() {
    section "OcheCore $version for linux-$arch (on this machine)"
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

    local bin_dir doc_dir
    if (( EUID == 0 )); then
        bin_dir=/usr/local/bin
        doc_dir=/usr/local/share/doc/ochecore
    else
        bin_dir="$HOME/.local/bin"
        doc_dir="$HOME/.local/share/doc/ochecore"
    fi
    mkdir -p "$bin_dir" "$doc_dir"
    # Replace atomically so a running daemon is not written over.
    install -m 0755 "$src/oche-core" "$bin_dir/oche-core.new"
    mv -f "$bin_dir/oche-core.new" "$bin_dir/oche-core"
    install -m 0644 "$src/LICENSE" "$src/NOTICE" "$src/THIRD_PARTY_NOTICES.txt" "$doc_dir/"
    echo "Installed $("$bin_dir/oche-core" --version 2>/dev/null || echo oche-core) to $bin_dir/oche-core"
    case ":$PATH:" in
        *":$bin_dir:"*) ;;
        *) echo "Add it to your PATH:  echo 'export PATH=\"$bin_dir:\$PATH\"' >> ~/.bashrc" ;;
    esac

    if (( system_changes )) && ! id -nG | tr ' ' '\n' | grep -qx video; then
        echo
        echo "Your user is not in the 'video' group, so it cannot open cameras."
        if ask "Add $USER to the video group now? [Y/n]" y; then
            as_root usermod -aG video "$USER"
            echo "Added. Log out and back in for it to take effect."
        fi
    fi

    local config="${XDG_CONFIG_HOME:-$HOME/.config}/ochecore/board.toml"
    if [[ ! -f "$config" ]] && { : </dev/tty; } 2>/dev/null && (( ! assume_yes )); then
        echo
        echo "Setting up this board (press Enter to accept each default)."
        "$bin_dir/oche-core" init </dev/tty || echo "Setup skipped; run 'oche-core init' later."
    fi

    if (( system_changes )) && command -v systemctl >/dev/null && [[ -d /run/systemd/system ]] && (( EUID != 0 )); then
        echo
        if ask "Start OcheCore automatically at boot? [Y/n]" y; then
            mkdir -p "$HOME/.config/systemd/user"
            cat > "$HOME/.config/systemd/user/ochecore.service" <<UNIT
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
            echo "Service enabled: systemctl --user status ochecore"
        fi
    fi

    section "Next steps"
    cat <<EOF
  1. oche-core run              # skip if the service is running
  2. oche-core remote           # terminal UI: assign cameras, press Start
  3. Empty the board, then press "Calibrate all". The first time it asks you to
     throw one dart at the 20; later calibrations only realign the cameras.
EOF
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

install_docker_mode() {
    section "OcheCore $version in Docker"
    ensure_docker
    local docker=(docker)
    docker info >/dev/null 2>&1 || docker=("${sudo_cmd[@]}" docker)

    dir="${dir:-$HOME/ochecore}"
    local image="${OCHECORE_IMAGE:-ghcr.io/${REPO,,}:$version}"
    local bind=127.0.0.1
    if (( lan )); then
        bind=0.0.0.0
        echo "Publishing on all interfaces: the API has no login. Use a trusted network."
    fi
    mkdir -p "$dir/data"

    local devices=() node
    for node in /dev/video[0-9]*; do
        [[ -c "$node" ]] && devices+=("$node")
    done
    if (( ${#devices[@]} == 0 )); then
        echo "No camera devices found now. Plug the cameras in and run this script again."
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
    } > "$dir/compose.yaml"

    cat > "$dir/oche-core" <<WRAPPER
#!/usr/bin/env bash
# Runs oche-core inside the container: ./oche-core remote, ./oche-core status…
cd "\$(dirname "\$0")"
exec ${docker[*]} compose exec ochecore oche-core "\$@"
WRAPPER
    chmod +x "$dir/oche-core"

    echo "Pulling $image…"
    if ! "${docker[@]}" compose -f "$dir/compose.yaml" pull; then
        "${docker[@]}" image inspect "$image" >/dev/null 2>&1 || die "could not pull $image"
        echo "Using the image already on this machine."
    fi
    "${docker[@]}" compose -f "$dir/compose.yaml" up -d

    section "Next steps"
    cat <<EOF
  cd $dir
  ./oche-core remote       # terminal UI: assign cameras, press Start
  ./oche-core status
  ${docker[*]} compose logs -f

Empty the board, then press "Calibrate all" in the UI. The first time it asks
you to throw one dart at the 20; later calibrations only realign the cameras.
The container restarts automatically after a reboot. Update by running this
script again; your settings stay in $dir/data.
EOF
}

case "$mode" in
    native) install_native ;;
    docker) install_docker_mode ;;
esac
