#!/usr/bin/env bash
# Downloads (or updates) Cloud Console from GitHub on this server.
# Usage: install.sh [install-directory]      (default: /opt/CloudConsole)
# Run it with:  curl -fsSL https://raw.githubusercontent.com/asalive1/CloudConsole/main/deploy/local/install.sh | bash
set -euo pipefail

dir=${1:-/opt/CloudConsole}
repo=https://github.com/asalive1/CloudConsole.git

for tool in git docker openssl; do
    command -v "$tool" >/dev/null || { echo "'$tool' is not installed. Install it first (see docs/LOCAL_DEPLOYMENT.md, step 1)." >&2; exit 1; }
done
docker compose version >/dev/null 2>&1 || { echo "Docker Compose plugin is missing (docs/LOCAL_DEPLOYMENT.md, step 1)." >&2; exit 1; }

if [ -d "$dir/.git" ]; then
    echo "Updating $dir"
    git -C "$dir" pull --ff-only
else
    echo "Downloading to $dir"
    mkdir -p "$(dirname "$dir")" 2>/dev/null || true
    git clone "$repo" "$dir"
fi
chmod +x "$dir"/deploy/local/*.sh
cat <<EOF

Done. Next:
  cd $dir
  ./deploy/local/new-station.sh      (answers a few questions)
  ./deploy/local/station.sh <name> up
EOF
