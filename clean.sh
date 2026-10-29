#!/usr/bin/env bash
# Remove only this checkout's generated build and packaging output.
set -euo pipefail

usage() {
  cat <<'HELP'
Usage: ./clean.sh [--dry-run]

Remove target/, dist/, and scripts/__pycache__/ beside this script.
This includes compiled binaries and release archives in dist/.
Source, Cargo.lock, configuration, data, recordings, and Docker resources
are preserved. Custom Cargo target directories outside this checkout are
not touched. Stop builds and packaging jobs before cleaning.

  --dry-run, -n  List existing paths without deleting anything
  --help, -h     Show this help
HELP
}

dry_run=false
for arg in "$@"; do
  case "$arg" in
    --dry-run|-n) dry_run=true ;;
    --help|-h) usage; exit 0 ;;
    *) printf 'Unknown argument: %s\n' "$arg" >&2; usage >&2; exit 2 ;;
  esac
done

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
if [[ ! -f "$root/Cargo.toml" || ! -d "$root/crates/oche-cli" ]]; then
  printf 'Run the clean.sh located at the OcheCore repository root.\n' >&2
  exit 1
fi

for relative in target dist scripts/__pycache__; do
  # Do not traverse a scripts/ symlink into another directory.
  if [[ "$relative" == scripts/* && -L "$root/scripts" ]]; then
    continue
  fi
  path="$root/$relative"
  if [[ -e "$path" || -L "$path" ]]; then
    if [[ "$dry_run" == true ]]; then
      printf 'Would remove: %s\n' "$path"
    else
      printf 'Removing: %s\n' "$path"
      rm -rf -- "$path"
    fi
  fi
done
