#!/usr/bin/env bash
# Runs Docker Compose for one local station (base + production + local overlays).
# Usage: station.sh <name> <up|down|restart|logs|ps|build|config|backup|update> [extra compose arguments]
set -euo pipefail

if [ "$#" -lt 2 ]; then
    sed -n '2,3p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
name=$1
action=$2
shift 2

root="$(cd "$(dirname "$0")/../.." && pwd)"
env_file="$root/deploy/local/stations/$name.env"
[ -f "$env_file" ] || { echo "no such station: $name (expected $env_file)" >&2; exit 1; }

compose=(docker compose -p "cc-$name" --env-file "$env_file"
    -f "$root/docker-compose.yml" -f "$root/docker-compose.prod.yml" -f "$root/docker-compose.local.yml")

case "$action" in
    up) exec "${compose[@]}" up -d --build "$@" ;;
    down) exec "${compose[@]}" down "$@" ;;
    restart) exec "${compose[@]}" restart "$@" ;;
    logs) exec "${compose[@]}" logs --tail 200 "$@" ;;
    ps) exec "${compose[@]}" ps "$@" ;;
    build) exec "${compose[@]}" build "$@" ;;
    config) exec "${compose[@]}" config "$@" ;;
    update)
        git -C "$root" pull --ff-only
        exec "${compose[@]}" up -d --build "$@"
        ;;
    backup)
        out="${1:-$root/deploy/local/backups}"
        mkdir -p "$out"
        file="$out/$name-$(date +%Y%m%d-%H%M%S).sql"
        umask 077
        "${compose[@]}" exec -T postgres pg_dump -U cloud_console cloud_console > "$file"
        echo "wrote $file"
        ;;
    *) echo "unknown action: $action" >&2; exit 2 ;;
esac
