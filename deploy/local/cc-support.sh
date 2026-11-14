#!/usr/bin/env bash
# Collect diagnostic logs and a database backup for one local station.
# Usage: bash deploy/local/cc-support.sh <station-name> [destination-directory]
set -euo pipefail

# Override with CC_SUPPORT_DIR or the optional destination argument.
support_dir=${CC_SUPPORT_DIR:-/srv/wo/cs/support}
log_dir=${CC_LOG_DIR:-/var/log/CloudConsole}

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "Usage: bash deploy/local/cc-support.sh <station-name> [destination-directory]" >&2
    exit 2
fi
station=$1
[[ "$station" =~ ^[a-z0-9][a-z0-9-]{0,30}$ ]] || {
    echo "Invalid station name: $station" >&2
    exit 2
}
destination=${2:-$support_dir}
[[ "$destination" = /* ]] || { echo "Support directory must be an absolute path" >&2; exit 2; }

for tool in docker tar timeout mktemp; do
    command -v "$tool" >/dev/null || { echo "Required command not found: $tool" >&2; exit 1; }
done
umask 077
mkdir -p -- "$destination"
work=$(mktemp -d "${TMPDIR:-/tmp}/cloudconsole-support.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT
mkdir -p "$work/logs"
report="$work/collection-status.txt"
errors=0
note_failure() {
    printf 'ERROR: %s\n' "$1" | tee -a "$report" >&2
    errors=$((errors + 1))
}

printf 'Station: %s\nCollected: %s UTC\n' "$station" "$(date -u '+%Y-%m-%d %H:%M:%S')" > "$report"
printf 'Bundle contains sensitive station data. Review before sharing.\n' >> "$report"
project="cc-$station"
if ! timeout 30 docker ps -a --filter "label=com.docker.compose.project=$project" \
    --format '{{.ID}} {{.Names}} {{.Status}}' > "$work/containers.txt" 2> "$work/docker-ps-error.txt"; then
    note_failure "Could not list station containers; see docker-ps-error.txt."
fi
if [ ! -s "$work/containers.txt" ]; then
    note_failure "No containers found for Compose project $project."
fi

while read -r id name _; do
    [ -n "${id:-}" ] || continue
    if ! timeout 60 docker logs --timestamps "$id" > "$work/logs/$name.log" 2>&1; then
        note_failure "Could not collect complete logs for $name; see its log file."
    fi
done < "$work/containers.txt"

if ! timeout 30 docker ps -a --filter "label=com.docker.compose.project=$project" \
    --filter 'label=com.docker.compose.service=postgres' --format '{{.ID}}' \
    > "$work/postgres-containers.txt" 2> "$work/postgres-list-error.txt"; then
    note_failure "Could not locate PostgreSQL container; see postgres-list-error.txt."
elif [ "$(wc -l < "$work/postgres-containers.txt")" -ne 1 ]; then
    note_failure "Expected one PostgreSQL container; see postgres-containers.txt."
else
    read -r postgres < "$work/postgres-containers.txt"
    if ! timeout 60 docker exec "$postgres" pg_dump -U cloud_console -d cloud_console \
        --format=custom > "$work/database.dump" 2> "$work/database-error.txt"; then
        rm -f -- "$work/database.dump"
        note_failure "Database backup failed or timed out; see database-error.txt."
    fi
fi

if [ -d "$log_dir/$station" ]; then
    if ! tar -C "$log_dir" -cf "$work/station-files.tar" "$station" 2> "$work/station-files-error.txt"; then
        note_failure "Could not collect all files from $log_dir/$station; see station-files-error.txt."
    fi
else
    printf 'No station log directory found at %s/%s; Docker logs collected separately.\n' "$log_dir" "$station" >> "$report"
fi

archive=$(mktemp --suffix=.tar.gz "$destination/cloudconsole-$station-$(date -u +%Y%m%dT%H%M%SZ).XXXXXXXX")
if ! tar -C "$work" -czf "$archive" .; then
    rm -f -- "$archive"
    echo "Failed to create support archive." >&2
    exit 1
fi
printf 'Support archive: %s\n' "$archive"
printf 'The archive includes a database dump and logs; it may contain credentials or private data.\n'
if [ "$errors" -ne 0 ]; then
    printf 'Collection incomplete: %s error(s). Review collection-status.txt in the archive.\n' "$errors" >&2
    exit 1
fi
