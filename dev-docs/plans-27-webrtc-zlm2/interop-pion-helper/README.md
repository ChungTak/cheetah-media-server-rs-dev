# Pion WHIP/WHEP helper

Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`).

This helper image is the Pion-side counterpart to the cheetah
WebRTC interop tests under `crates/protocols/webrtc/module/tests/interop.rs`.
It runs a tiny Go binary that can either publish a synthetic VP8
stream to a WHIP endpoint, or play a stream from a WHEP endpoint
and report first-keyframe latency.

## Build locally

```bash
docker build -t cheetah-pion-helper:dev dev-docs/plans-27-webrtc-zlm2/interop-pion-helper
```

## Run a WHIP publisher

```bash
docker run --rm --network host \
  -e WEBRTC_INTEROP_ARTIFACT_DIR=/tmp/pion-artifacts \
  -v /tmp/pion-artifacts:/tmp/pion-artifacts \
  cheetah-pion-helper:dev \
  --mode whip --url http://127.0.0.1:8080/index/api/whip?app=live\&stream=test
```

## Run a WHEP player

```bash
docker run --rm --network host \
  -e WEBRTC_INTEROP_ARTIFACT_DIR=/tmp/pion-artifacts \
  -v /tmp/pion-artifacts:/tmp/pion-artifacts \
  cheetah-pion-helper:dev \
  --mode whep --url http://127.0.0.1:8080/index/api/whep?app=live\&stream=test
```

## Output artifacts

When `WEBRTC_INTEROP_ARTIFACT_DIR` is set the helper writes a
`peer-stats.json` summary on clean exit. Schema:

```json
{
  "first_keyframe_ms": 0,
  "nacks_sent": 0,
  "nacks_received": 0,
  "bytes_sent": 0,
  "bytes_received": 0
}
```

The cheetah harness side reads this file from
`target/webrtc-interop/<test-name>/peer-stats.json` (mount the volume
appropriately) and runs the assertion helpers in
`module/tests/interop_harness.rs::assertions`.

## Status

This is a documentation scaffold; the Go source in `main.go` is
intentionally kept compact. The full nightly lab integration depends
on docker-compose (`interop-docker-compose.yml`) wiring the helper to
the ZLM container — that wiring is the next round of Phase 06 work.
