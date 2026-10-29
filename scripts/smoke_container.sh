#!/usr/bin/env bash
set -euo pipefail
image=${1:?usage: smoke_container.sh IMAGE}
container="ochecore-smoke-$$-$RANDOM"
volume="$container-data"
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  docker volume rm "$volume" >/dev/null 2>&1 || true
}
trap cleanup EXIT
docker volume create "$volume" >/dev/null
docker run --detach --name "$container" --network none --read-only \
  --cap-drop ALL --security-opt no-new-privileges:true \
  --mount "type=volume,source=$volume,target=/data" \
  "$image" --bind 0.0.0.0 run --demo >/dev/null
ready=false
for _ in {1..40}; do
  if docker exec "$container" /usr/local/bin/oche-core status --json >/dev/null 2>&1; then ready=true; break; fi
  sleep 0.25
done
if [[ "$ready" != true ]]; then docker logs "$container"; exit 1; fi
docker exec "$container" /usr/local/bin/oche-core start --json >/dev/null
docker exec "$container" /usr/local/bin/oche-core demo throw T20 --json > /tmp/"$container".json
python3 - /tmp/"$container".json <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))
assert s['throws'][0]['score'] == 60 and s['throws'][0]['synthetic']
assert not s['capabilities']['vision']
PY
rm /tmp/"$container".json
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
echo 'PASS: non-root/read-only container, offline controls, synthetic visit, persistent identity and configuration'
