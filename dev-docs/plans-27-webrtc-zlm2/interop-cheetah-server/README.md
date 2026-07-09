# cheetah-server interop image

Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`).

This directory contains the Dockerfile and config used to run
`cheetah-server` inside the docker-compose interop lab. With this
image the lab is closed-loop: cheetah, ZLMediaKit, Pion, GStreamer,
Janus and Playwright all share `network_mode: host` and the
ignored harness tests can drive end-to-end SDP / media exchanges
against a real cheetah instance.

## Build

The Dockerfile expects to be built from the workspace root so cargo
path dependencies resolve:

```bash
docker build \
  -f dev-docs/plans-27-webrtc-zlm2/interop-cheetah-server/Dockerfile \
  -t cheetah-server:interop \
  .
```

The build uses `--mount=type=cache,target=...` so subsequent builds
are incremental; the first build downloads ~500 MB of crate index
plus all transitive deps.

## Run

```bash
docker run --rm --network host \
  -v "$PWD/dev-docs/plans-27-webrtc-zlm2/interop-cheetah-server/interop.yaml":/etc/cheetah/config.yaml:ro \
  cheetah-server:interop
```

Health check (control plane on 8891):

```bash
curl -fsS http://127.0.0.1:8891/healthz   # placeholder; the real route depends on cheetah-control
```

## docker-compose integration

The compose file (`interop-docker-compose.yml`) brings the cheetah
service up under the `cheetah` profile so operators can opt-in:

```bash
docker compose -f dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml \
  --profile cheetah up -d
```

The cheetah service shares `network_mode: host` with ZLM, so
cheetah's WebRTC UDP (8000) and TCP (8088) ports won't collide
with ZLM's ports (default 8000 might overlap — change `listen_udp`
in `interop.yaml` to e.g. `0.0.0.0:8100` if running side-by-side).

## interop.yaml differences vs `config.example.yaml`

The lab config enables WebRTC + RTMP by default (fewer toggles for
operators) and shortens the handshake timeout to 5 s so failures
surface quickly. All other knobs match the example so the lab
exercises production-like defaults.
