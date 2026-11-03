# Development standards

## Branching and pull requests

- Keep `main` deployable.
- Use short-lived branches named `feature/<summary>`, `fix/<summary>`, or `chore/<summary>`.
- Open a pull request for changes to `main`; describe behavior changes and include focused validation results.
- Require the CI workflow to pass before merging. Prefer squash merges for a readable project history.

## Rust

- Format with `cargo fmt --all -- --check`.
- Require `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`.
- Keep API handlers thin; isolate database operations and authentication checks from response construction.
- Return explicit HTTP errors; do not silently substitute defaults for invalid configuration or failed persistence.

## React and TypeScript

- Keep application code in `frontend/src`.
- Use strict TypeScript and functional React components.
- Run `npm ci` followed by `npm run build` in CI.
- Do not place station API keys in persistent browser storage.

## API

- Prefix application routes with `/api`.
- Describe endpoints and schemas in `backend/openapi.json`.
- Use JSON request and response bodies, stable error shapes, and explicit authentication requirements.
- Use the `X-API-Key` header for station API requests. Guest sessions use a scoped bearer token and do not receive station-key privileges.

## Configuration and secrets

- Use environment variables for service configuration.
- Keep local overrides in `.env`, which is ignored by Git; update `.env.example` when configuration changes.
- Compose credentials are development-only and must be replaced for deployed environments.
- Never add `BuildPrompts/` to source control.
