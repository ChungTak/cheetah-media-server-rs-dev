# Phase 06: cheetah-server image used by the interop lab.
#
# This wraps the workspace's `cheetah-server` binary in a slim
# runtime container so the docker-compose lab can run the cheetah
# WebRTC stack alongside ZLMediaKit / Pion / GStreamer / Janus on
# a shared host network. The image is built from the workspace
# root (`docker build -f .../Dockerfile .`); the workspace path
# layout is preserved so cargo's path dependencies resolve.

FROM rust:1.83-slim AS builder

ENV CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    DEBIAN_FRONTEND=noninteractive

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /work
COPY . .

# Build the server with WebRTC + RTMP + RTSP. Add more --features
# entries when other protocols are needed in the lab.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo build --release \
        -p cheetah-server \
        --features webrtc \
        && cp target/release/cheetah-server /usr/local/bin/cheetah-server

FROM debian:12-slim
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        curl && \
    rm -rf /var/lib/apt/lists/*

# Copy the binary and the example config. The compose stanza
# overrides the config path with a mount when the operator wants
# to iterate.
COPY --from=builder /usr/local/bin/cheetah-server /usr/local/bin/cheetah-server
COPY --from=builder /work/config.example.yaml /etc/cheetah/config.yaml

# Default ports (overridable via config mount):
#   1935  RTMP plain
#   1936  RTMPS
#   554   RTSP
#   8000  WebRTC UDP single-port
#   8088  WebRTC TCP single-port (when enabled)
#   8891  control plane
EXPOSE 1935 1936 554 8000/udp 8088 8891

# Run as non-root for parity with the other helper containers.
RUN useradd --create-home --uid 1000 cheetah && \
    mkdir -p /etc/cheetah && \
    chown -R cheetah:cheetah /etc/cheetah
USER cheetah

ENV CHEETAH_CONFIG=/etc/cheetah/config.yaml \
    RUST_LOG=info,cheetah=debug

ENTRYPOINT ["cheetah-server"]
