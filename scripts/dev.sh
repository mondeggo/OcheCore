#!/usr/bin/env sh
set -eu
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT_DIR"

usage() {
    echo "Usage: scripts/dev.sh [COMPOSE_ARGS...]"
    echo "Builds and runs OcheCore in Docker, in demo mode, at http://localhost:5180."
    echo "Without arguments: up --build. OCHECORE_DEV_PORT changes the port."
}

case "${1:-}" in
    -h|--help) usage; exit 0 ;;
esac

if [ "$#" -eq 0 ]; then
    set -- up --build
fi
# compose.yaml publishes these; the development ones keep production's alone.
export OCHECORE_PORT="${OCHECORE_DEV_PORT:-5180}"
export OCHECORE_BIND="${OCHECORE_DEV_BIND:-127.0.0.1}"
exec docker compose --project-name ochecore-dev --project-directory "$ROOT_DIR" \
    -f "$ROOT_DIR/compose.yaml" -f "$ROOT_DIR/compose.dev.yaml" "$@"
