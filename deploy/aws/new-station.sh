#!/usr/bin/env bash
# Creates the configuration for one isolated station stack on this EC2 instance.
# Usage: new-station.sh <name> <slot 0-3> <domain> <public-ip>
#   name   lowercase letters, digits and dashes; becomes the Compose project cc-<name>
#   slot   0-3; selects a non-overlapping port block so four stations can share a host
#   domain public DNS name that points at this instance (for example console.wxyz.example.com)
#   public-ip  the instance's Elastic IP; Janus and MediaMTX advertise it in ICE candidates
set -euo pipefail

if [ "$#" -ne 4 ]; then
    sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
name=$1
slot=$2
domain=$3
public_ip=$4

root="$(cd "$(dirname "$0")/../.." && pwd)"
edge="$root/deploy/aws/edge"
env_file="$root/stations/$name.env"

[[ "$name" =~ ^[a-z0-9][a-z0-9-]{0,30}$ ]] || { echo "invalid station name: $name" >&2; exit 2; }
[[ "$slot" =~ ^[0-3]$ ]] || { echo "slot must be 0, 1, 2 or 3" >&2; exit 2; }
[[ "$domain" =~ ^[A-Za-z0-9.-]+$ ]] || { echo "invalid domain: $domain" >&2; exit 2; }
[[ "$public_ip" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]] || { echo "invalid public IP: $public_ip" >&2; exit 2; }
[ ! -e "$env_file" ] || { echo "$env_file already exists" >&2; exit 1; }
for other in "$root"/stations/*.env; do
    [ -e "$other" ] || continue
    if grep -q "^STATION_SLOT=$slot$" "$other"; then
        echo "slot $slot is already used by $(basename "$other" .env)" >&2
        exit 1
    fi
done

secret() { openssl rand -hex "$1"; }

mkdir -p "$root/stations" "$edge/conf.d" "$edge/webroot"
umask 077
cat > "$env_file" <<EOF
STATION_NAME=$name
STATION_SLOT=$slot
STATION_DOMAIN=$domain
STATION_ID=$(cat /proc/sys/kernel/random/uuid)
STATION_API_KEY=$(secret 24)
GUEST_JWT_SECRET=$(secret 32)
JANUS_ROOM_PIN=$(secret 16)
POSTGRES_PASSWORD=$(secret 24)
WHIP_WHEP_USERNAME=cloudconsole-stream
WHIP_WHEP_PASSWORD=$(secret 16)
JANUS_PUBLIC_ADDRESS=$public_ip
STUN_SERVER=stun:stun.l.google.com:19302
HTTP_BIND=127.0.0.1
HTTP_PORT=$((7002 + slot))
ENGINE_UDP_PORTS=$((7010 + slot * 100))-$((7019 + slot * 100))
METADATA_TCP_PORTS=$((7010 + slot * 100))-$((7019 + slot * 100))
JANUS_RTP_PORT_RANGE=$((7020 + slot * 100))-$((7099 + slot * 100))
MEDIAMTX_UDP_PORT=$((7100 + slot * 100))
EOF
umask 022

sed -e "s/@DOMAIN@/$domain/g" "$edge/station-http.conf.template" > "$edge/conf.d/$name.conf"

echo "Created $env_file (mode 600). Station API key:"
grep '^STATION_API_KEY=' "$env_file" | cut -d= -f2
cat <<EOF

Security group ports for this station (inbound):
  UDP $((7020 + slot * 100))-$((7099 + slot * 100))   browser microphone media (Janus)
  UDP $((7100 + slot * 100))                WHIP/WHEP media
  UDP $((7010 + slot * 100))-$((7019 + slot * 100))   RTP/SRT audio inputs from your automation system (restrict to its address)
  TCP $((7010 + slot * 100))-$((7019 + slot * 100))   metadata server ports (restrict to its address)

Next: deploy/aws/station.sh $name up, then deploy/aws/issue-cert.sh $name
EOF
