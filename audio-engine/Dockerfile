FROM rust:1.89-slim AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config \
        libgstreamer1.0-dev \
        libgstreamer-plugins-base1.0-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY audio-engine/Cargo.toml ./audio-engine/Cargo.toml
COPY audio-engine/Cargo.lock ./audio-engine/Cargo.lock
COPY audio-engine/src ./audio-engine/src
WORKDIR /app/audio-engine
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        gstreamer1.0-plugins-base \
        gstreamer1.0-plugins-good \
        gstreamer1.0-plugins-bad \
        gstreamer1.0-plugins-ugly \
        gstreamer1.0-libav \
        gstreamer1.0-alsa \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /app/audio-engine/target/release/cloud-console-audio-engine /usr/local/bin/cloud-console-audio-engine
EXPOSE 8001
ENTRYPOINT ["cloud-console-audio-engine"]
