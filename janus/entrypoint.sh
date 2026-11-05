#!/bin/sh
set -eu

config_dir=/tmp/cloud-console-janus-config
cp -R /var/janus/janus/etc/janus "$config_dir"
sed -i "s/#rtp_port_range = \"20000-40000\"/rtp_port_range = \"${JANUS_RTP_PORT_RANGE:-11000-11099}\"/" "$config_dir/janus.jcfg"

set -- -F "$config_dir"
if [ -n "${DOCKER_IP:-}" ]; then
    set -- "$@" --nat-1-1="$DOCKER_IP"
fi

exec /var/janus/janus/bin/janus "$@"
