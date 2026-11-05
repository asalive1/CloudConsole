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
| Station HTTP (127.0.0.1 only) | 8080+N |
| Engine audio in (UDP) | 5000+50N .. 5049+50N |
| Metadata relay (TCP) | 6000+20N .. 6019+20N |
| Janus WebRTC media (UDP) | 11000+100N .. 11099+100N |
| MediaMTX (UDP) | 12000+N |
| Edge (TCP) | 80, 443 |

Open only the ports you use in the security group; `new-station.sh` prints them.
Restrict SRT/RTP/metadata ports to the sender's IP ranges.

## Steps

1. Launch Ubuntu 24.04 with an Elastic IP; point a DNS name at it per station.
2. `sudo deploy/aws/bootstrap-ubuntu.sh` (Docker, Compose, UDP buffer sysctls, backup dir).
3. `deploy/aws/new-station.sh <name> <slot> <domain> <elastic-ip>` creates `stations/<name>.env`
   with random secrets.
4. `deploy/aws/station.sh <name> build && deploy/aws/station.sh <name> up`
5. `deploy/aws/issue-cert.sh <name>` obtains a Let's Encrypt certificate and enables TLS.
6. `deploy/aws/validate.sh <name>`

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
