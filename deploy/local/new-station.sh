#!/usr/bin/env bash
# Creates the configuration for one station on the local server.
# Usage: new-station.sh <name> <slot 0-9> [janus-address]
#   name           lowercase letters, digits and dashes; becomes the Compose project cc-<name>
#   slot           0-9; selects a non-overlapping port block
#   janus-address  address browsers use for WebRTC media (default: the data NIC address)
# Override the NIC addresses with AUDIO_IP and DATA_IP in the environment.
set -euo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
    sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
name=$1
slot=$2
audio_ip=${AUDIO_IP:-192.168.87.207}
data_ip=${DATA_IP:-192.168.50.207}
janus_ip=${3:-$data_ip}

root="$(cd "$(dirname "$0")/../.." && pwd)"
dir="$root/deploy/local/stations"
env_file="$dir/$name.env"

[[ "$name" =~ ^[a-z0-9][a-z0-9-]{0,30}$ ]] || { echo "invalid station name: $name" >&2; exit 2; }
[[ "$slot" =~ ^[0-9]$ ]] || { echo "slot must be 0-9" >&2; exit 2; }
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
HTTP_PORT=$((8080 + slot))
ENGINE_UDP_PORTS=$((5000 + slot * 50))-$((5049 + slot * 50))
METADATA_TCP_PORTS=$((6000 + slot * 20))-$((6019 + slot * 20))
JANUS_RTP_PORT_RANGE=$((11000 + slot * 100))-$((11099 + slot * 100))
MEDIAMTX_UDP_PORT=$((12000 + slot))
EOF
umask 022

echo "Created $env_file (mode 600). Station API key:"
grep '^STATION_API_KEY=' "$env_file" | cut -d= -f2
cat <<EOF

Ports for station '$name' (slot $slot):
  TCP $((8080 + slot))   on $data_ip      web UI (point Nginx Proxy Manager here)
  UDP $((5000 + slot * 50))-$((5049 + slot * 50))   on $audio_ip   SRT/RTP audio inputs
  TCP $((6000 + slot * 20))-$((6019 + slot * 20))   on $data_ip      metadata
  UDP $((11000 + slot * 100))-$((11099 + slot * 100))  on $data_ip      browser microphone media (Janus)
  UDP $((12000 + slot))   on $data_ip      WHIP/WHEP media

Next: deploy/local/station.sh $name up
EOF
