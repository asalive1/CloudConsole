#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$tmp/deploy/local/stations"
cp "$root/deploy/local/station.sh" "$tmp/deploy/local/station.sh"
cp "$root/docker-compose.yml" "$root/docker-compose.prod.yml" \
    "$root/docker-compose.local.yml" "$root/docker-compose.host-audio.yml" "$tmp/"
cat > "$tmp/deploy/local/stations/test.env" <<'EOF'
STATION_SLOT=1
AUDIO_BIND=127.0.0.1
DATA_BIND=127.0.0.1
METADATA_BIND=127.0.0.1
JANUS_PUBLIC_ADDRESS=127.0.0.1
POSTGRES_PASSWORD=test
STATION_API_KEY=test
GUEST_JWT_SECRET=test
STATION_ID=00000000-0000-0000-0000-000000000001
JANUS_ROOM_PIN=test
ENGINE_UDP_PORTS=7110-7119
METADATA_TCP_PORTS=7110-7119
AUDIO_INPUT_PORTS="5004, 7020-7030, 10005"
EOF

bash "$tmp/deploy/local/station.sh" test config > "$tmp/config"
for port in 5004 7020 7030 10005; do
    grep -q "target: $port$" "$tmp/config" || { echo "missing port $port" >&2; exit 1; }
done
grep -q 'AUDIO_INPUT_PORTS: 5004, 7020-7030, 10005' "$tmp/config"

sed -i 's/^AUDIO_INPUT_PORTS=.*/AUDIO_INPUT_PORTS=5004,,10005/' "$tmp/deploy/local/stations/test.env"
if bash "$tmp/deploy/local/station.sh" test config > /dev/null 2>&1; then
    echo "invalid port list was accepted" >&2
    exit 1
fi

sed -i '/^AUDIO_INPUT_PORTS=/d' "$tmp/deploy/local/stations/test.env"
bash "$tmp/deploy/local/station.sh" test config > "$tmp/config"
grep -q 'target: 7110$' "$tmp/config"
grep -q 'AUDIO_INPUT_PORTS: 7110-7119' "$tmp/config"

echo "Station port configuration tests passed"
