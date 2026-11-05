#!/usr/bin/env bash
# Backs up one station: a PostgreSQL dump plus its environment file (which holds the
# station secrets, so the archive is created mode 600).
# Usage: backup.sh <name> [backup-dir]
# Environment: KEEP_DAYS (default 14) prunes older local archives.
#              S3_BUCKET (optional) copies the archive with the AWS CLI, for example s3://my-bucket/cloud-console
set -euo pipefail

if [ "$#" -lt 1 ]; then
    sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
name=$1
root="$(cd "$(dirname "$0")/../.." && pwd)"
dest="${2:-/var/backups/cloud-console}/$name"
env_file="$root/stations/$name.env"
[ -f "$env_file" ] || { echo "no such station: $name" >&2; exit 1; }

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$dest"
umask 077

docker compose -p "cc-$name" --env-file "$env_file" \
    -f "$root/docker-compose.yml" -f "$root/docker-compose.prod.yml" \
    exec -T postgres pg_dump -U cloud_console --clean --if-exists cloud_console > "$work/database.sql"
[ -s "$work/database.sql" ] || { echo "database dump is empty" >&2; exit 1; }
cp "$env_file" "$work/station.env"

archive="$dest/$name-$stamp.tar.gz"
tar -C "$work" -czf "$archive" database.sql station.env
echo "wrote $archive"

if [ -n "${S3_BUCKET:-}" ]; then
    aws s3 cp "$archive" "${S3_BUCKET%/}/$name/" --only-show-errors
    echo "uploaded to ${S3_BUCKET%/}/$name/"
fi

find "$dest" -name "$name-*.tar.gz" -mtime "+${KEEP_DAYS:-14}" -delete
