# AWS deployment (Ubuntu 24.04, EC2, Docker Compose)

Target: up to 4 stations per EC2 instance, 14 channels per station, sub-second audio.
Each station is its own Compose project (`cc-<name>`) with its own network, Postgres
volume, secrets and port block. A shared `cc-edge` nginx terminates TLS.

> Status: tooling is validated locally (Compose config, shellcheck, unit tests).
> It has **not** been run on a real EC2 instance. Use the checklist below to validate.

## Sizing

Start with a compute-optimised instance such as `c6i.4xlarge` (16 vCPU, 32 GB) for 4 stations.
Default per-station limits (override in the station env file): engine 1.5 CPU / 1.5 GB,
see `docker-compose.prod.yml` for the rest. Raise limits if the load test shows throttling.

## Port plan (slot N = 0..3)

| Purpose | Port |
|---|---|
| Station HTTP (127.0.0.1 only) | 7002+N |
| Engine audio in (UDP) | 7010+100N .. 7019+100N |
| Metadata relay (TCP) | 7010+100N .. 7019+100N |
| Janus WebRTC media (UDP) | 7020+100N .. 7099+100N |
| MediaMTX (UDP) | 7100+100N |
| Edge HTTP / HTTPS listeners (TCP) | 7000 / 7001 |

Station slots 0-3 use blocks 7010-7109, 7110-7209, 7210-7309 and
7310-7409. TCP and UDP port numbers are independent. The 80-port Janus range
is sized with headroom for roughly 40 ports for ten microphone/guest users joining
both Program and Cue; ICE candidates and other simultaneous listeners can increase
usage. Load-test actual usage. Configure RTP/SRT listener ports explicitly within
the station's audio range; the generic input defaults do not follow the slot.

The edge requires external TCP forwarding from public port 80 to the instance's
port 7000 and from public port 443 to port 7001. Public port 80 is required for
Let's Encrypt HTTP-01 issuance and renewal; public port 443 preserves normal HTTPS
URLs. An EC2 security group does not perform port translation: provision a separate
TCP-capable load balancer or other forwarding device and allow its traffic through
the instance's firewall/security group. Do not forward UDP media through this
HTTP/HTTPS mapping. The forwarding device must pass TLS through to the edge.

Open only the ports you use in the security group; `new-station.sh` prints them.
Restrict SRT/RTP/metadata ports to the sender's IP ranges.

## Steps

1. Launch Ubuntu 24.04; configure the forwarding described above and point each
   station's DNS name at its public address. Set `JANUS_PUBLIC_ADDRESS` to the
   address at which browsers can directly reach the instance's UDP media ports.
2. `sudo deploy/aws/bootstrap-ubuntu.sh` (Docker, Compose, UDP buffer sysctls, backup dir).
3. `deploy/aws/new-station.sh <name> <slot> <domain> <elastic-ip>` creates `stations/<name>.env`
   with random secrets.
4. `deploy/aws/station.sh <name> build && deploy/aws/station.sh <name> up`
5. `deploy/aws/issue-cert.sh <name>` obtains a Let's Encrypt certificate and enables TLS.
6. `deploy/aws/validate.sh <name>`

### Migrating existing stations

Existing station env files and generated edge configurations are not updated by
pulling new code. Back up the env files and database first. For each station, edit
`HTTP_PORT`, `ENGINE_UDP_PORTS`, `METADATA_TCP_PORTS`, `JANUS_RTP_PORT_RANGE`
and `MEDIAMTX_UDP_PORT` according to its slot above. Update saved RTP/SRT listener
and metadata server endpoints, external senders/clients, UDP forwarding and firewall
rules. Recreate the station using `deploy/aws/station.sh <name> up`.

For each already-issued certificate, regenerate `deploy/aws/edge/conf.d/<name>.conf`
from `station-tls.conf.template`, substituting its domain and new `HTTP_PORT`;
for stations without a certificate use `station-http.conf.template`. Verify and
reload nginx with `docker compose -p cc-edge -f deploy/aws/edge/docker-compose.yml
exec -T nginx nginx -t` and then the same command with `nginx -s reload`.
Set up the public forwarding before the cutover; reissuing a certificate is not
required. Coordinate this change since existing connections will be interrupted.

## Backup and recovery

- `deploy/aws/backup.sh <name>` writes a mode-600 archive (pg_dump + env file) to
  `/var/backups/cloud-console`; set `S3_URI` to upload and `KEEP_DAYS` to prune.
  Schedule it with cron, e.g. `15 3 * * * /opt/CloudConsole/deploy/aws/backup.sh <name>`.
- Instance loss: repeat steps 1-4 on a new instance, then
  `deploy/aws/restore.sh <name> <archive>` and restart the station.
- Test a restore regularly.

## Validation checklist (exit criterion)

- [ ] `validate.sh` passes for every station
- [ ] HTTPS works, HTTP redirects, certificate renews (`certbot renew --dry-run`)
- [ ] 14 channels per station active on all 4 stations at once: no CPU throttling
- [ ] End-to-end latency (SRT in to browser monitor) under 1 s
- [ ] Metadata relay delivers delayed output in step with the program delay
- [ ] Backup then restore on a fresh instance reproduces the station config
- [ ] Stopping one station does not affect the others
