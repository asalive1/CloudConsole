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
ports_overlay=
cleanup() {
    if [ -n "$ports_overlay" ]; then rm -f -- "$ports_overlay"; fi
}
trap cleanup EXIT
if grep -qx 'HOST_AUDIO_NETWORK=true' "$env_file"; then
    compose+=(-f "$root/docker-compose.host-audio.yml")
elif grep -q '^AUDIO_INPUT_PORTS=' "$env_file"; then
    ports=$(sed -n 's/^AUDIO_INPUT_PORTS=//p' "$env_file")
    [[ "$ports" != *$'\n'* ]] || { echo "AUDIO_INPUT_PORTS must occur only once" >&2; exit 1; }
    ports=${ports%$'\r'}
    ports=${ports%\"}
    ports=${ports#\"}
    ports=${ports%\'}
    ports=${ports#\'}
    [[ -n "$ports" && "$ports" != ,* && "$ports" != *, && "$ports" != *,,* ]] || {
        echo "AUDIO_INPUT_PORTS must contain comma-separated ports or ranges" >&2; exit 1;
    }
    ports_overlay=$(mktemp "${TMPDIR:-/tmp}/cloudconsole-ports.XXXXXXXX")
    printf 'services:\n  audio-engine:\n    ports: !override\n' > "$ports_overlay"
    IFS=',' read -r -a entries <<< "$ports"
    for entry in "${entries[@]}"; do
        entry=$(printf '%s' "$entry" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
        [[ "$entry" =~ ^([0-9]+)(-([0-9]+))?$ ]] || {
            echo "invalid AUDIO_INPUT_PORTS entry: $entry" >&2; exit 1;
        }
        if (( ${#BASH_REMATCH[1]} > 5 || ${#BASH_REMATCH[3]} > 5 )); then
            echo "invalid AUDIO_INPUT_PORTS entry: $entry" >&2; exit 1
        fi
        first=$((10#${BASH_REMATCH[1]}))
        last=$first
        if [ -n "${BASH_REMATCH[3]}" ]; then last=$((10#${BASH_REMATCH[3]})); fi
        if (( first < 1 || last > 65535 || first > last )); then
            echo "invalid AUDIO_INPUT_PORTS entry: $entry" >&2; exit 1
        fi
        if (( first == last )); then port_spec=$first; else port_spec="$first-$last"; fi
        printf '      - "${AUDIO_BIND:?set AUDIO_BIND}:%s:%s/udp"\n' "$port_spec" "$port_spec" >> "$ports_overlay"
    done
    printf '      - "${METADATA_BIND:?set METADATA_BIND}:${METADATA_TCP_PORTS:-7010-7019}:${METADATA_TCP_PORTS:-7010-7019}/tcp"\n' >> "$ports_overlay"
    compose+=(-f "$ports_overlay")
fi
alsa_overlay="$root/deploy/local/stations/$name.alsa.yml"
if [ -f "$alsa_overlay" ]; then
    compose+=(-f "$alsa_overlay")
fi

case "$action" in
    up) "${compose[@]}" up -d --build "$@" ;;
    down) "${compose[@]}" down "$@" ;;
    restart) "${compose[@]}" restart "$@" ;;
    logs) "${compose[@]}" logs --tail 200 "$@" ;;
    ps) "${compose[@]}" ps "$@" ;;
    build) "${compose[@]}" build "$@" ;;
    config) "${compose[@]}" config "$@" ;;
    update)
        git -C "$root" pull --ff-only
        "${compose[@]}" up -d --build "$@"
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
