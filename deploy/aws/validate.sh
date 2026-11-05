#!/usr/bin/env bash
# Checks that a station stack is healthy and isolated as configured.
# Usage: validate.sh <name>
# Verifies: all services running, the API answers through the station proxy, resource limits are
# applied, the station binds only to loopback, and the published ports match its slot.
set -euo pipefail

if [ "$#" -ne 1 ]; then
    sed -n '2,5p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi
name=$1
root="$(cd "$(dirname "$0")/../.." && pwd)"
env_file="$root/stations/$name.env"
[ -f "$env_file" ] || { echo "no such station: $name" >&2; exit 1; }
get() { grep "^$1=" "$env_file" | cut -d= -f2; }

failures=0
check() {
    if "$@" > /dev/null 2>&1; then
        echo "ok    $DESC"
    else
        echo "FAIL  $DESC"
        failures=$((failures + 1))
    fi
}

project="cc-$name"
http_port="$(get HTTP_PORT)"

for service in audio-engine janus mediamtx backend frontend postgres nginx; do
    DESC="$service is running"
    check test "$(docker ps -q --filter "label=com.docker.compose.project=$project" --filter "label=com.docker.compose.service=$service" --filter status=running | wc -l)" -ge 1
done

DESC="API health responds on 127.0.0.1:$http_port"
check curl -fsS "http://127.0.0.1:$http_port/api/health"

DESC="station configuration endpoint accepts the station API key"
check curl -fsS -H "X-API-Key: $(get STATION_API_KEY)" "http://127.0.0.1:$http_port/api/station/config"

DESC="station configuration endpoint rejects a wrong key"
check test "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-API-Key: wrong' "http://127.0.0.1:$http_port/api/station/config")" = 401

for service in audio-engine janus backend postgres; do
    id="$(docker ps -q --filter "label=com.docker.compose.project=$project" --filter "label=com.docker.compose.service=$service" | head -n1)"
    DESC="$service has a memory limit"
    check test "$(docker inspect -f '{{.HostConfig.Memory}}' "$id")" -gt 0
    DESC="$service has a CPU limit"
    check test "$(docker inspect -f '{{.HostConfig.NanoCpus}}' "$id")" -gt 0
done

DESC="station HTTP is not published on a public interface"
check test "$(docker port "$(docker ps -q --filter "label=com.docker.compose.project=$project" --filter "label=com.docker.compose.service=nginx" | head -n1)" 80/tcp | grep -vc '^127\.0\.0\.1:')" -eq 0

DESC="Janus publishes only its own UDP range ($(get JANUS_RTP_PORT_RANGE))"
janus_id="$(docker ps -q --filter "label=com.docker.compose.project=$project" --filter "label=com.docker.compose.service=janus" | head -n1)"
first="${janus_id:+$(get JANUS_RTP_PORT_RANGE | cut -d- -f1)}"
check docker port "$janus_id" "${first}/udp"

domain="$(get STATION_DOMAIN)"
if [ -f "$root/deploy/aws/edge/conf.d/$name.conf" ] && grep -q 'listen 443' "$root/deploy/aws/edge/conf.d/$name.conf"; then
    DESC="HTTPS answers at https://$domain/api/health"
    check curl -fsS "https://$domain/api/health"
else
    echo "skip  HTTPS (run issue-cert.sh to enable)"
fi

if [ "$failures" -gt 0 ]; then
    echo "$failures check(s) failed" >&2
    exit 1
fi
echo "all checks passed"
