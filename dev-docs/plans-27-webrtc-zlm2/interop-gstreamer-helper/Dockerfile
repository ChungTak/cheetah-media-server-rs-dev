# Phase 06: GStreamer WHIP/WHEP helper image used by the cheetah
# interop lab. Wraps `gst-launch-1.0` with the `whipclientsink` /
# `whepclientsrc` plugins (gst-plugins-bad >= 1.22) so the harness
# can drive both publish and play paths against a running cheetah
# / ZLMediaKit instance.
#
# The image is intentionally fat (~600 MB) — production deployments
# rarely need GStreamer in the same container, so we trade size for
# a one-shot working dev environment. The nightly CI runs it
# behind `--profile gstreamer` (see `interop-docker-compose.yml`).

FROM debian:12-slim

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        gstreamer1.0-tools \
        gstreamer1.0-plugins-base \
        gstreamer1.0-plugins-good \
        gstreamer1.0-plugins-bad \
        gstreamer1.0-plugins-ugly \
        gstreamer1.0-libav \
        gstreamer1.0-nice \
        gstreamer1.0-x \
        ca-certificates \
        curl \
        bash && \
    rm -rf /var/lib/apt/lists/*

# Make the helper script available on PATH.
COPY entrypoint.sh /usr/local/bin/cheetah-gst-helper
RUN chmod +x /usr/local/bin/cheetah-gst-helper

ENV WEBRTC_INTEROP_ARTIFACT_DIR=/artifacts
ENV GST_DEBUG=3,webrtc*:5

# Run as non-root for parity with the Pion helper.
RUN useradd --create-home --uid 1000 gst
USER gst

ENTRYPOINT ["/usr/local/bin/cheetah-gst-helper"]
