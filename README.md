# Cloud Console

Cloud Console is a browser-based broadcast console. The first sprint establishes the project foundation and its station-scoped persistence and API framework.

## Local development

Requirements: Docker Desktop with Compose enabled.

```powershell
Copy-Item .env.example .env
docker compose up --build -d
```

Open [http://localhost:8080](http://localhost:8080). The API health check is available at `/api/health`, and the OpenAPI document is at `/api/openapi.json`.

The Compose defaults are for local development only. Replace `STATION_API_KEY` and `GUEST_JWT_SECRET` in `.env` before using any non-local deployment. Do not commit `.env`.

The initial station API key is sent to API endpoints in the `X-API-Key` header. The browser console accepts it from the `api_key` URL parameter or a sign-in field and removes the URL parameter after reading it. Guest invitation links exchange for a guest-scoped JWT; guest sessions are created in a pending state and do not grant audio access before host approval.

## Project layout

```text
frontend/       React and TypeScript browser application
backend/        Rust and Axum API service
audio-engine/   reserved for the Phase 3 GStreamer service
migrations/     PostgreSQL migrations
nginx/          local reverse proxy
docs/           development standards and architecture notes
scripts/        developer utility scripts
```

The source-control ignore rules exclude `BuildPrompts/`; the prompt documents remain in the local workspace and are not part of this repository.

## Status

Phase 1 establishes the build, container, CI, and development conventions. Phase 2 provides station-scoped PostgreSQL migrations, direct station API-key validation, guest-link session exchange, health, configuration, and user endpoints. Audio transport and mixer behavior are later phases.
