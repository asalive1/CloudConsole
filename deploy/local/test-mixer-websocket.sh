#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
suffix="$$-$RANDOM"
network="cc-ws-test-$suffix"
database="cc-ws-db-$suffix"
backend="cc-ws-api-$suffix"
image="cc-ws-test:$suffix"
cleanup() {
    docker rm -f "$backend" "$database" >/dev/null 2>&1 || true
    docker network rm "$network" >/dev/null 2>&1 || true
    docker image rm "$image" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker build -f "$root/backend/Dockerfile" -t "$image" "$root"
docker network create "$network" >/dev/null
docker run -d --name "$database" --network "$network" --network-alias cloudconsole-mixer-db \
    -e POSTGRES_USER=cloud_console -e POSTGRES_PASSWORD=synthetic-password \
    -e POSTGRES_DB=cloud_console postgres:17-alpine >/dev/null
ready=false
for _ in $(seq 1 60); do
    if docker exec "$database" pg_isready -U cloud_console -d cloud_console >/dev/null 2>&1; then
        ready=true
        break
    fi
    sleep 1
done
if [ "$ready" != true ]; then echo "Test database did not start" >&2; exit 1; fi

docker run -d --name "$backend" --network "$network" --network-alias cloudconsole-mixer-api \
    -e DATABASE_URL=postgres://cloud_console:synthetic-password@cloudconsole-mixer-db:5432/cloud_console \
    -e STATION_API_KEY=synthetic-test-api-key \
    -e GUEST_JWT_SECRET=synthetic-secret-at-least-thirty-two-characters \
    -e STATION_ID=00000000-0000-0000-0000-000000000001 \
    -e JANUS_ROOM_PIN=synthetic-test-pin \
    -e AUDIO_ENGINE_URL=http://127.0.0.1:1 -e JANUS_HTTP_URL=http://127.0.0.1:1 \
    "$image" >/dev/null
ready=false
for _ in $(seq 1 60); do
    if docker exec "$database" psql -U cloud_console -d cloud_console -tAc \
        "SELECT count(*) FROM stations WHERE id='00000000-0000-0000-0000-000000000001'" 2>/dev/null | grep -q '^1$'; then
        ready=true
        break
    fi
    sleep 1
done
if [ "$ready" != true ]; then
    docker logs "$backend" >&2
    echo "Test backend did not initialize" >&2
    exit 1
fi
docker exec "$database" psql -U cloud_console -d cloud_console -c \
    "INSERT INTO inputs (id, station_id, name, source_type, enabled, config) VALUES ('00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000001', 'Test input', 'aux', true, '{\"type\":\"test\"}');" >/dev/null

baseline=$(docker exec "$backend" sh -c 'ls /proc/1/fd | wc -l')
docker run --rm -i --network "$network" node:24-alpine node - < "$root/backend/tests/mixer-websocket.mjs"
sleep 2
after=$(docker exec "$backend" sh -c 'ls /proc/1/fd | wc -l')
echo "Backend descriptors: baseline=$baseline after=$after"
if (( after > baseline + 3 )); then
    echo "Backend retained WebSocket descriptors after disconnects" >&2
    exit 1
fi
