# GStreamer WHIP/WHEP helper

Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`).

GStreamer's `webrtcbin` element is the canonical reference
implementation of the W3C / RFC pipeline; running cheetah against
it catches both wire-shape regressions and codec interop bugs.

This directory documents the **command-line invocations** the
nightly lab uses against `webrtcbin`. We do not ship a custom
binary — `gst-launch-1.0` is enough for the WHIP path, and the
WHEP path uses `webrtcsrc` with a tiny bash wrapper.

## Prereqs

```bash
sudo apt install gstreamer1.0-tools \
                 gstreamer1.0-plugins-{base,good,bad,ugly} \
                 gstreamer1.0-libav \
                 gstreamer1.0-nice \
                 gstreamer1.0-plugins-bad-apps  # whipclientsink/whepclientsrc
```

## WHIP publisher (videotestsrc → WHIP endpoint)

```bash
gst-launch-1.0 \
  videotestsrc is-live=true ! \
  videoconvert ! \
  vp8enc deadline=1 ! \
  rtpvp8pay ! \
  application/x-rtp,media=video,encoding-name=VP8,payload=96 ! \
  whipclientsink whip-endpoint="$WEBRTC_INTEROP_ZLM_WHIP_URL"
```

Set `GST_DEBUG=3,webrtc*:5` to capture verbose webrtc logs and
redirect them to `$WEBRTC_INTEROP_ARTIFACT_DIR/peer.log`.

## WHEP player (WHEP endpoint → fakesink, log first frame)

```bash
gst-launch-1.0 \
  whepclientsrc whep-endpoint="$WEBRTC_INTEROP_ZLM_WHEP_URL" ! \
  rtpvp8depay ! \
  vp8dec ! \
  fakesink dump=true
```

Capture a 10 s sample with `--eos-on-shutdown` and a `timeout` so
the test exits cleanly:

```bash
timeout 10 gst-launch-1.0 -e \
  whepclientsrc whep-endpoint="$WEBRTC_INTEROP_ZLM_WHEP_URL" ! \
  rtpvp8depay ! vp8dec ! fakesink \
  > "$WEBRTC_INTEROP_ARTIFACT_DIR/peer.log" 2>&1
```

## docker-compose integration

`interop-docker-compose.yml` does **not** pull a GStreamer image by
default — the toolchain is large and the harness already supports
swapping in a different helper at runtime. To add GStreamer to the
lab:

1. Build a minimal image: `FROM debian:12-slim` + apt install of
   the packages above.
2. Add a `gstreamer-helper` service with `profiles: [gstreamer]`
   pointing at the built image.
3. Mirror the `pion-helper` env-var contract (read
   `WEBRTC_INTEROP_ZLM_WHIP_URL` / `WEBRTC_INTEROP_ZLM_WHEP_URL`).

## Status

This directory is documentation-grade. Nightly CI runs the
ZLMediaKit + cheetah path via the existing harness; GStreamer
joins once the image lands in the compose file.
