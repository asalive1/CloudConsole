#!/usr/bin/env bash
# Restores one station from a backup archive made by backup.sh. This replaces the
# station's database contents. On a new instance the environment file is restored
# first, so the station keeps its API key and secrets.
# Usage: restore.sh <name> <archive.tar.gz>
set -euo pipefail

if [ "$#" -ne 2 ]; then
    sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
name=$1
archive=$2
root="$(cd "$(dirname "$0")/../.." && pwd)"
env_file="$root/stations/$name.env"
[ -f "$archive" ] || { echo "no such archive: $archive" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tar -C "$work" -xzf "$archive" database.sql station.env

if [ ! -f "$env_file" ]; then
    mkdir -p "$root/stations"
    install -m 600 "$work/station.env" "$env_file"
    echo "restored $env_file"
fi

compose=(docker compose -p "cc-$name" --env-file "$env_file"
    -f "$root/docker-compose.yml" -f "$root/docker-compose.prod.yml")

"${compose[@]}" up -d postgres
for _ in $(seq 1 30); do
    "${compose[@]}" exec -T postgres pg_isready -U cloud_console -d cloud_console > /dev/null 2>&1 && break
    sleep 2
done
"${compose[@]}" stop backend > /dev/null 2>&1 || true
"${compose[@]}" exec -T postgres psql -U cloud_console -d cloud_console -v ON_ERROR_STOP=1 < "$work/database.sql" > /dev/null
"${compose[@]}" up -d --build
echo "restored $name from $archive"
