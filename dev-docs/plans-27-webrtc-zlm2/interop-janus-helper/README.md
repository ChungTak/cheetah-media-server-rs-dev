# Janus signaling smoke helper

Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`).

Janus is a popular SFU; cheetah's interop suite includes a
`janus_signaling_smoke` ignored test that posts a minimal create /
attach / offer sequence to a running Janus REST API. This
directory documents how to bring up that Janus instance and what
endpoints / plugins the test expects.

## Prereqs

We use the upstream Janus image:

```bash
docker pull canyan/janus-gateway:1.x
docker run --rm --net=host \
  -e ADMIN_KEY=cheetah \
  canyan/janus-gateway:1.x
```

The HTTP API listens on `127.0.0.1:8088` by default. Set:

```bash
export WEBRTC_INTEROP_JANUS_URL=http://127.0.0.1:8088/janus
```

## Plugins exercised

The smoke test uses `janus.plugin.echotest` because it does not
require a corresponding stream registration step:

1. `POST /janus` → `{ "janus": "create", "transaction": "..." }`
   returns a session id.
2. `POST /janus/<session>` → `{ "janus": "attach",
   "plugin": "janus.plugin.echotest", ... }` returns a handle id.
3. `POST /janus/<session>/<handle>` → `{ "janus": "message",
   "body": { "video": true, "audio": true } }` echoes media back.

The cheetah side answers the offer cheetah generates and verifies
the round-trip. As of the current round the test body is still a
URL-shape check; the full handshake lands in the next round when
the helper image is integrated into compose.

## docker-compose integration

```yaml
services:
  janus:
    image: canyan/janus-gateway:1.x
    network_mode: host
    profiles:
      - janus
    environment:
      ADMIN_KEY: ${JANUS_ADMIN_KEY:-cheetah}
```

Add this stanza to `interop-docker-compose.yml` and start with
`docker compose --profile janus up -d`. The harness picks it up
automatically once `WEBRTC_INTEROP_JANUS_URL` is exported.

## Status

This directory is documentation-grade. The HTTP-shape ignored test
(`janus_signaling_smoke`) already lives in
`crates/protocols/webrtc/module/tests/interop.rs`; expanding the
test body to drive a real `attach` / `message` round-trip is the
next round of Phase 06 work.
