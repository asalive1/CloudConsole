#!/usr/bin/env bash
# Runs Docker Compose for one station with the production overlay.
# Usage: station.sh <name> <up|down|restart|logs|ps|build|config> [extra compose arguments]
set -euo pipefail

if [ "$#" -lt 2 ]; then
    sed -n '2,3p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
name=$1
action=$2
shift 2

root="$(cd "$(dirname "$0")/../.." && pwd)"
env_file="$root/stations/$name.env"
[ -f "$env_file" ] || { echo "no such station: $name (expected $env_file)" >&2; exit 1; }

compose=(docker compose -p "cc-$name" --env-file "$env_file"
    -f "$root/docker-compose.yml" -f "$root/docker-compose.prod.yml")

case "$action" in
    up) exec "${compose[@]}" up -d --build "$@" ;;
    down) exec "${compose[@]}" down "$@" ;;
    restart) exec "${compose[@]}" restart "$@" ;;
    logs) exec "${compose[@]}" logs --tail 200 "$@" ;;
    ps) exec "${compose[@]}" ps "$@" ;;
    build) exec "${compose[@]}" build "$@" ;;
    config) exec "${compose[@]}" config "$@" ;;
    *) echo "unknown action: $action" >&2; exit 2 ;;
esac
