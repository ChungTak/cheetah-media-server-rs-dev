# WebRTC + str0m 总体架构设计

- **状态**: 已完成（Phase 01-05 全部落地，架构设计已完整体现在代码中）
- **范围**: 定义使用 `str0m 0.19.0` 实现 WebRTC 的 crate 边界、依赖方向、核心状态机、driver I/O、module 业务编排、codec contract、配置和 API
- **完成标准**: 实现者可按本文拆出 `cheetah-webrtc-core`、`cheetah-webrtc-driver-tokio`、`cheetah-webrtc-module`，并把 WebRTC 相关能力放到正确层级

## 架构目标

WebRTC 能力必须服务于本项目的统一媒体模型：

```text
WebRTC ingress
  -> str0m Rtc
  -> cheetah-codec WebRTC ingress contract
  -> AVFrame + TrackInfo
  -> Engine StreamManager
  -> RTMP / RTSP / RTP / HLS / fMP4 / HTTP-FLV ...
```

```text
RTMP / RTSP / RTP / GB28181 / file / HLS / fMP4 ingress
  -> Engine StreamManager
  -> cheetah-codec WebRTC egress contract
  -> str0m writer or RTP mode
  -> WebRTC peer
```

关键目标：

- `str0m` 是 WebRTC 协议状态机，不是 module 业务框架。
- WebRTC 不成为私有媒体系统，必须走 engine 和 `AVFrame + TrackInfo`。
- 单端口、多线程、连接迁移是 driver 能力，不进入 core。
- WHIP/WHEP、SMS API、client jobs、P2P、echo test 是 module 能力。
- RTP/RTCP/时间戳/参数集/codec compat 尽量收敛到 `cheetah-codec`。

## Crate 与依赖方向

新增目录：

```text
crates/protocols/webrtc/
  core/                    # cheetah-webrtc-core
  driver-tokio/            # cheetah-webrtc-driver-tokio
  module/                  # cheetah-webrtc-module
  testing/property-tests/  # cheetah-webrtc-property-tests
  fuzz/                    # standalone cargo-fuzz workspace
```

依赖方向：

```text
cheetah-webrtc-module
  -> cheetah-webrtc-driver-tokio
  -> cheetah-webrtc-core
  -> cheetah-sdk
  -> cheetah-codec

cheetah-webrtc-driver-tokio
  -> cheetah-webrtc-core
  -> cheetah-runtime-api
  -> tokio

cheetah-webrtc-core
  -> str0m
  -> cheetah-codec
```

约束：

- `cheetah-webrtc-core` 不依赖 `tokio`、`axum`、`EngineContext`、socket、数据库或系统时间 API。
- `cheetah-webrtc-driver-tokio` 可以依赖 `tokio`，但不得把 `tokio::*` 类型暴露给 module。
- `cheetah-webrtc-module` 只通过 `EngineContext`、`RuntimeApi`、`PublisherApi`、`SubscriberApi` 与系统交互。
- `cheetah-codec` 不依赖 `str0m`，只定义 WebRTC 需要的媒体 contract 和 payload view。

## `cheetah-webrtc-core`

核心职责：

- 创建和持有 `str0m::Rtc`。
- 输入 remote SDP、local candidate、network packet、timer tick、user command。
- 输出 local SDP、network packet、next timeout、media/datachannel event、stats/diagnostic。
- 将 `str0m` 类型转换成项目内部稳定类型，避免上层直接依赖所有 `str0m` 细节。

建议核心类型：

```rust
pub struct WebRtcCore {
    sessions: HashMap<WebRtcSessionId, WebRtcCoreSession>,
    limits: WebRtcCoreLimits,
}

pub struct WebRtcCoreSession {
    id: WebRtcSessionId,
    role: WebRtcSessionRole,
    rtc: str0m::Rtc,
    stream_key: StreamKeyParts,
    state: WebRtcSessionState,
    created_at: WebRtcInstant,
    last_activity_at: WebRtcInstant,
}

pub enum WebRtcCoreInput {
    Command(WebRtcCoreCommand),
    Network(WebRtcNetworkInput),
    Timeout { session_id: WebRtcSessionId, now: WebRtcInstant },
    Tick { now: WebRtcInstant },
}

pub enum WebRtcCoreOutput {
    SendPacket(WebRtcPacketOut),
    SetTimer(WebRtcTimer),
    CancelTimer { session_id: WebRtcSessionId },
    Event(WebRtcCoreEvent),
    Diagnostic(WebRtcDiagnostic),
    CloseSession { session_id: WebRtcSessionId, reason: WebRtcCloseReason },
}
```

core commands：

- `CreateOfferSession`
- `AcceptOffer`
- `ApplyAnswer`
- `AddRemoteCandidate`
- `WriteFrame`
- `WriteRtpPacket`
- `SendDataChannel`
- `RequestKeyframe`
- `SetTargetBitrate`
- `Close`

core events：

- `SessionCreated`
- `LocalDescription`
- `IceConnected`
- `IceDisconnected`
- `MediaTrackAdded`
- `MediaFrame`
- `RtpPacket`
- `RtcpFeedback`
- `DataChannelOpened`
- `DataChannelMessage`
- `Stats`
- `SessionClosed`

时间规则：

- core 不调用 `Instant::now()`。
- driver 将 `RuntimeApi::now()` 或 tokio instant 映射成 core 可用时间。
- 所有 `str0m::Rtc::handle_input`、`poll_output`、timeout 驱动都由外部输入推进。

## `cheetah-webrtc-driver-tokio`

driver 职责：

- UDP single-port listener。
- TCP listener / TCP candidate 支持。
- packet classification：STUN、DTLS、RTP、RTCP、TURN-like relay candidate response。
- session routing：按 ICE ufrag、remote socket address、DTLS association、RTP SSRC/RID 做渐进式绑定。
- timer driving：根据 core 输出设置 per-session timer。
- sharding：多线程 worker 按 session id hash 分片，避免单全局锁。
- backpressure：每个 session 有 bounded write queue。
- connection migration：同一 ICE username/session 的 remote address 变化时更新 route，并发出 diagnostic。

driver handle：

```rust
pub enum WebRtcDriverCommand {
    CreateSession(WebRtcSessionSpec),
    ApplyRemoteDescription { session_id: WebRtcSessionId, sdp: String },
    AddRemoteCandidate { session_id: WebRtcSessionId, candidate: String },
    SendFrame(Box<WebRtcSendFrame>),
    SendDataChannel(WebRtcDataMessage),
    RequestKeyframe { session_id: WebRtcSessionId, track_id: TrackId },
    StopSession(WebRtcSessionId),
}

pub enum WebRtcDriverEvent {
    Core(WebRtcCoreEvent),
    RouteUpdated(WebRtcRouteUpdate),
    Diagnostic(WebRtcDriverDiagnostic),
}
```

单端口路由状态：

```text
initial STUN packet
  -> parse username local_ufrag:remote_ufrag
  -> match pending session
  -> bind remote SocketAddr to session

DTLS packet
  -> route by SocketAddr
  -> fallback by pending DTLS/session route

RTP/RTCP packet
  -> route by SocketAddr
  -> fallback by SSRC map after media starts
```

连接迁移策略：

- 如果 STUN binding request 使用已知 ICE username 但 remote address 改变，driver 更新 session route。
- 更新前必须确保 session id 匹配，避免劫持。
- 更新后旧地址保留短 TTL，防止 NAT rebinding 抖动。
- migration 事件上报 module metrics。

## `cheetah-webrtc-module`

module 职责：

- module factory、manifest、配置 schema。
- HTTP API：SMS play/publish、WHIP/WHEP、session stop、client jobs、P2P、DataChannel echo。
- 鉴权和 stream key 映射。
- WebRTC publish：从 driver event 接收 frame/RTP，归一化后写 engine publisher。
- WebRTC play：订阅 engine stream，按 bootstrap policy 送给 driver/core。
- Simulcast 策略：选择层、切层、观测多层。
- TWCC/BWE 策略：读取估计值，执行降层、限速、丢帧或触发上游 request_keyframe。
- 生命周期：module stop 时关闭 driver、HTTP sessions、client jobs 和 publisher/subscriber。

module manifest：

```rust
ModuleManifest {
    module_id: ModuleId::new("webrtc"),
    display_name: "WebRTC Module".to_string(),
    dependencies: Vec::new(),
    config_namespace: "webrtc".to_string(),
    routes_prefix: "/api/v1/rtc".to_string(),
    capabilities: vec![
        ModuleCapability::Publish,
        ModuleCapability::Subscribe,
        ModuleCapability::HttpApi,
        ModuleCapability::BackgroundJob,
    ],
}
```

HTTP routes：

```text
POST   /api/v1/rtc/play
POST   /api/v1/rtc/publish
POST   /api/v1/rtc/whep
POST   /api/v1/rtc/whip
DELETE /api/v1/rtc/session/{session_id}
PATCH  /api/v1/rtc/session/{session_id}

POST   /api/v1/rtc/pull/start
POST   /api/v1/rtc/pull/stop
GET    /api/v1/rtc/pull/list
POST   /api/v1/rtc/push/start
POST   /api/v1/rtc/push/stop
GET    /api/v1/rtc/push/list
POST   /api/v1/rtc/p2p/add
POST   /api/v1/rtc/p2p/remove
GET    /api/v1/rtc/p2p/list
POST   /api/v1/rtc/p2p/stop
POST   /api/v1/rtc/echo/start
POST   /api/v1/rtc/echo/stop
```

Phase 03 先实现 server play/publish、WHIP/WHEP、session delete；Phase 05 再实现 client/P2P/DataChannel 全量接口。

## `cheetah-codec` WebRTC contract

需要明确新增或增强：

```rust
pub enum FutureProtocolKind {
    WebRtcRtpRtcp,
    // existing variants
}

pub struct WebRtcIngressPacketView {
    pub track_id: TrackId,
    pub codec: CodecId,
    pub rtp_timestamp_ticks: u32,
    pub sequence_number: u16,
    pub marker: bool,
    pub rid: Option<String>,
    pub repaired_rid: Option<String>,
    pub twcc_sequence: Option<u16>,
}

pub struct WebRtcEgressContractView {
    pub track_id: TrackId,
    pub codec: CodecId,
    pub rtp_timestamp_ticks: u32,
    pub random_access: bool,
    pub discontinuity: bool,
    pub fragment_boundary: FragmentBoundary,
    pub parameter_set_replay: ParameterSetReplay,
}
```

规则：

- WebRTC ingress 不能绕过 timestamp normalization。
- WebRTC egress 视频必须有 access unit boundary，否则拒绝发送。
- H264/H265/H266 keyframe 前必须能补参数集，缺失时返回明确错误。
- 音频 clock rate 使用 `TrackInfo::clock_rate`，Opus 默认 48 kHz。
- RTP timestamp 由 canonical timeline 转换，不直接复用其他协议原始 timestamp。

## Codec profile

配置中定义 profile：

```yaml
modules:
  webrtc:
    codec_profile: browser
```

建议枚举：

- `browser`: H264、VP8、VP9、AV1、Opus；H265/G711 只在 SDP 明确支持时允许；AAC/MP3 默认拒绝。
- `device`: H264、H265、G711、Opus、VP8、VP9、AV1；允许更宽松 SDP。
- `passthrough`: 面向非浏览器 WebRTC/RTP peer，允许 RTP mode，但必须显式启用。

不做隐式转码：

- RTMP AAC -> WebRTC browser，如果 peer 不支持 AAC，返回 `UnsupportedCodec`。
- RTSP H265 -> Chrome WHEP，如果 SDP 不接受 H265，返回 `NotAcceptable` 或只发送可接受音频。
- WebRTC VP8 -> RTMP，如果 RTMP 输出不支持 VP8，则其他协议播放请求返回明确错误。

## 配置草案

```yaml
modules:
  webrtc:
    enabled: true
    listen_udp: 0.0.0.0:8000
    listen_tcp: 0.0.0.0:8000
    public_ips: []
    candidate_hostname: ""
    ice_lite: false
    enable_udp: true
    enable_tcp: true
    enable_turn: false
    stun_servers: []
    turn_servers: []
    max_sessions: 4096
    shard_count: 0
    read_buffer_size: 65536
    write_queue_capacity: 512
    event_queue_capacity: 1024
    session_idle_timeout_ms: 30000
    handshake_timeout_ms: 10000
    migration_route_ttl_ms: 30000
    codec_profile: browser
    prefer_video_codec: h264
    prefer_audio_codec: opus
    enable_simulcast: true
    simulcast_default_policy: highest
    enable_bwe: true
    bwe_initial_bitrate_kbps: 1200
    rtx_cache_packets: 1024
    rtx_cache_age_ms: 3000
    rtx_ratio_cap: 0.15
    video_reorder_packets: 30
    audio_reorder_packets: 10
    bootstrap_frame_count: 150
    bootstrap_max_age_ms: 5000
    datachannel:
      enabled: true
      max_channels: 32
      message_queue_capacity: 256
      max_message_bytes: 65536
    client_jobs:
      max_pull_jobs: 128
      max_push_jobs: 128
      retry_backoff_ms: 1000
      max_retry_backoff_ms: 30000
```

## 观测与诊断

WebRTC module 必须暴露：

- active sessions
- ICE state / DTLS state / selected candidate pair
- publish/play session count
- per-session ingress/egress bitrate
- packet loss、NACK count、RTX sent/received、PLI/FIR count
- TWCC/BWE estimate
- simulcast selected RID / available layers
- GOP bootstrap source：keyframe / fallback / no-keyframe
- DataChannel opened/closed/message drop count
- migration count and last route update

诊断分层：

- `WebRtcCoreDiagnostic`: SDP、ICE、DTLS、SRTP、SCTP、RTP/RTCP 状态机诊断。
- `WebRtcDriverDiagnostic`: socket、route、queue、timer、migration 诊断。
- `WebRtcModuleDiagnostic`: API、auth、stream、codec、business lifecycle 诊断。

## 错误处理

HTTP 错误映射：

- `400 Bad Request`: body/SDP/field 格式错误。
- `401/403`: 鉴权失败。
- `404`: 播放流不存在。
- `409 Conflict`: 同一 `StreamKey` 已有发布者、session 状态冲突。
- `415 Unsupported Media Type`: WHIP/WHEP body 不是 SDP。
- `422 Unprocessable Entity`: SDP 可解析但 codec/profile 不可接受。
- `503 Service Unavailable`: driver 未启动、资源耗尽。

核心错误规则：

- SDP 兼容修复失败必须返回明确诊断，不静默吞掉。
- queue full 不得阻塞热路径；按配置丢弃低优先级包或关闭 session。
- publisher lease 冲突不得绕过 engine 单发布者语义。
- module stop 必须释放所有 session、jobs、publisher/subscriber。

## 测试策略

按层测试：

- core：offer/answer、SDP fixture、RTP/RTCP event、DataChannel event、timer 推进、属性测试、fuzz。
- driver：UDP single-port、TCP candidate、route migration、bounded queue、timer、multi-session integration。
- module：WHIP/WHEP、SMS API、publish/play、engine bridge、GOP bootstrap、client/P2P job lifecycle。
- codec：WebRTC ingress/egress contract、timestamp、parameter set replay、RTP extension view。

最低检查：

```text
cargo fmt
cargo clippy -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-core
cargo clippy -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-driver-tokio
cargo clippy -p cheetah-webrtc-module --tests
cargo test -p cheetah-webrtc-module
cargo test -p cheetah-codec -- webrtc
```

