#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/support" "$tmp/logs/test"
printf 'file log\n' > "$tmp/logs/test/file.log"
cat > "$tmp/bin/docker" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
    ps)
        if [[ "$*" == *"service=postgres"* ]]; then
            echo db123
        else
            printf 'db123 cc-test-postgres-1 Up\napp123 cc-test-backend-1 Exited\n'
        fi
        ;;
    logs) printf 'logs for %s\n' "$3" ;;
    exec)
        if [ "${FAIL_DUMP:-0}" = 1 ]; then
            echo 'database unavailable' >&2
            exit 1
        fi
        printf 'synthetic database dump\n'
        ;;
    *) exit 2 ;;
esac
EOF
chmod +x "$tmp/bin/docker"
export PATH="$tmp/bin:$PATH" CC_SUPPORT_DIR="$tmp/support" CC_LOG_DIR="$tmp/logs"

bash "$root/deploy/local/cc-support.sh" test > "$tmp/output"
archive=$(sed -n 's/^Support archive: //p' "$tmp/output")
test -f "$archive"
test "$(stat -c '%a' "$archive")" = 600
mkdir "$tmp/extracted"
tar -xzf "$archive" -C "$tmp/extracted"
grep -q 'logs for db123' "$tmp/extracted/logs/cc-test-postgres-1.log"
grep -q 'logs for app123' "$tmp/extracted/logs/cc-test-backend-1.log"
grep -q 'synthetic database dump' "$tmp/extracted/database.dump"
tar -tf "$tmp/extracted/station-files.tar" | grep -q 'test/file.log'

if FAIL_DUMP=1 bash "$root/deploy/local/cc-support.sh" test > "$tmp/failure-output" 2>&1; then
    echo "Missing database dump was reported as success" >&2
    exit 1
fi
failed_archive=$(sed -n 's/^Support archive: //p' "$tmp/failure-output")
test -f "$failed_archive"
mkdir "$tmp/failed"
tar -xzf "$failed_archive" -C "$tmp/failed"
test ! -e "$tmp/failed/database.dump"
grep -q 'Database backup failed' "$tmp/failed/collection-status.txt"
grep -q 'database unavailable' "$tmp/failed/database-error.txt"
echo "Support collection tests passed"
