FROM rust:1.89-slim AS build
WORKDIR /app
COPY backend/Cargo.toml ./backend/Cargo.toml
COPY backend/Cargo.lock ./backend/Cargo.lock
COPY backend/src ./backend/src
COPY backend/openapi.json ./backend/openapi.json
COPY migrations ./migrations
WORKDIR /app/backend
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /app/backend/target/release/cloud-console-api /usr/local/bin/cloud-console-api
EXPOSE 8000
ENTRYPOINT ["cloud-console-api"]
