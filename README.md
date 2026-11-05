# Cloud Console

Cloud Console is a browser-based broadcast console for station operators, hosts, co-hosts, and invited guests.

## Local development

Requirements: Docker Desktop with Compose enabled.

```powershell
Copy-Item .env.example .env
docker compose up --build -d
```

Open [http://localhost:8080](http://localhost:8080). The API health check is available at `/api/health`, and the OpenAPI document is at `/api/openapi.json`.

The Compose defaults are for local development only. Replace `STATION_API_KEY`, `GUEST_JWT_SECRET`, `JANUS_ROOM_PIN`, and the WHIP/WHEP credentials in `.env` before using any non-local deployment. Do not commit `.env`.

The station API key is sent to operator endpoints in the `X-API-Key` header. The console accepts it from the `api_key` URL parameter or sign-in field and removes the URL parameter after reading it. Guest invitation links use `/?invite=<token>`; a guest registers a name and remains muted and disconnected until an operator approves the request. Pending requests expire after 20 minutes. Approved guests can reconnect through the same invitation while it remains valid.

## Project layout

```text
frontend/       React and TypeScript browser application
backend/        Rust and Axum API service
audio-engine/   Rust/GStreamer Program audio pipeline
migrations/     PostgreSQL migrations
nginx/          local reverse proxy
janus/          Janus Gateway image and runtime configuration
docs/           development standards and architecture notes
scripts/        developer utility scripts
```

The source-control ignore rules exclude `BuildPrompts/`; the prompt documents remain in the local workspace and are not part of this repository.

## Status

Phase 1 establishes the build, container, CI, and development conventions. Phase 2 provides station-scoped PostgreSQL migrations, direct station API-key validation, guest-link session exchange, health, configuration, and user endpoints. Phase 3 adds a separate GStreamer service with configurable RTP, SRT, and ALSA sources; Program bus output routing; pipeline recovery; and audio telemetry. Phase 4 adds Janus VideoRoom audio-only connections for hosts, co-hosts, and approved guests, persisted connection state, approval/rejection, and reconnect support. MediaMTX exposes separate WHIP publish and WHEP read endpoints for external streams.

The audio engine is internal to the Compose network. Station-authenticated engine requests are proxied through `/api/audio/*`. For example, an RTP Automation source defaults to stereo L24 on UDP port 5005; the source and output transports are configurable through the audio API. The SRT program output defaults to caller mode with AAC in MPEG-TS, while SRT inputs default to listener mode. ALSA outputs support 48 kHz at 16- or 24-bit depth. Physical ALSA devices are not available in the Windows/Docker Desktop validation environment; configure a Linux ALSA device for hardware playback.

## Phase 4 media configuration

Janus signaling is proxied at `/janus`; its WebRTC media range is UDP `11000-11099` (about two ports per connected microphone per room). When testing on a single machine, set `JANUS_PUBLIC_ADDRESS=127.0.0.1` so browsers receive a reachable ICE candidate. MediaMTX provides WHIP and WHEP at `/media/<stream-name>/whip` and `/media/<stream-name>/whep`, with WebRTC media on UDP `12000`. Both media services need inbound UDP reachability on their published ports for direct ICE connectivity.

Set `JANUS_PUBLIC_ADDRESS` to the public IP address when the host is behind 1:1 NAT. Configure `STUN_SERVER` and, where restrictive NAT or firewalls require relaying, `TURN_SERVER_URL`, `TURN_SERVER_USERNAME`, and `TURN_SERVER_CREDENTIAL`. MediaMTX stream endpoints use HTTP Basic authentication from `WHIP_WHEP_USERNAME` and `WHIP_WHEP_PASSWORD`. Janus WebSocket upgrades are authorized against the participant JWT before NGINX forwards the connection.

The local NGINX listener is plain HTTP and is suitable only for localhost development. Remote browser microphone access and production WHIP/WHEP should be served over HTTPS/WSS by a TLS-terminating reverse proxy that preserves `/api/`, `/janus`, and `/media/`. WHIP/WHEP streams are currently separate from the Janus studio room and Program bus; no automatic audio bridge or mixing between those paths is configured.
