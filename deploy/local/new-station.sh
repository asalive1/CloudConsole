#!/usr/bin/env bash
# Creates the configuration for one station on the local server.
# Usage: new-station.sh                      (asks questions)
#        new-station.sh <name> <slot 0-3> [janus-address]
#   name           lowercase letters, digits and dashes; becomes the Compose project cc-<name>
#   slot           0-3; selects a non-overlapping port block
#   janus-address  address browsers use for WebRTC media (default: the data NIC address)
# Override the NIC addresses with AUDIO_IP and DATA_IP in the environment.
set -euo pipefail

audio_ip=${AUDIO_IP:-192.168.87.207}
data_ip=${DATA_IP:-192.168.50.207}
root="$(cd "$(dirname "$0")/../.." && pwd)"
dir="$root/deploy/local/stations"

if [ "$#" -gt 3 ]; then
    sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
if [ "$#" -eq 0 ]; then
    echo "Create a new station. Press Enter to accept the value in [brackets]."
    read -r -p "Station name (letters, numbers, dashes, e.g. andy): " name
    used=$(grep -h '^STATION_SLOT=' "$dir"/*.env 2>/dev/null | cut -d= -f2 | tr '\n' ' ' || true)
    next=0
    while [[ " $used " == *" $next "* ]]; do next=$((next + 1)); done
    if [ "$next" -gt 3 ]; then
        echo "all station slots 0-3 are already in use" >&2
        exit 1
    fi
    read -r -p "Slot number 0-3 (gives this station its own ports) [$next]: " slot
    slot=${slot:-$next}
    read -r -p "Address browsers use for microphone audio [$data_ip]: " janus_ip
    janus_ip=${janus_ip:-$data_ip}
else
    name=$1
    slot=${2:-}
    janus_ip=${3:-$data_ip}
fi
name=$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]')
env_file="$dir/$name.env"

[[ "$name" =~ ^[a-z0-9][a-z0-9-]{0,30}$ ]] || { echo "invalid station name: $name" >&2; exit 2; }
[[ "$slot" =~ ^[0-3]$ ]] || { echo "slot must be 0-3" >&2; exit 2; }
[ ! -e "$env_file" ] || { echo "$env_file already exists" >&2; exit 1; }
for other in "$dir"/*.env; do
    [ -e "$other" ] || continue
    if grep -q "^STATION_SLOT=$slot$" "$other"; then
        echo "slot $slot is already used by $(basename "$other" .env)" >&2
        exit 1
    fi
done

secret() { openssl rand -hex "$1"; }

mkdir -p "$dir"
umask 077
cat > "$env_file" <<EOF
STATION_NAME=$name
STATION_SLOT=$slot
HOST_AUDIO_NETWORK=true
STATION_ID=$(cat /proc/sys/kernel/random/uuid)
STATION_API_KEY=$(secret 24)
GUEST_JWT_SECRET=$(secret 32)
JANUS_ROOM_PIN=$(secret 16)
POSTGRES_PASSWORD=$(secret 24)
WHIP_WHEP_USERNAME=cloudconsole-stream
WHIP_WHEP_PASSWORD=$(secret 16)
AUDIO_BIND=$audio_ip
DATA_BIND=$data_ip
METADATA_BIND=$data_ip
JANUS_PUBLIC_ADDRESS=$janus_ip
STUN_SERVER=stun:stun.l.google.com:19302
HTTP_PORT=$((7002 + slot))
ENGINE_UDP_PORTS=$((7010 + slot * 100))-$((7019 + slot * 100))
AUDIO_INPUT_PORTS=$((7010 + slot * 100))-$((7019 + slot * 100))
METADATA_TCP_PORTS=$((7010 + slot * 100))-$((7019 + slot * 100))
JANUS_RTP_PORT_RANGE=$((7020 + slot * 100))-$((7099 + slot * 100))
MEDIAMTX_UDP_PORT=$((7100 + slot * 100))
EOF
umask 022

echo "Created $env_file (mode 600). Station API key:"
grep '^STATION_API_KEY=' "$env_file" | cut -d= -f2
cat <<EOF

Ports for station '$name' (slot $slot):
  TCP $((7002 + slot))   on $data_ip      web UI (point Nginx Proxy Manager here)
  UDP $((7010 + slot * 100))-$((7019 + slot * 100))   on $audio_ip   SRT/RTP audio inputs
  TCP $((7010 + slot * 100))-$((7019 + slot * 100))   on $data_ip      metadata
  UDP $((7020 + slot * 100))-$((7099 + slot * 100))   on $data_ip      browser microphone media (Janus)
  UDP $((7100 + slot * 100))   on $data_ip      WHIP/WHEP media

Next: deploy/local/station.sh $name up
EOF
