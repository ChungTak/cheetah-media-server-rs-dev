# Phase 06: thin wrapper around the upstream Janus Gateway image.
# The upstream `canyan/janus-gateway` already exposes the HTTP API
# we need for the smoke test; this Dockerfile only adds a small
# helper script (`smoke.sh`) that drives the create / attach /
# message handshake from inside the container so the cheetah side
# can verify the round-trip without exposing the API to the host.

FROM canyan/janus-gateway:latest

USER root
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        curl \
        jq \
        bash && \
    rm -rf /var/lib/apt/lists/*

COPY smoke.sh /usr/local/bin/janus-smoke
RUN chmod +x /usr/local/bin/janus-smoke

# We override the entrypoint only when the operator explicitly
# wants the smoke driver. Default behaviour is still
# `/usr/local/bin/janus`.
ENV WEBRTC_INTEROP_JANUS_URL=http://127.0.0.1:8088/janus
