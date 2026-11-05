# Local server deployment (Docker Compose + Nginx Proxy Manager)

Runs one or more stations on a single local server with two network interfaces:

| NIC | Address | Used for |
|---|---|---|
| `eno3` | 192.168.87.207 | Audio: SRT/RTP inputs from the automation system |
| `eno4` | 192.168.50.207 | Data: web UI, browser microphone media, metadata |

Each station is its own Compose project (`cc-<name>`) with its own containers, database
volume, secrets, API key and port block. Nginx Proxy Manager (NPM) gives each station an
HTTPS name and forwards it to that station's web port.

Files: [docker-compose.local.yml](../docker-compose.local.yml) (pins ports to a NIC; used
together with `docker-compose.yml` and `docker-compose.prod.yml`) and the helper scripts in
`deploy/local/`.

> Status: the overlay and scripts were validated with `docker compose config` and a script
> test in a Linux container. They have not yet been run on your server.

## Quick answers

- **Where does it come from?** `install.sh` clones the GitHub repository
  (https://github.com/asalive1/CloudConsole) onto the server. Docker then **builds the images
  on the server** from that code the first time you start a station (about 10 to 25
  minutes). Nothing else needs to be downloaded by hand. The compose files do not pull from
  GitHub themselves; the clone is the code.
- **Will I be prompted?** Yes. Run `./deploy/local/new-station.sh` with no arguments and it
  asks for the station name, slot and address (Enter accepts the default). You can also pass
  them in one line: `./deploy/local/new-station.sh andy 1 192.168.50.207`.
- **Where are the .env files?** One per station, in `/opt/CloudConsole/deploy/local/stations/<name>.env`
  (for example `andy.env`). They are created for you and are never uploaded to GitHub.
  You do not need to create or edit `.env` by hand.
- **Names:** station names are lowercase; `ANDY` becomes `andy`.

## Step by step

All commands are typed on the server (SSH or console). Replace `andy` with your station name.

### Step 1. Install the tools (once)

On Ubuntu/Debian:

```bash
sudo apt update
sudo apt install -y git openssl curl
curl -fsSL https://get.docker.com | sudo sh        # installs Docker and the Compose plugin
sudo usermod -aG docker $USER                      # then log out and back in
docker compose version                             # must print v2.24 or newer
```

### Step 2. Download the code (once)

```bash
sudo mkdir -p /opt/CloudConsole && sudo chown $USER /opt/CloudConsole
curl -fsSL https://raw.githubusercontent.com/asalive1/CloudConsole/main/deploy/local/install.sh | bash
cd /opt/CloudConsole
```

(Or do it by hand: `git clone https://github.com/asalive1/CloudConsole.git /opt/CloudConsole`
then `chmod +x /opt/CloudConsole/deploy/local/*.sh`.)

### Step 3. Create the station

```bash
cd /opt/CloudConsole
./deploy/local/new-station.sh
```

It asks:

1. **Station name**: for example `andy`.
2. **Slot (0-9)**: it suggests the next free one. Each station needs a different slot,
   because the slot decides the ports (slot 0 = web port 8080, slot 1 = 8081, and so on).
3. **Address browsers use for microphone audio**: press Enter for 192.168.50.207. Enter your
   public IP instead only if guests connect from the internet.

At the end it prints **your station API key** (this is the sign-in password for the station)
and the list of ports. Copy the key somewhere safe. You can see it again any time:
`grep STATION_API_KEY deploy/local/stations/andy.env`.

### Step 4. Start it

```bash
./deploy/local/station.sh andy up      # first time: builds everything, be patient
./deploy/local/station.sh andy ps      # every service should say "running" / "healthy"
curl -I http://192.168.50.207:8080/api/health     # expect HTTP 200 (use your slot's port)
```

### Step 5. Add it to Nginx Proxy Manager

See section 4 below. After that, open `https://andy.example.com/` and sign in with the API key.

### Step 6. Update later

```bash
cd /opt/CloudConsole
./deploy/local/station.sh andy update      # git pull + rebuild + restart
```

Run `update` for every station. Your settings and data are kept.

## What the secrets and IDs in the env file mean

You never have to type these; `new-station.sh` generates them.

| Setting | What it is |
|---|---|
| `STATION_API_KEY` | The operator/admin password for this station. Different for every station. |
| `GUEST_JWT_SECRET` | Internal key that signs guest and mic session tokens. Keep private. |
| `STATION_ID` | A random unique ID for this station inside its own database. |
| `POSTGRES_PASSWORD` | Password for this station's private database. Not used from outside. |
| `JANUS_ROOM_ID` | The number of the audio "room" inside the Janus audio server where all microphones meet. Optional; the default (4815162342) is fine. Every station has its own Janus, so stations can share the same number without clashing. |
| `JANUS_CUE_ROOM_ID` | Same idea for the Cue bus. Leave it blank (the console derives one). |
| `JANUS_ROOM_PIN` | The room's password. The console and Janus use it between themselves; users never see it. |
| `WHIP_WHEP_USERNAME` / `PASSWORD` | Login for the optional WHIP/WHEP stream endpoint. |
| `AUDIO_BIND` / `DATA_BIND` / `METADATA_BIND` | Which NIC address each group of ports is published on (eno3 audio, eno4 data). |
| `HTTP_PORT`, `ENGINE_UDP_PORTS`, `METADATA_TCP_PORTS`, `JANUS_RTP_PORT_RANGE`, `MEDIAMTX_UDP_PORT` | The ports for this station's slot. |

The sample [.env.example](../.env.example) is only for developers running a single station
on a laptop. Ignore it for the server.
## 2. Multiple stations (slots and ports)

Give each station its own name and slot. Slots never overlap:

| Slot | Web (TCP, eno4) | Audio in (UDP, eno3) | Metadata (TCP, eno4) | Mic media (UDP, eno4) | WHIP/WHEP (UDP, eno4) |
|---|---|---|---|---|---|
| 0 | 8080 | 5000-5049 | 6000-6019 | 11000-11099 | 12000 |
| 1 | 8081 | 5050-5099 | 6020-6039 | 11100-11199 | 12001 |
| 2 | 8082 | 5100-5149 | 6040-6059 | 11200-11299 | 12002 |
| 3 | 8083 | 5150-5199 | 6060-6079 | 11300-11399 | 12003 |
| N | 8080+N | 5000+50N … 5049+50N | 6000+20N … 6019+20N | 11000+100N … 11099+100N | 12000+N |

```bash
deploy/local/new-station.sh wxyz 0
deploy/local/new-station.sh kabc 1
deploy/local/station.sh kabc up
```

Check that nothing else on the server already uses these ports (NPM itself uses 80, 81
and 443, which do not overlap). Each station needs the ports for the audio inputs you
actually configure; the SRT/RTP admin screen lets you choose any port inside the station's
block. Allow roughly 1 to 2 GB of RAM per station.

## 3. API keys

Every station has its own `STATION_API_KEY`, generated by `new-station.sh`. Operators enter
it on the sign-in screen for that station's URL; it also authorizes `/admin` and the API
(`X-API-Key` header).

To set your own key (or rotate it), edit `STATION_API_KEY=` in
`deploy/local/stations/<name>.env`, then recreate the backend:

```bash
deploy/local/station.sh wxyz up
```

Keys are not shared between stations: a key for `wxyz` is rejected by `kabc`. Keep the env
files private; they also hold the database and guest-link secrets.

## 4. Nginx Proxy Manager

Create one **Proxy Host** per station (Hosts → Proxy Hosts → Add):

| Field | Value (station `wxyz`, slot 0) |
|---|---|
| Domain Names | `wxyz.example.com` |
| Scheme | `http` |
| Forward Hostname / IP | `192.168.50.207` (the host's data address, not `localhost`) |
| Forward Port | `8080` (use 8081 for slot 1, and so on) |
| Cache Assets | off |
| Block Common Exploits | on |
| **Websockets Support** | **on** (required: live mixer updates and the microphone signalling use WebSockets) |

SSL tab: request a Let's Encrypt certificate, then enable **Force SSL** and **HTTP/2**. HTTPS
is required: browsers only allow microphone access on `https://` pages (or `localhost`).

**Custom locations: none are needed.** Each station's own Nginx already routes `/api/`,
`/api/mixer/ws`, `/janus` and `/media/`, so everything goes to a single proxy host.

**Advanced tab (recommended):** NPM closes idle connections after 60 seconds by default,
which drops the WebSockets. Paste:

```nginx
proxy_read_timeout 86400s;
proxy_send_timeout 86400s;
```

If NPM runs in Docker on the same server, forward to `192.168.50.207:808N` (a published
port), not a container name. If it runs on another machine, that machine must reach
192.168.50.207.

Media does **not** go through NPM. Browsers send microphone audio straight to the Janus UDP
ports on `JANUS_PUBLIC_ADDRESS`, so those must be reachable from the users (LAN, or router
forward for remote users).

## 5. URLs (examples)

| Purpose | URL |
|---|---|
| Operator console | `https://wxyz.example.com/` |
| Station admin | `https://wxyz.example.com/admin` |
| API docs | `https://wxyz.example.com/api/docs` |
| Health check | `https://wxyz.example.com/api/health` |
| Guest link | `https://wxyz.example.com/?invite=<token>` (created in the console, "Settings and guests") |
| Embedding in another web UI | `<iframe src="https://wxyz.example.com/" allow="microphone; autoplay">` |
| Direct (bypassing NPM, for testing) | `http://192.168.50.207:8080/` (microphone will not work over plain http) |

A second station is simply `https://kabc.example.com/` forwarded to port 8081.

## 6. Audio and metadata on the two NICs

- **Audio inputs** (SRT/RTP) are published only on 192.168.87.207, so senders must target
  that address and a UDP port in the station's range, e.g. SRT listener `192.168.87.207:5000`
  for slot 0. In admin, leave the network interface at the default (`eth0` inside the
  container): the host NIC is selected by the published address, not by that dropdown.
- **Metadata** TCP ports are published on 192.168.50.207. Automation systems connect to
  `192.168.50.207:6000` (slot 0) for a server-mode input or output. For client-mode
  endpoints, the engine connects out to the IP and port you enter.
- **Outbound connections** (SRT caller, RTP output, metadata client) leave through the server's
  routing table, not the published address. If outputs must use eno3, make sure the audio
  subnet (192.168.87.0/24) is routed via eno3 and the default route is on eno4.
- Docker publishes ports through its own firewall rules, which bypass UFW. Restrict the
  audio and metadata ports at the network level (VLAN or switch ACL) if needed.

## 7. Operations

```bash
deploy/local/station.sh wxyz logs            # recent logs
deploy/local/station.sh wxyz logs audio-engine
deploy/local/station.sh wxyz restart
deploy/local/station.sh wxyz down            # stop (data kept)
deploy/local/station.sh wxyz backup          # pg_dump to deploy/local/backups/
```

Update: `git pull`, then `deploy/local/station.sh <name> up` for each station.

Restore a backup into a running station:

```bash
cat deploy/local/backups/wxyz-<timestamp>.sql | \
  docker compose -p cc-wxyz --env-file deploy/local/stations/wxyz.env \
    -f docker-compose.yml -f docker-compose.prod.yml -f docker-compose.local.yml \
    exec -T postgres psql -U cloud_console cloud_console
```

(Restore into a freshly created station; existing rows will conflict.)

## 8. First-run checklist

- [ ] `deploy/local/station.sh wxyz ps` shows all services running
- [ ] `curl http://192.168.50.207:8080/api/health` returns HTTP 200
- [ ] `https://wxyz.example.com/` loads and the API key signs in
- [ ] An SRT/RTP input to `192.168.87.207:<port>` shows a level in the console
- [ ] A browser microphone connects and shows a level (confirms the Janus UDP ports)
- [ ] The metadata relay passes data on the metadata port
- [ ] A second station on slot 1 works at the same time
