#!/usr/bin/env bash
set -euo pipefail
image=${1:?usage: smoke_container.sh IMAGE}
container="ochecore-smoke-$$-$RANDOM"
data_dir=$(mktemp -d)
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -rf "$data_dir"
}
trap cleanup EXIT
# Match the host folder owner, as in the documented Compose setup.
test_uid=$(id -u)
test_gid=$(id -g)
if [[ "$test_uid" == 0 ]]; then
  test_uid=65532
  test_gid=65532
  chown "$test_uid:$test_gid" "$data_dir"
fi
docker run --detach --name "$container" --network none --read-only \
  --user "$test_uid:$test_gid" \
  --cap-drop ALL --security-opt no-new-privileges:true \
  --mount "type=bind,source=$data_dir,target=/data" \
  "$image" >/dev/null
ready=false
for _ in {1..40}; do
  if docker exec "$container" /usr/local/bin/oche-core status --json >/dev/null 2>&1; then ready=true; break; fi
  sleep 0.25
done
if [[ "$ready" != true ]]; then docker logs "$container"; exit 1; fi
docker exec "$container" /usr/local/bin/oche-core start --json >/dev/null
docker logs "$container" > "$data_dir/startup.log" 2>&1
python3 - "$data_dir/startup.log" <<'CHECK'
from pathlib import Path
import sys
text = Path(sys.argv[1]).read_text()
assert 'No configuration found' in text
assert 'Starting the daemon now' in text
CHECK
docker exec "$container" /usr/local/bin/oche-core config set board.name 'Container board' --json >/dev/null
first=$(docker exec "$container" /usr/local/bin/oche-core status --json)
docker restart --time 5 "$container" >/dev/null
for _ in {1..40}; do
  if docker exec "$container" /usr/local/bin/oche-core status --json >/dev/null 2>&1; then break; fi
  sleep 0.25
done
second=$(docker exec "$container" /usr/local/bin/oche-core status --json)
python3 - "$first" "$second" <<'PY'
import json, sys
a, b = map(json.loads, sys.argv[1:])
assert a['config']['board']['id'] == b['config']['board']['id']
assert b['config']['board']['name'] == 'Container board'
assert a['session_id'] != b['session_id']
PY
echo 'PASS: first-run setup and automatic startup, non-root/read-only container, offline controls, persistent identity and configuration'
