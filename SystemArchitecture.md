# Cheetah Media Server Architecture

## 1. Layered Model

The project follows a six-layer architecture with one-way downward dependencies:

1. Application layer (`apps/*`)
2. Engine orchestration layer (`cheetah-engine`, `cheetah-control`)
3. SDK and contracts layer (`cheetah-sdk`, `cheetah-runtime-api`)
4. Module integration layer (`cheetah-*-module`)
5. Driver/runtime adaptation layer (`cheetah-*-driver-tokio`, `cheetah-runtime-tokio`)
6. Foundation protocol/media layer (`cheetah-*-core`, `cheetah-codec`)

Any cross-layer capability must be exposed via traits or APIs and injected from upper layers.

## 2. Protocol Decomposition Rule

Every protocol must be split into three crates:

- `cheetah-<proto>-core`
- `cheetah-<proto>-driver-tokio`
- `cheetah-<proto>-module`

Responsibilities:

- `core`: Sans-I/O protocol state machine only.
- `driver`: socket, framing, timer, spawning, channel and backpressure handling.
- `module`: engine integration, authz/authn, resource/session orchestration and routing.

The Cargo package names carry the public architecture boundary. On disk, protocol crates are grouped under `crates/protocols/<proto>/` to keep the workspace readable as more protocols are added.

Recommended protocol directory layout:

```text
crates/protocols/<proto>/
  core/                 # cheetah-<proto>-core
  driver-tokio/         # cheetah-<proto>-driver-tokio
  module/               # cheetah-<proto>-module
  bindings/<target>/    # cheetah-<proto>-c-api, cheetah-<proto>-wasm, ...
  testing/<kind>/       # cheetah-<proto>-property-tests, ...
  fuzz/                 # standalone cargo-fuzz workspace
```

Shared crates are grouped by responsibility:

- `crates/foundation/*`
- `crates/runtime/*`
- `crates/sdk/*`
- `crates/system/*`

## 3. RTMP Reference Mapping

Current RTMP crates:

- `crates/protocols/rtmp/core` (`cheetah-rtmp-core`)
- `crates/protocols/rtmp/driver-tokio` (`cheetah-rtmp-driver-tokio`)
- `crates/protocols/rtmp/module` (`cheetah-rtmp-module`)
- `crates/protocols/rtmp/testing/property-tests` (`cheetah-rtmp-property-tests`)
- `crates/protocols/rtmp/fuzz` (`cheetah-rtmp-fuzz`, standalone cargo-fuzz harness crate)
- `crates/protocols/rtmp/bindings/c-api` (`cheetah-rtmp-c-api`, core-only C ABI wrapper)
- `crates/protocols/rtmp/bindings/wasm` (`cheetah-rtmp-wasm`, wasm bridge on top of C ABI)

Current HTTP-FLV crates:

- `crates/protocols/http-flv/core` (`cheetah-http-flv-core`)
- `crates/protocols/http-flv/driver-tokio` (`cheetah-http-flv-driver-tokio`)
- `crates/protocols/http-flv/module` (`cheetah-http-flv-module`)
- `crates/protocols/http-flv/fuzz` (`cheetah-http-flv-fuzz`, standalone cargo-fuzz harness crate)

FLV encapsulation boundary clarification:

- The FLV frame↔payload egress mapping (`map_frame_to_rtmp_flv_payload`, `build_track_bootstrap_payloads`, sequence-header / config / `onMetaData` builders) lives in `cheetah-codec` (`cheetah_codec::flv_egress`) and is shared by both RTMP and HTTP-FLV.
- `onMetaData` script data is serialized by a minimal self-contained AMF0 encoder inside `cheetah-codec`; the full AMF0 codec (encode + decode for RTMP command messages) stays in `cheetah-rtmp-core`.
- `cheetah-rtmp-core::flv` re-exports the codec API to preserve its public surface; `cheetah-http-flv-core` / `cheetah-http-flv-module` consume `cheetah-codec` directly and carry no `cheetah-rtmp-core` production dependency.

Property-based test crates use the explicit `property-tests` suffix. The older `pbt` abbreviation meant "property-based testing", but it is intentionally avoided in crate and directory names because it obscures the crate purpose.

Core input/output model:

- `CoreInput::{Bytes, Timeout, Command}`
- `CoreOutput::{Write, Event, SetTimer, CancelTimer}`

This model keeps runtime and I/O out of protocol-core while preserving explicit timer and event flow.

RTMP boundary clarification:

- `cheetah-rtmp-core` no longer exposes connection façades such as `RtmpServerConnection` / `RtmpMessageChannel` / legacy client-connection wrappers.
- Runtime wiring is implemented only in `cheetah-rtmp-driver-tokio` (`start_server` for inbound and `start_client` for outbound play/publish).
- `cheetah-rtmp-module` owns orchestration logic, including static pull/push background jobs, and drives core via driver command/event channels.

`cheetah-rtmp-core` feature policy:

- crate is always `#![no_std]` with `alloc`
- host and cross-target builds must compile without a `std` feature toggle
- `property-tests` remains test-surface export only and does not alter protocol behavior

RTMP FFI policy:

- C API/wasm scope is **core-only binding**; no driver/module runtime logic is moved into FFI crates.
- FFI wrappers must remain boundary adapters only and must not modify `cheetah-rtmp-core` hot-path behavior.

## 3.1 RTSP Reference Mapping

Current RTSP crates:

- `crates/protocols/rtsp/core` (`cheetah-rtsp-core`)
- `crates/protocols/rtsp/driver-tokio` (`cheetah-rtsp-driver-tokio`)
- `crates/protocols/rtsp/module` (`cheetah-rtsp-module`)
- `crates/protocols/rtsp/testing/property-tests` (`cheetah-rtsp-property-tests`)
- `crates/protocols/rtsp/fuzz` (`cheetah-rtsp-fuzz`, standalone cargo-fuzz harness crate)

RTSP capability snapshot:

- control plane: OPTIONS, DESCRIBE, ANNOUNCE, SETUP, PLAY, PAUSE, RECORD, TEARDOWN, GET_PARAMETER, SET_PARAMETER
- transport matrix: RTP over UDP unicast, RTP over TCP interleaved, RTP over HTTP tunnel (GET/POST + session cookie + base64 POST), RTP multicast PLAY
- server path: publish ingest + play egress
- outbound path: RTSP pull jobs, RTSP push jobs, relay jobs (pull + push)
- compatibility baseline: standard + non-standard SDP/transport handling with bounded robustness, capture-fixture replay, property tests and fuzz targets

RTSP boundary clarification:

- `cheetah-rtsp-core` remains Sans-I/O and runtime-neutral, covering request/response parsing, interleaved framing, RTP/RTCP packet models, and Transport/Session/Range/RTP-Info/SDP/auth parsing.
- `cheetah-rtsp-driver-tokio` owns runtime/socket concerns: TCP/UDP I/O, HTTP tunnel connection pairing, multicast endpoint operations, and outbound client driving.
- `cheetah-rtsp-module` owns engine integration, publish lease orchestration, server session lifecycle, and static pull/push/relay job supervision.

## 3.2 fMP4 Reference Mapping

Current fMP4 crates:

- `crates/protocols/fmp4/core` (`cheetah-fmp4-core`)
- `crates/protocols/fmp4/driver-tokio` (`cheetah-fmp4-driver-tokio`)
- `crates/protocols/fmp4/module` (`cheetah-fmp4-module`)
- `crates/protocols/fmp4/testing/property-tests` (`cheetah-fmp4-property-tests`)
- `crates/protocols/fmp4/fuzz` (`cheetah-fmp4-fuzz`, standalone cargo-fuzz harness crate)

Shared ISO BMFF/fMP4 container capability in `cheetah-codec`:

- `fmp4_mux.rs`: `Fmp4Muxer` — generates init segments (ftyp+moov) and media segments (styp+moof+mdat)
- `fmp4_demux.rs`: `Fmp4Demuxer` — streaming demuxer with box reassembly, arbitrary chunk input

fMP4 capability snapshot:

- play egress: HTTP-fMP4 (chunked), WS-fMP4 (binary frames), HTTPS/WSS-fMP4 (TLS)
- ingest: remote fMP4 pull jobs (http/https/ws/wss sources)
- codec matrix: H264, H265, H266, VP8, VP9, AV1, MJPEG (video); AAC, G711A, G711U, Opus, MP3, MP2 (audio)
- URL routing: `/{app}/{stream}.mp4`, `/{app}/{stream}.live.mp4` (SMS compat)
- multi-track: up to `max_tracks` audio/video tracks in single moov

fMP4 boundary clarification:

- `cheetah-fmp4-core` is Sans-I/O: HTTP request routing (.mp4/.live.mp4), WebSocket upgrade validation, CORS, session state machine.
- `cheetah-fmp4-driver-tokio` owns TCP bind/accept, HTTP/1.1 parsing, chunked response encoding, WebSocket binary framing, per-connection write queues, and pull client (http/https/ws/wss).
- `cheetah-fmp4-module` owns engine subscribe/publish, play session lifecycle, pull job supervision with backoff retry, and config validation.

## 3.3 RTP Reference Mapping

Current RTP crates:

- `crates/protocols/rtp/core` (`cheetah-rtp-core`)
- `crates/protocols/rtp/driver-tokio` (`cheetah-rtp-driver-tokio`)
- `crates/protocols/rtp/module` (`cheetah-rtp-module`)

RTP capability snapshot:

- transport matrix: RTP over UDP, RTP over TCP (RFC 4571 2-byte length prefix and RTSP-style 4-byte interleaved framing with auto-detect), RTCP (SR/RR reports, RR-timeout sender shutdown, dedicated RTCP UDP socket support)
- payload formatting: MPEG-TS, PS (Program Stream), ES (Elementary Stream), Ehome, Hikvision XHB, JT/T 1078 — all collapsed into `AVFrame + TrackInfo` by `cheetah-codec`
- codec matrix: H264, H265, AAC, G711 (PCMA/PCMU), Opus, MP3, VP8, VP9, AV1
- server ingest: UDP/TCP active/passive server modes, SSRC lock, single-port multi-stream support, bounded TCP context recovery (search-by-SSRC + PS-system-header), dynamic `nMaxRtpLength` learner with bounded cap, ZLM-style `RtpProcess` pre-publish bounded frame cache
- client egress: active/passive client sending, multi-target `senderInfos`, configurable G711 packet duration (ABL `kRtpG711DurMs`-style), `disableVideo` / `disableAudio` track filters
- bridge compatibility: bi-directional bridge to and from RTMP, RTSP, HLS, fMP4 inside the cheetah engine

RTP boundary clarification:

- `cheetah-rtp-core` is purely Sans-I/O and runtime-neutral. It defines the core session state (`RtpCore`), mapping SSRC to stream keys, parsing raw RTP/RTCP packets, and integrating demuxers (PS/TS via `cheetah-codec`). Input is passed as `RtpCoreInput` and output is yielded as `RtpCoreOutput` (such as `SendUdp`, `SendTcp`, `SendRtcp`, `Event(RtpCoreEvent)`).
- `cheetah-rtp-driver-tokio` owns all networking and IO: TCP/UDP bind, read loop, frame assembling, write buffering, timer ticks, and driving the core state machine using active tokio execution loops.
- `cheetah-rtp-module` handles engine integration, authentication, stream key mapping (e.g. mapping SSRC or predefined paths like `/live/{ssrc}`), API bindings, session lifecycles, and managing static pull/push jobs.

## 3.4 GB28181 Reference Mapping

Current GB28181 crates:

- `crates/protocols/gb28181/core` (`cheetah-gb28181-core`)
- `crates/protocols/gb28181/driver-tokio` (`cheetah-gb28181-driver-tokio`)
- `crates/protocols/gb28181/module` (`cheetah-gb28181-module`)

GB28181 capability snapshot:

- control plane (SIP): REGISTER challenge auth (with lenient `\r\n`/`\n`/`\r` line terminators and `,`/`;` Digest parameter parsing), duplicate header tolerance, keepalive, INVITE, ACK, BYE, standard/non-standard SDP negotiation
- media sessions: GB28181 passive media stream reception, active device stream pull (via INVITE request)
- audio talkback: bi-directional voice talk (`sendrecv` SDP negotiation)
- compatibility baseline: robust SIP message parsing (vendored after ABLMediaServer's lenient SIP parse + DigestAuthentication), keepalive offline timeouts, customizable auth challenge, and ZLMediaKit / SMS-compatible REST endpoints.

GB28181 boundary clarification:

- `cheetah-gb28181-core` is purely Sans-I/O. It models SIP requests/responses, device registry, keepalive timeouts, invite/bye dialogs, and SDP parsing (`GbSdp`). Interaction is driven via `Gb28181CoreInput` and yields `Gb28181CoreOutput` (like `SendSip`, `Gb28181Event`).
- `cheetah-gb28181-driver-tokio` executes the UDP/TCP SIP message loop, handles TCP connection states, manages timer-based ticks for offline checks, and routes outgoing SIP buffers.
- `cheetah-gb28181-module` manages GB28181 business logic, registering HTTP REST APIs (for triggering manual INVITE, BYE, and talkback), keeping track of active device maps, checking publish leases, and bridging incoming streams to the core media engine.
- `Gb28181ModuleConfig` has `control_owner = local | signaling` (default `local`). `local` keeps the existing media SIP/GB listener; `signaling` disables it and expects the cluster signaling control plane to drive GB sessions. Module `init` fails on dual-owner configurations: `signaling` requires `signaling_control_plane.enabled=true`, and `local` conflicts with an active `canary`/`production` rollout of the signaling control plane.

## 3.5 HLS Reference Mapping

Current HLS crates:

- `crates/protocols/hls/core` (`cheetah-hls-core`)
- `crates/protocols/hls/driver-tokio` (`cheetah-hls-driver-tokio`)
- `crates/protocols/hls/module` (`cheetah-hls-module`)
- `crates/protocols/hls/testing/property-tests` (`cheetah-hls-property-tests`)
- `crates/protocols/hls/fuzz` (`cheetah-hls-fuzz`, standalone cargo-fuzz harness crate)

HLS capability snapshot:

- containers: MPEG-TS segments (`.ts`, default) and fMP4 segments (`.m4s` + `init.mp4`).
- Low-Latency HLS (fMP4 container): partial segments (`EXT-X-PART`), `EXT-X-SERVER-CONTROL` blocking reload, `EXT-X-PART-INF`, `EXT-X-PRELOAD-HINT`, `EXT-X-PROGRAM-DATE-TIME`, rendition reports.
- play egress: master + media playlists (`/{app}/{stream}.m3u8`, `/{app}/{stream}/index.m3u8`), segment/part fetch, embedded hls.js player page, HTTPS/TLS.
- session identity: `HLS_SESSION` cookie or `?session=` query; CDN Bearer-token auth mode via `cdn_secret`.
- codec matrix: H264, H265, VP8, VP9, AV1 (video); AAC, G711A, G711U, MP3, Opus (audio).

HLS boundary clarification:

- `cheetah-hls-core` is Sans-I/O: playlist building (`PlaylistBuilder`), LL-HLS part/segment state (`ll_hls`), TS/fMP4 mux+demux views (delegating container work to `cheetah-codec`), playback pacing (`HlsPlaybackPacer`), request routing, and playlist parsing. No socket/timer/runtime.
- `cheetah-hls-driver-tokio` owns the HTTP(S) server (`start_server`/`start_tls_server`), connection command channel, and optional on-disk segment writer (`HlsFileWriter`).
- `cheetah-hls-module` owns engine subscribe, per-stream muxer lifecycle, segment ring buffer, remote pull, and config validation; timers go through `RuntimeApi` (runtime-neutral, see §5).

## 3.6 HTTP-TS Reference Mapping

Current TS crates:

- `crates/protocols/ts/core` (`cheetah-ts-core`)
- `crates/protocols/ts/driver-tokio` (`cheetah-ts-driver-tokio`)
- `crates/protocols/ts/module` (`cheetah-ts-module`)
- `crates/protocols/ts/testing/property-tests` (`cheetah-ts-property-tests`)

Shared MPEG-TS mux/demux capability lives in `cheetah-codec` (`ts_mux`/`ts_demux`); the TS core consumes those views rather than reimplementing container logic.

TS capability snapshot:

- play egress: HTTP-TS (`/{app}/{stream}.ts`), WS-TS (binary frames), ZLM-compatible `.live.ts`, HTTPS/WSS-TS (TLS).
- 188-byte TS packetization with periodic PAT/PMT injection (`pat_pmt_interval_ms`) so late joiners recover track topology.
- bootstrap GOP catch-up (`bootstrap_max_frames`) for fast first-frame.
- ingest: RTP-encapsulated MPEG-TS demux via `RtpTsIngest` → engine tracks; remote TS pull jobs with backoff retry.

TS boundary clarification:

- `cheetah-ts-core` is Sans-I/O: HTTP request routing, WebSocket upgrade validation, CORS, session state (`TsCore`), and RTP-TS ingest state (`RtpTsIngest`).
- `cheetah-ts-driver-tokio` owns the HTTP(S)/WS server (`start_server`), per-connection command channel, and the pull client (`TsPullClient`).
- `cheetah-ts-module` owns engine subscribe/publish, play session lifecycle, pull job supervision, and config validation.

## 3.7 MP4 VOD Reference Mapping

Current MP4 crates:

- `crates/protocols/mp4/core` (`cheetah-mp4-core`)
- `crates/protocols/mp4/driver-tokio` (`cheetah-mp4-driver-tokio`)
- `crates/protocols/mp4/module` (`cheetah-mp4-module`)
- `crates/protocols/mp4/testing/property-tests` (`cheetah-mp4-property-tests`)
- `crates/protocols/mp4/fuzz` (`cheetah-mp4-fuzz`, standalone cargo-fuzz harness crate)

MP4 file parsing/reading capability lives in `cheetah-codec` (`Mp4Reader`); the VOD core consumes it via explicit `ReadAt` requests rather than performing I/O itself.

MP4 VOD capability snapshot:

- on-demand playback of local MP4 files as a live engine stream (`file/<stream>`), consumable by RTSP/RTMP/HTTP-FLV/etc subscribers.
- session control: seek, pause, scale (playback rate), stop; ABL-style `read_count` (play once / repeat `n` / infinite loop).
- multi-file playlists (ZLM `;`-separated URI lists) concatenated into one timeline; high-speed keyframe-only gating above a configurable scale threshold.
- ZLM compatibility: `loadMP4File` / `seekRecordStamp` / `setRecordSpeed` REST endpoints, `mp4:`/`flv:` URI prefixes, path-traversal-safe resolution under the configured root.

MP4 VOD boundary clarification:

- `cheetah-mp4-core` is Sans-I/O: the `VodSession` state machine (`Start`/`Tick`/`Seek`/`Pause`/`Scale`/`Stop`) that emits `ReadAt`/`EmitFrame`/`ScheduleTick` outputs; no file I/O, sockets, or `Instant::now()`.
- `cheetah-mp4-driver-tokio` owns file I/O, the schedule-tick loop, and command dispatch on internal tokio channels. Its public surface stays runtime-neutral: `VodDriverHandle::take_events()` yields a `VodEventStream` (`impl futures::Stream`), not a `tokio::sync::mpsc::Receiver`.
- `cheetah-mp4-module` carries no `tokio` production dependency. `VodApi` receives an `Arc<dyn RuntimeApi>` from `EngineContext` and drives the VOD→engine `bridge_events` task via `RuntimeApi::spawn`, consuming the driver's neutral event stream. It owns URI resolution, session registry, and the ZLM-compat HTTP surface.

## 3.8 SRT Reference Mapping

Current SRT crates:

- `crates/protocols/srt/core` (`cheetah-srt-core`)
- `crates/protocols/srt/driver-tokio` (`cheetah-srt-driver-tokio`)
- `crates/protocols/srt/module` (`cheetah-srt-module`)
- `crates/protocols/srt/testing/property-tests` (`cheetah-srt-property-tests`)
- `crates/protocols/srt/fuzz` (`cheetah-srt-fuzz`, standalone cargo-fuzz harness crate)

SRT capability snapshot:

- roles: listener (inbound) and caller (outbound); stream modes publish / request / play.
- `streamid` parsing (`#!::` key/value form) resolving stream key, mode, user/host/session, and vendor extras.
- payload: MPEG-TS carried over SRT, demuxed into engine tracks via `cheetah-codec`.
- encryption: AES-128 / AES-256 passphrase (optional); configurable latency window.

SRT boundary clarification:

- `cheetah-srt-core` is Sans-I/O: session options, `streamid`/URL parsing, and the `SrtCore` input/output/event model; no socket or handshake execution.
- `cheetah-srt-driver-tokio` owns the UDP transport and SRT handshake/ARQ via the vendored `shiguredo_srt` engine (`spawn_driver`), surfacing peers through `SrtDriverHandle`/`SrtDriverCommand`/`SrtDriverEvent`.
- `cheetah-srt-module` owns engine publish/subscribe wiring, listener/caller session binding, metrics, HTTP surface, and config validation.

## 3.9 WebRTC Reference Mapping

Current WebRTC crates:

- `crates/protocols/webrtc/core` (`cheetah-webrtc-core`)
- `crates/protocols/webrtc/driver-tokio` (`cheetah-webrtc-driver-tokio`)
- `crates/protocols/webrtc/module` (`cheetah-webrtc-module`)

WebRTC capability snapshot:

- signaling: WHIP/WHEP (HTTP), OME-compatible WebSocket signaling, and P2P mesh signaling (client + inbound server).
- session lifecycle: offer/answer negotiation, ICE candidate exchange (host/srflx/relay, TCP fallback), data channel bridging, keyframe requests, per-session supervision.

WebRTC boundary clarification:

- `cheetah-webrtc-core` is purely Sans-I/O SDP/ICE/session state.
- `cheetah-webrtc-driver-tokio` owns all runtime and I/O concerns and exposes them behind runtime-neutral abstractions:
  - WebSocket: `WsFrame`/`WsError`, `trait WsConnection` (`send_text`/`recv`/`close`), `trait WsConnector` (`connect(url, timeout)`), plus the inbound server `bind_ws_server(addr) -> (WsServerListener, SocketAddr)` and `WsServerListener::serve(cfg, handler, cancel)`. The accept loop, handshake timeout, ping/pong, capacity backpressure, and connection counting all live here; `tokio-tungstenite` never leaks into the module.
  - HTTP/TLS client: `WhipWhepHttpClient` (raw HTTP/1.1 over `tokio-rustls`) for WHIP/WHEP egress.
  - The tokio driver task, UDP/ICE transport, and `WebRtcDriverHandle`/`WebRtcDriverCommand` command channel.
- `cheetah-webrtc-module` holds engine wiring plus WebRTC business/signaling logic: WHIP/WHEP + OME + P2P route handling, SDP munging/compat, SSRF/URL policy, `OmeWsMessage`/`P2pMessage` encode/decode, publish leases, and session bookkeeping. It consumes the driver's neutral `WsConnection`/`WsConnector`/`WsServerListener` handles and injects `RuntimeApi` for timers/tasks. The module's production code carries **no direct `tokio` dependency** (tokio is a dev-dependency for tests only); `dev-scripts/check_runtime_boundaries.sh` enforces this at the manifest level.

## 3.10 Optional Media Processing with avcodec-rs

Audio/video transcoding, image processing, snapshots, ABR generation, mixing, mosaic, and
caption extraction are optional Job/Work capabilities. Their implementation contract is defined
by [`dev-docs/904_avcodec_processing_plan`](dev-docs/904_avcodec_processing_plan/README.md).

The fixed dependency boundary is:

- `cheetah-media-processing-module` is the only Cheetah module that owns codec/image sessions.
- Cheetah directly depends only on the top-level `avcodec` crate, pinned by version and immutable
  git revision with default features disabled.
- Cheetah does not directly depend on avcodec backend/core/FFI crates, FFmpeg/image libraries, or
  an FFmpeg executable.
- avcodec types stay private to the processing module. Public SDK/domain types remain
  runtime-neutral and backend-neutral.
- `cheetah-codec` continues to own canonical compressed `AVFrame + TrackInfo` semantics,
  timestamp normalization, Access Units, parameter sets, and pure compatibility parsers; it does
  not own codec sessions.

Processing lifecycle:

- `MediaProcessingApi` provides preflight plus create/get/list/update/stop/delete Job operations.
- `ImageProcessApi` provides bounded still-image operations and JPEG output.
- Each codec graph is owned by one blocking worker for its entire lifetime.
- Stream processing publishes to a dedicated derived `StreamKey`; it never overwrites the source
  or bypasses the single-publisher lease.
- Auto-created derived Jobs are only allowed by explicit protocol policy and are shared by a
  normalized processing-spec fingerprint.
- Every queue, cache, input count, pixel rate, worker count, retry, and grace period is bounded.

Cargo features for audio, video, image, NativeFree, and Software profiles are independent and
disabled by default. Capabilities are advertised only when the feature is compiled, startup
preflight succeeds, and a production provider is registered. PNG encoding, unavailable profiles,
and unsupported codecs return explicit `Unsupported`; they never produce placeholder output.

The legacy `FfmpegApi`, local process executor, FFmpeg proxy routes/configuration, and direct
`image` implementation are removed during the 904 migration. avcodec's Software profile may use
an internal upstream backend, but that detail must not restore a Cheetah FFmpeg API or leak into
public contracts.

Preflight and capability honesty:

- `MediaProcessingApi::preflight` probes each compiled profile for the requested codec
  decode/encode pairs, image operators, audio resample/channel adaptation, flush/reset support,
  memory domain, and buffer path.
- `ProcessingPreflightReport` records the active profile, a top-level `available` boolean,
  the list of ready operation names, and a `diagnostics` map from unavailable operation name to
  reason.
- A single failed operation removes only that operation; the provider still advertises the remaining
  ones. If the core provider cannot initialize, module startup fails and health reports `Disabled`
  or `Degraded` rather than treating a missing feature as an error.

Observability:

- `MetricsApi` exposes `inc` and `set` (gauge). `MediaProcessingProvider` publishes
  `media_processing_jobs`, `media_processing_frames_total`, `media_processing_drops_total`,
  `media_processing_bytes_total`, `media_processing_pending_total`,
  `media_processing_queue_depth`, `media_processing_latency_ms`, `media_processing_shared_refs`,
  `media_processing_restarts_total`, `media_processing_resource_reserved`, and
  `media_processing_preflight` with low-cardinality labels. Stale metric keys are zeroed on each
  publish so gauges do not outlive the jobs they describe.
- `ResourceLeakReport` includes `active_processing_job_ids`, live blocking workers, derived
  publishers/subscribers, shared-task references, and reserved processing resources, collected by
  paginating through `MediaProcessingApi::list_jobs` for non-terminal jobs and querying engine
  task/stream state.

Structured lifecycle logging:

- Each job emits one structured log at creation, startup, first output, drain, and terminal state.
- Log fields include job id/kind/owner/generation, avcodec revision/profile, sanitized source/target
  keys, codec, dimensions, memory domain, backend selection summary, latency, packet/frame/byte
  counters, pending/drops/flushes/resets, shared refcount, terminal state, and a stable error code.
- Raw payloads, credentials, URL parameters, and font contents are never logged. Per-frame info or
  warn logs are avoided on the normal path; high-frequency errors are aggregated and emitted by
  threshold.

Hot config reload:

- Pure upper-bound increases (max concurrent jobs, image dimensions, pixel rate, frame bytes,
  overlay counts, etc.) are applied immediately via `ModuleConfigChange`.
- Profile, default backend, or any bound decrease that would drop below current usage returns
  `ModuleRestartRequired`; the engine stops and recreates the module, releasing old workers and
  re-registering the provider. The module does not implement a private restart bypass.

Fault injection and diagnostics:

- `tests/fault_injection.rs` exercises backend selection failure, unsupported output codecs,
  corrupt input packets, worker spawn failure, worker panic, output queue backpressure, deadline
  rejection, cancellation, module restart, and engine shutdown.
- Every scenario terminates in a stable `Stopped`/`Failed` state with a non-panicking process and an
  empty `ResourceLeakReport`; no derived stream is left behind.
- Runtime-level faults are injected by a `FaultRuntime` test double that overrides
  `RuntimeApi::spawn_blocking` without modifying production code.

## 3.11 Admission API

Synchronous admission decisions gate side-effecting media operations before they allocate resources.

- `cheetah-media-api` exposes `MediaAdmissionApi::authorize(ctx, AdmissionRequest) -> Decision`.
- `AdmissionAction` covers `Publish`, `Play`, `CreatePullProxy`, `CreatePushProxy`,
  `CreateProcessingJob`, `OpenRtpReceiver`, and `OpenRtpSender`.
- `Decision` is `Allow` or `Deny { code: MediaErrorCode, reason: String }`.
- `MediaServices` has a dedicated `Admission` slot with `register_admission` / `admission` / `unregister`.
- `EngineMediaFacade` invokes admission before `acquire_publisher`, `open_subscriber`,
  `create_pull_proxy`, `create_push_proxy`, `create_processing_job`, `open_rtp_receiver`, and
  `open_rtp_sender`. A `Deny` returns the stable `MediaErrorCode` before the provider allocates any
  lease, port, worker, derived stream, or session.
- The existing `WebhookApi` decision path is kept for ZLM-compatible webhook hooks. `WebhookDecisionClient` also implements `MediaAdmissionApi` and maps `Publish`/`Play` to the existing `on_publish`/`on_play` webhook decision flow; other actions default to `Allow` until native translators land.

## 3.12 Webhook Administration

Webhook profiles are managed through `WebhookAdminApi` and exposed as native HTTP routes.

- `cheetah-media-api` defines `WebhookAdminApi` with `create_profile`, `get_profile`, `list_profiles`, `update_profile`, `delete_profile` and `test_profile`.
- Profiles carry `id`, `enabled`, `mode` (`NativeDomain`/`ZlmCompatible`), `target_url`, `event_filter`, `admission_actions`, `failure_policy`, `timeout_ms`, `secret` and `generation`.
- Updates require `expected_generation`; the provider preserves the existing `secret` when an update request leaves it empty.
- `WebhookAdminStore` persists profiles through `DatabaseApi` under the `webhook:profile:` key prefix; module restart reloads from the same store.
- `test_profile` sends a synthetic `WebhookTest` envelope, validates DNS/connect/HTTP/signature and returns a `WebhookTestReport` without allocating media resources.
- Native routes under `/api/v1/webhook/profiles` map to `WebhookAdminApi` and require `MediaScope::ServerAdmin`.
- `cheetah-webhook-dispatcher` registers the admin provider as `MediaCapability::WebhookAdmin` in `MediaServices` alongside the `Webhook` and `Admission` providers.

### Outbound dispatch

`WebhookDispatcher` consumes `MediaEvent`s from the event bus, translates them per profile `mode` and POSTs the resulting envelope.

- `WebhookProfileMode::NativeDomain` emits a signed envelope with `event_id`, `event_type`, `occurred_at`, `resource`, `payload` and `attempt`, signed with `HMAC-SHA256` in `X-Webhook-Signature` when a secret is configured.
- `WebhookProfileMode::ZlmCompatible` emits ZLM-style hooks (`on_publish`, `on_play`, `on_stream_changed`, etc.) for the subset of events that have a defined mapping.
- Each profile gets its own bounded worker queue; slow profiles do not block others.
- Retries are limited by `max_retries` and `max_retry_duration_ms`, use exponential backoff from `retry_interval_ms`, and only happen for network errors, HTTP 429, or 5xx responses. 4xx responses (other than 429) are not retried.
- Unsupported mappings for the active mode increment `unsupported_mapping_total{event_type,profile}` through `MetricsApi`.
- Closing the dispatcher handle or removing a profile from configuration stops further delivery attempts for that profile.

## 3.13 Signaling Control Plane

The `signaling-control-plane` Cargo feature gates the new gRPC-based signaling control plane.

- `apps/cheetah-server` adds the optional feature `signaling-control-plane` and includes it in `media-control-full`.
- When enabled, `main.rs` registers a `signaling_control_plane` module schema in `ConfigStore`.
- `RolloutMode` lives in `cheetah-media-api` and is re-exported/reused by `cheetah-media-control-plane` and `cheetah-server`.
- `RolloutGate` (in `cheetah-media-api`) exposes `query_allowed`, `event_allowed`, `mutation_allowed`, and `operation_allowed(tenant, operation)`; `Canary` mode supports tenant and operation allowlists.
- `SignalingControlPlaneConfig` declares the full configuration surface:
  - `enabled`, `rollout` (`register_only`/`shadow_query`/`canary`/`production`)
  - `grpc.listen`, `grpc.advertised_endpoint`, `grpc.message_limits`
  - `registry.endpoint`, `registry.zone`, `registry.node_identity`, `registry.addresses`
  - `contract.min_version`, `contract.max_version`, `contract.checksum`
  - `store.path`, `store.max_size_mb`, `store.retention_hours`
  - `events.max_events`, `events.retention_hours`, `events.cursor_key_handle`
  - `capacity.max_nodes`, `capacity.max_resources`
  - `tls.server_cert_pem`, `tls.server_key_pem`, `tls.client_ca_pem`, `tls.client_cert_required`
  - `secret_exchange.enabled`, `secret_exchange.endpoint`, `secret_exchange.renewal_margin_sec`
- `validate()` enforces required fields when enabled, socket-address parsing, message limits (non-zero `max_inbound_size` and `max_outbound_size`), contract version ordering, TLS consistency (including `client_ca_pem` when client certificates are required), and secret-exchange consistency.
- `cheetah-media-grpc-adapter` `GrpcAdapterConfig` carries an optional `GrpcTlsConfig` and `GrpcMessageLimits` (default 4 MiB inbound/outbound); `GrpcMessageLimits` are validated and will be applied to typed services as they are added.
- `cheetah-media-api` exposes an `AdminApi` with `AdminScope` (Node, Reconcile, Tls, Store, Orphan), `AdminIdentity`, and typed requests/responses for drain, reconciliation, safe diagnostics, TLS/cursor rotation, store checkpoint, and orphan cleanup. Implementations must reject secret dump, raw SQLite queries, arbitrary file reads, and tenant/fencing bypass.
- The current `Assembly` is a placeholder; later MIG tasks will wire the gRPC adapter and control-plane facade.

## 4. Media Model and Unification

All protocol ingest into engine should converge to:

- `AVFrame`
- `TrackInfo`

Shared media canonicalization belongs to `cheetah-codec`:

- timestamp normalization
- timebase conversion
- access-unit assembly
- parameter-set cache/replay

Unified timing semantics:

- `AVFrame` carries `pts/dts/duration` in both native `timebase` ticks and `*_us` microseconds.
- `FrameFlags::KEY` represents random-access points; `FrameFlags::DISCONTINUITY` marks reconnect/seek/source-switch boundary.
- Video frames with `pts < dts` must explicitly carry `FrameFlags::B_FRAME`; otherwise timing validation fails.
- `TrackInfo::clock_rate` maps to canonical media timebase (`1/clock_rate`) via checked conversion.
- `AVFrame.pts/dts` always represent **canonical timeline** values and must not be treated as raw protocol timestamps.

Three timeline levels:

- `source timeline`: protocol-native timing context (for example RTP timestamp, RTMP tag timestamp, CTS, wrap/epoch, optional RTCP mapping).
- `canonical timeline`: engine-internal normalized media timeline used by ring buffer, bootstrap, pacing, recording and cross-protocol conversion.
- `egress timeline`: protocol-specific export view derived from canonical timeline (for example RTMP timestamp/CTS or RTSP RTP timestamp).

Boundary rules:

- Source timing may be preserved as metadata for compatibility/observability, but must not override canonical ordering semantics.
- Egress timestamp repair is an output-encapsulation concern and must not mutate canonical media timeline.

Observability and diagnostics baseline:

- Runtime reports must expose `startup_latency_ms`, `first_second_avg_frame_interval_ms`, `average_playback_rate_x`, and `first_keyframe_delay_ms`.
- Timestamp-repair alerts must be classified by layer:
  - `source_repair_events`: source timeline reorder/repair observations (including B-frame reorder noise).
  - `canonical_repair_events`: canonical timeline monotonic repair events.
  - `egress_repair_events`: protocol-export monotonic repair events.
- High-frequency warning policy must be layer-aware:
  - normal B-frame reorder should stay in `source_repair_events` and must not escalate canonical/egress warnings.
  - `canonical_repair_events` and `egress_repair_events` must be compared against an explicit threshold (for example `REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD`) before raising anomaly.
- Every repair-class log must include both source and canonical context (at least source timestamp + canonical `pts`/`dts`) to make layer attribution deterministic.

Protocol modules should not duplicate these behaviors.

Implementation status: the Sans-I/O baseline lives in `cheetah-codec::observability`:

- `RepairLayer` + `classify_timestamp_alert` map each `TimestampAlert` to the source/canonical layer (pure discontinuity/reset markers are not repairs); `RepairEventCounters` accumulate per-layer totals and expose `is_high_frequency_anomaly`, where the source layer never escalates and canonical/egress escalate at `REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD`.
- `RuntimeReportBuilder` computes the runtime-report schema (`RuntimeObservabilityReport`) from injected wall-clock (`now_us`) and canonical `pts_us` samples — it reads no clock and performs no I/O.
- `cheetah-engine::MetricsRegistry` publishes these through `MetricsApi`: `record_repair_events` adds the layer counters and `record_runtime_report` sets the timing gauges (`startup_latency_ms`, `first_second_avg_frame_interval_ms`, `average_playback_rate_x`, `first_keyframe_delay_ms`). `MetricsApi::inc` lets modules record ad-hoc counters such as `unsupported_mapping_total{event_type,profile}`.
- Per-frame wiring of drivers/modules into `RuntimeReportBuilder` on the live egress path is staged (the metric feed points are defined; hot-path integration lands incrementally per protocol).
- `cheetah-engine::ResourceLeakObserver` (exposed as `Engine::resource_leak_report`) gathers
  non-terminal tasks, active streams, processing Jobs/workers/derived publishers/shared
  references, and non-terminal media sessions into a `ResourceLeakReport`. This is used by tests
  and shutdown checks to confirm that cancellation, engine stop, and module restart do not leave
  orphan runtime objects.

## 5. Runtime Abstraction Rule

Public interfaces in `cheetah-runtime-api`, `cheetah-sdk`, `cheetah-engine`, and `*-module` must remain runtime-neutral.

- Do not expose `tokio::*` or `tokio_util::*` in those public APIs.
- Tokio-specific types stay in `cheetah-runtime-tokio`, `*-driver-tokio`, and application crates.
- `cheetah-engine` may also use Tokio primitives internally as an orchestration layer implementation detail, but must not expose them in its public API.
- CPU-bound codec/image work uses the runtime-neutral `RuntimeApi::spawn_blocking`; its Tokio
  implementation may map to `tokio::task::spawn_blocking`. Codec sessions must never run on an
  async protocol worker.
- CI guard: `dev-scripts/check_runtime_boundaries.sh`

## 6. Testing Strategy

By layer:

- `core`: unit test + property-based test + fuzz (without real network I/O)
- `driver`: runtime and I/O integration tests
- `module`: interoperability and end-to-end flow tests

RTMP property tests are placed in `crates/protocols/rtmp/testing/property-tests` and run against `cheetah-rtmp-core` Sans-I/O APIs.
RTSP property tests are placed in `crates/protocols/rtsp/testing/property-tests` and run against `cheetah-rtsp-core` Sans-I/O APIs.
HTTP-FLV compatibility/robustness tests are staged in `crates/protocols/http-flv/module/tests` (aligned with `.flvstream` fixture + transport fault views); property tests live in `crates/protocols/http-flv/testing/property-tests` and run against `cheetah-codec` FLV egress mapping plus `cheetah-http-flv-core` request parsing.
TS property tests are placed in `crates/protocols/ts/testing/property-tests` and run against `cheetah-codec` MPEG-TS mux/demux plus `cheetah-ts-core` request parsing.

Fuzz harnesses are placed in `crates/protocols/<proto>/fuzz` and are standalone cargo-fuzz workspaces, not default root workspace members.

CI baseline for RTMP core:

- `cargo clippy -p cheetah-rtmp-core`
- `cargo test -p cheetah-rtmp-core`
- `cargo test -p cheetah-rtmp-property-tests`
- `cargo test -p cheetah-rtmp-c-api`
- `cargo test -p cheetah-rtmp-wasm`
- `(cd crates/protocols/rtmp/fuzz && cargo +nightly fuzz build)`
- `dev-scripts/check_rtmp_core_no_std.sh`

CI/check baseline for HTTP-FLV:

- `cargo clippy -p cheetah-http-flv-core`
- `cargo test -p cheetah-http-flv-core`
- `cargo clippy -p cheetah-http-flv-driver-tokio`
- `cargo test -p cheetah-http-flv-driver-tokio`
- `cargo clippy -p cheetah-http-flv-module --tests`
- `cargo test -p cheetah-http-flv-module`
- `cargo test -p cheetah-http-flv-property-tests`
- `(cd crates/protocols/http-flv/fuzz && cargo +nightly fuzz build)`

CI/check baseline for TS:

- `cargo clippy -p cheetah-ts-core`
- `cargo test -p cheetah-ts-core`
- `cargo clippy -p cheetah-ts-driver-tokio`
- `cargo test -p cheetah-ts-driver-tokio`
- `cargo clippy -p cheetah-ts-module --tests`
- `cargo test -p cheetah-ts-module`
- `cargo test -p cheetah-ts-property-tests`

fMP4 property tests are placed in `crates/protocols/fmp4/testing/property-tests` and run against `cheetah-codec` fMP4 mux/demux APIs.

CI/check baseline for fMP4:

- `cargo clippy -p cheetah-codec`
- `cargo test -p cheetah-codec -- fmp4`
- `cargo clippy -p cheetah-fmp4-core`
- `cargo test -p cheetah-fmp4-core`
- `cargo clippy -p cheetah-fmp4-driver-tokio`
- `cargo test -p cheetah-fmp4-driver-tokio`
- `cargo clippy -p cheetah-fmp4-module --tests`
- `cargo test -p cheetah-fmp4-module`
- `cargo test -p cheetah-fmp4-property-tests`
- `(cd crates/protocols/fmp4/fuzz && cargo +nightly fuzz build)`

CI/check baseline for RTP:

- `cargo clippy -p cheetah-rtp-core`
- `cargo test -p cheetah-rtp-core`
- `cargo clippy -p cheetah-rtp-driver-tokio`
- `cargo test -p cheetah-rtp-driver-tokio`
- `cargo clippy -p cheetah-rtp-module --tests`
- `cargo test -p cheetah-rtp-module`

CI/check baseline for GB28181:

- `cargo clippy -p cheetah-gb28181-core`
- `cargo test -p cheetah-gb28181-core`
- `cargo clippy -p cheetah-gb28181-driver-tokio`
- `cargo test -p cheetah-gb28181-driver-tokio`
- `cargo clippy -p cheetah-gb28181-module --tests`
- `cargo test -p cheetah-gb28181-module`
