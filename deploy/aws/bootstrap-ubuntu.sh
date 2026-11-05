#!/usr/bin/env bash
# Prepares a fresh Ubuntu 24.04 EC2 instance: Docker Engine with the Compose plugin,
# UDP socket buffer sizes suited to real-time audio, and the backup directory.
# Usage: sudo deploy/aws/bootstrap-ubuntu.sh
set -euo pipefail

[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo)" >&2; exit 1; }
. /etc/os-release
[ "${ID:-}" = ubuntu ] && [ "${VERSION_ID:-}" = "24.04" ] || echo "warning: tested on Ubuntu 24.04 only" >&2

if ! command -v docker > /dev/null; then
    apt-get update
    apt-get install -y ca-certificates curl gnupg openssl
    install -m 0755 -d /etc/apt/keyrings
    curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
    chmod a+r /etc/apt/keyrings/docker.asc
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu ${VERSION_CODENAME} stable" \
        > /etc/apt/sources.list.d/docker.list
    apt-get update
    apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
fi
systemctl enable --now docker

cat > /etc/sysctl.d/90-cloud-console.conf <<'EOF'
net.core.rmem_max = 8388608
net.core.wmem_max = 8388608
net.core.rmem_default = 1048576
net.core.wmem_default = 1048576
net.core.netdev_max_backlog = 5000
EOF
sysctl --system > /dev/null

install -d -m 700 /var/backups/cloud-console
echo "Bootstrap complete. Next: deploy/aws/new-station.sh <name> <slot> <domain> <public-ip>"
