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

## Selecting host audio and data interfaces

On native Linux, new stations use host networking for the audio engine so the interface
drop-downs list the server's real interfaces and addresses. Select the appropriate interface
for each RTP or SRT input/output and each TCP metadata endpoint. With no explicit selection,
audio uses `AUDIO_BIND` and metadata uses `METADATA_BIND`. The web UI and browser media
remain bound to `DATA_BIND`. Change these addresses in the station's `.env` file and
recreate the services if the server addresses change. All must be assigned to the host.
The host routing table and firewall must also permit traffic to the chosen destination;
selecting a source interface does not create routes.

Existing stations retain their previous Docker bridge networking until migrated. Before
migration, record existing network input and metadata settings, verify their listener
ports are within the station's configured ranges, and replace any saved container-only
interface such as `eth0` with the correct host interface. Host-mode audio RTP/SRT listeners
are restricted to `ENGINE_UDP_PORTS`, and TCP metadata listeners to `METADATA_TCP_PORTS`.
These limits do not restrict remote destination ports for outgoing RTP, SRT callers, or
metadata clients. A failed configuration is saved but reported as not applied; correct
the saved item and try again. Confirm the engine logs if a pipeline later fails.

For an existing station, add `HOST_AUDIO_NETWORK=true` to its local `.env`, then run
`./deploy/local/station.sh <name> up`. This recreates services and briefly interrupts audio.
The wrapper continues to include any station-specific ALSA overlay. To roll back, first
stop and remove the host-mode proxy, then remove `HOST_AUDIO_NETWORK=true` and run the
same command. For example, for station `wxyz`:

```bash
docker compose -p cc-wxyz --env-file deploy/local/stations/wxyz.env \
  -f docker-compose.yml -f docker-compose.prod.yml -f docker-compose.local.yml \
  -f docker-compose.host-audio.yml stop engine-proxy
docker compose -p cc-wxyz --env-file deploy/local/stations/wxyz.env \
  -f docker-compose.yml -f docker-compose.prod.yml -f docker-compose.local.yml \
  -f docker-compose.host-audio.yml rm -f engine-proxy
```

Saved host interface names may need to be updated for bridge networking. Avoid direct
Compose lifecycle commands unless they include
`docker-compose.host-audio.yml` after the local overlay.

Host networking bypasses Docker's published-port firewall for the engine: protect the
audio and data NICs with host/network firewall rules, and only open required station
listener ports to trusted peers. Multicast requires network and switch support. The
unauthenticated engine control API uses a root-only Unix socket in a per-station Docker
volume; a proxy exposes it only to that station's Docker network. Only trusted containers
should be attached to that network, and Docker administrators have access. The Janus
bridge binds the station Docker network's gateway and uses UDP ports 10500-10507 (two
per slot); these are internal bridge ports, not published on the physical NICs.
Docker Desktop is not equivalent to native Linux for host networking.

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
2. **Slot (0-3)**: it suggests the next free one. Each station needs a different slot,
   because the slot decides the ports (slot 0 = web port 7002, slot 1 = 7003, and so on).
3. **Address browsers use for microphone audio**: press Enter for 192.168.50.207. Enter your
   public IP instead only if guests connect from the internet.

At the end it prints **your station API key** (this is the sign-in password for the station)
and the list of ports. Copy the key somewhere safe. You can see it again any time:
`grep STATION_API_KEY deploy/local/stations/andy.env`.

### Step 4. Start it

```bash
./deploy/local/station.sh andy up      # first time: builds everything, be patient
./deploy/local/station.sh andy ps      # every service should say "running" / "healthy"
curl -I http://192.168.50.207:7002/api/health     # expect HTTP 200 (use your slot's port)
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
| 0 | 7002 | 7010-7019 | 7010-7019 | 7020-7099 | 7100 |
| 1 | 7003 | 7110-7119 | 7110-7119 | 7120-7199 | 7200 |
| 2 | 7004 | 7210-7219 | 7210-7219 | 7220-7299 | 7300 |
| 3 | 7005 | 7310-7319 | 7310-7319 | 7320-7399 | 7400 |

```bash
deploy/local/new-station.sh wxyz 0
deploy/local/new-station.sh kabc 1
deploy/local/station.sh kabc up
```

Check that nothing else on the server already uses these ports. Each station needs the ports for the audio inputs you
actually configure; the SRT/RTP admin screen lets you choose any port inside the station's
block. Allow roughly 1 to 2 GB of RAM per station.

The station blocks are 7010-7109, 7110-7209, 7210-7309 and 7310-7409.
TCP and UDP can use the same numbers independently. Each block provides 10 UDP
audio listeners, 80 Janus UDP ports, one MediaMTX UDP port and 10 TCP metadata
listeners. Ten microphone/guest users joining both Program and Cue can consume
roughly 40 Janus ports, depending on ICE; test the actual deployment at full load.
The generic RTP/SRT input defaults in the administration screen do not follow the
slot: explicitly select an audio port from this table.

### Migrating an existing station

Existing station env files and saved audio/metadata endpoints are not changed by an
update. Before recreating a station, back up its configuration and database, then
edit its env file to use the table above (`HTTP_PORT`, `ENGINE_UDP_PORTS`,
`METADATA_TCP_PORTS`, `JANUS_RTP_PORT_RANGE` and `MEDIAMTX_UDP_PORT`). Update
saved RTP/SRT listener and metadata server ports in Station Administration, their
senders/clients, NPM's upstream port, router forwarding and firewall rules. Recreate
the stack with `deploy/local/station.sh <name> up`. Existing slots above 3 retain
their old ports until manually reassigned to a free supported slot. Coordinate the
cutover because changing ports interrupts existing connections.

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

NPM is managed separately from this repository. To keep its host listeners in the
7000 range, configure its deployment to publish TCP `7000:80` (HTTP), `7001:443`
(HTTPS) and, for example, `7006:81` (administration), instead of publishing 80,
443 and 81 directly. Forward public TCP 80 to host port 7000 and public TCP 443
to host port 7001 for normal HTTPS URLs and Let's Encrypt HTTP-01 renewal. Restrict
the administrative port to trusted users. Configure forwarding on the router or
an external TCP proxy; publishing these ports alone does not provide public port
translation. The AWS edge also uses 7000/7001, so it cannot share those listeners
on the same IP with NPM.

Create one **Proxy Host** per station (Hosts → Proxy Hosts → Add):

| Field | Value (station `wxyz`, slot 0) |
|---|---|
| Domain Names | `wxyz.example.com` |
| Scheme | `http` |
| Forward Hostname / IP | `192.168.50.207` (the host's data address, not `localhost`) |
| Forward Port | `7002` (use 7003 for slot 1, and so on) |
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

If NPM runs in Docker on the same server, forward to the station's published port (for
example `192.168.50.207:7002`), not a container name. If it runs on another machine, that machine must reach
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
| Direct (bypassing NPM, for testing) | `http://192.168.50.207:7002/` (microphone will not work over plain http) |

A second station is simply `https://kabc.example.com/` forwarded to port 7003.

## 6. Audio and metadata on the two NICs

- **Audio inputs** (SRT/RTP) are published only on 192.168.87.207, so senders must target
  that address and a UDP port in the station's range, e.g. SRT listener `192.168.87.207:7010`
  for slot 0. In admin, leave the network interface at the default (`eth0` inside the
  container): the host NIC is selected by the published address, not by that dropdown.
- **Metadata** TCP ports are published on 192.168.50.207. Automation systems connect to
  `192.168.50.207:7010` (slot 0) for a server-mode input or output. For client-mode
  endpoints, the engine connects out to the IP and port you enter.
- **Outbound connections** (SRT caller, RTP output, metadata client) leave through the server's
  routing table, not the published address. If outputs must use eno3, make sure the audio
  subnet (192.168.87.0/24) is routed via eno3 and the default route is on eno4.
- Docker publishes ports through its own firewall rules, which bypass UFW. Restrict the
  audio and metadata ports at the network level (VLAN or switch ACL) if needed.

### ALSA devices and loopback audio

ALSA is optional. On a Linux Docker host with `/dev/snd` and `/proc/asound/pcm`,
create `deploy/local/stations/<name>.alsa.yml`:

```yaml
services:
  audio-engine:
    devices:
      - /dev/snd:/dev/snd
    volumes:
      - /proc/asound:/host/asound:ro
    environment:
      ALSA_PCM_PATH: /host/asound/pcm
```

The station wrapper automatically includes this station-specific overlay for all
operations. Recreate the audio engine with `deploy/local/station.sh <name> up`;
this interrupts audio while the containers are recreated. The overlay grants the
engine access to host sound devices; use only for trusted station containers.
The inventory mount is read-only and outside `/proc`, where Docker blocks mounts.
If ALSA hardware or loopback devices are added after the container starts, recreate
the audio engine so Docker can update its device access.

Inspect host playback with `cat /proc/asound/pcm` and
`cat /proc/asound/card*/pcm*p/sub*/hw_params` while audio is playing. ALSA
loopback typically sends playback device 0 to capture device 1, or playback 1 to
capture 0, on the same card and subdevice. For example, playback `hw:0,0,3`
would use capture `hw:0,1,3`. The list shows card/device pairs, not individual
subdevices: enter the full identifier manually when needed. Match the playback
rate, channels, and sample format; raw `hw:` does not convert formats. Confirm
that a source opens and its meter moves, and review `station.sh <name> logs audio-engine`
and backend logs if it does not.

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
- [ ] `curl http://192.168.50.207:7002/api/health` returns HTTP 200
- [ ] `https://wxyz.example.com/` loads and the API key signs in
- [ ] An SRT/RTP input to `192.168.87.207:<port>` shows a level in the console
- [ ] A browser microphone connects and shows a level (confirms the Janus UDP ports)
- [ ] The metadata relay passes data on the metadata port
- [ ] A second station on slot 1 works at the same time
