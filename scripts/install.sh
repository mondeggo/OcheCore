#!/usr/bin/env bash
# Kept so existing install commands keep working; the installer and manager is
# now ochecore.sh. These are the same:
#   curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/install.sh | bash -s -- --docker
#   curl -fsSL https://raw.githubusercontent.com/mondeggo/ochecore/main/scripts/ochecore.sh | bash -s -- install --docker
set -euo pipefail

here="$(dirname -- "${BASH_SOURCE[0]:-.}")"
if [[ -n "${BASH_SOURCE[0]:-}" && -f "$here/ochecore.sh" ]]; then
    exec bash "$here/ochecore.sh" install "$@"
fi
manager=$(curl -fsSL "https://raw.githubusercontent.com/${OCHECORE_REPO:-mondeggo/ochecore}/main/scripts/ochecore.sh") \
    || { echo "Error: could not download ochecore.sh" >&2; exit 1; }
exec bash -c "$manager" ochecore.sh install "$@"
