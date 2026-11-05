#!/usr/bin/env bash
# Starts the shared TLS edge proxy if needed, obtains a Let's Encrypt certificate for the
# station's domain, and switches its edge configuration to HTTPS.
# Usage: CERTBOT_EMAIL=ops@example.com issue-cert.sh <name>
# The domain must already resolve to this instance and TCP 80 and 443 must be open.
set -euo pipefail

if [ "$#" -ne 1 ]; then
    sed -n '2,5p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
name=$1
: "${CERTBOT_EMAIL:?set CERTBOT_EMAIL}"

root="$(cd "$(dirname "$0")/../.." && pwd)"
edge="$root/deploy/aws/edge"
env_file="$root/stations/$name.env"
[ -f "$env_file" ] || { echo "no such station: $name" >&2; exit 1; }
domain="$(grep '^STATION_DOMAIN=' "$env_file" | cut -d= -f2)"
http_port="$(grep '^HTTP_PORT=' "$env_file" | cut -d= -f2)"

edge_compose=(docker compose -p cc-edge -f "$edge/docker-compose.yml")
mkdir -p "$edge/webroot" "$edge/conf.d"
"${edge_compose[@]}" up -d nginx
"${edge_compose[@]}" run --rm --entrypoint certbot certbot certonly --webroot -w /var/www/certbot \
    -d "$domain" --email "$CERTBOT_EMAIL" --agree-tos --no-eff-email --non-interactive

sed -e "s/@DOMAIN@/$domain/g" -e "s/@HTTP_PORT@/$http_port/g" \
    "$edge/station-tls.conf.template" > "$edge/conf.d/$name.conf"
"${edge_compose[@]}" up -d certbot
"${edge_compose[@]}" exec -T nginx nginx -t
"${edge_compose[@]}" exec -T nginx nginx -s reload
echo "https://$domain/ is now served for station $name"
