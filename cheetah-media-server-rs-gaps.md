# cheetah-media-server-rs：面向外部 Rust SDK 集成的能力缺口

> 更新：上游已就本文缺口新增 `cheetah-connector` crate。基于上游最新 HEAD 的复核与
> STREAM-01 仍缺失能力，见
> [`cheetah-media-server-rs-connector-gaps.md`](cheetah-media-server-rs-connector-gaps.md)。
> 本文保留原始（pinned rev `182621c`）分析。

本文基于 pinned checkout：

```text
/home/ubuntu/.cargo/git/checkouts/cheetah-media-server-rs-dev-f743a1214fa3cf30/182621c
```

对应 revision 为 `182621c393eff754660a445d174a61e41622c660`。本文中的“现有 API”仅指该 checkout 中可核对的代码；“建议 API”均明确标为 proposed capability。

## 1. 背景/目标

`dyun-gu-dev` 作为外部 Rust integrator，需要通过 feature-gated、稳定且可安装的 SDK 驱动真实媒体协议路径：

- RTSP pull；
- HTTP-FLV pull；
- RTMP push；
- WebRTC push。

这些路径应能在 CI 中测试，不依赖外部媒体服务器、浏览器、硬件、native SDK、系统库或构建期间下载的 native artifact。外部调用者还需要把协议 URL/options 直接映射为 Cheetah 的 `SubscriberSource` 或 `PublisherSink`，而不是自行拼装 engine、module、driver、socket 和协议状态机。

## 2. 现状（可用的部分）

- Cheetah 采用 core + driver + module 分层：protocol core 是 Sans-I/O 状态机，driver 负责 runtime/socket/timer/task，module 集成 `cheetah-engine` 与 `EngineContext`。
- `crates/system/cheetah-engine/src/engine.rs` 提供：

  ```rust
  EngineBuilder::new(
      config_provider: Arc<dyn ConfigProvider>,
      config_apply_api: Arc<dyn ConfigApplyApi>,
      runtime_api: Arc<dyn RuntimeApi>,
  ) -> Self
  ```

  以及 `register_module_factory(...)`, `build()`, `Engine::start()`, `Engine::stop()`, `stream_manager_api()`, `publisher_api()`, `subscriber_api()` 等 API。
- `apps/cheetah-server/src/main.rs` 展示了注册 `RtmpModuleFactory`、`RtspModuleFactory`、`HttpFlvModuleFactory`、`WebRtcModuleFactory` 后启动 engine 的方式。
- `crates/sdk/cheetah-sdk/src/stream.rs` 已有高价值的 SDK contracts：

  ```rust
  pub trait PublisherSink: Send + Sync {
      fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), SdkError>;
      fn push_frame(&self, frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError>;
      fn close(&self) -> Result<(), SdkError>;
      fn take_keyframe_requests(&self) -> u64;
  }

  #[async_trait]
  pub trait SubscriberSource: Send {
      async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, SdkError>;
      async fn close(&mut self) -> Result<(), SdkError>;
      fn id(&self) -> SubscriberId;
  }
  ```

- RTSP、RTMP、HTTP-FLV、WebRTC 都有可复用的底层 Rust API；Cheetah 不是缺少协议实现，而是缺少面向外部调用者的组合层和测试 transport。
- `crates/foundation/cheetah-codec/src/frame.rs` 的 `AVFrame` 含 `track_id`、`media_kind`、`codec`、`format`、`pts`、`dts`、`timebase`、`pts_us`、`dts_us`、`duration`、`duration_us`、`flags`、`payload`、`side_data`、`origin`。
- `crates/foundation/cheetah-codec/src/track.rs` 的 `TrackInfo` 含 track/media/codec、clock/sample rate/channels、width/height/FPS、bitrate、`CodecExtradata` 和 readiness/config state；`CodecExtradata` 覆盖 H.264/H.265/H.266、AAC、AV1、VP8/VP9、Opus 等。
- 当前 checkout 的 protocol crates 未观察到 native codec dependency；但完整集成仍需额外引入 engine、runtime-tokio、module 和 driver crates，并应单独进行依赖/许可证/构建审计。

## 3. 缺失能力清单

### Gap 1：没有可安装的高层 connector/facade

**当前不足。** 外部调用者目前只能直接使用低层入口：

| 协议/方向 | 当前 API | 源码路径 |
| --- | --- | --- |
| RTSP pull | `start_tcp_client(runtime_api: Arc<dyn RuntimeApi>, peer: SocketAddr, config: RtspClientConfig, cancel: CancellationToken) -> io::Result<RtspClientHandle>` | `crates/protocols/rtsp/driver-tokio/src/client/mod.rs` |
| HTTP-FLV pull | `pull_http_flv_once(runtime_api: Arc<dyn RuntimeApi>, source_url: &str, cancel: &CancellationToken, limits: PullReadLimits) -> Result<HttpFlvPullResult, HttpFlvPullError>` | `crates/protocols/http-flv/module/src/pull.rs` |
| RTMP push | `start_client(runtime_api: Arc<dyn RuntimeApi>, url: RtmpUrl, mode: RtmpClientMode, config: RtmpClientDriverConfig, cancel: CancellationToken) -> io::Result<RtmpClientHandle>` | `crates/protocols/rtmp/driver-tokio/src/client.rs` |
| WebRTC push | `spawn_driver(...)` | `crates/protocols/webrtc/driver-tokio/src/lib.rs` |

此外还有 `EngineBuilder`、`ModuleFactory`、`StreamManagerApi`、`PublisherApi`、`SubscriberApi`、`PublisherSink`、`SubscriberSource`，但没有一个稳定的、可直接安装的 facade 把 `(protocol, url, options)` 组装成 pull `SubscriberSource` 或 push `PublisherSink`。Cheetah 中也没有与外部 repository connector 同名的通用 public `Connector` trait。

**为什么阻碍外部 SDK/CI。** 外部 integrator 必须自己处理 URL 解析、runtime/module registration、driver event/command channels、track discovery、协议生命周期、重连、backpressure 和 frame conversion。这样无法提供一个小而稳定的 `cheetah` feature，也难以对四个方向写一致的 CI tests。

**建议的 capability（proposed）。**

```rust
// proposed API；当前 checkout 中不存在
pub enum Protocol {
    Rtsp,
    HttpFlv,
    Rtmp,
    WebRtc,
}

pub trait RuntimeConnector: Send + Sync {
    fn open_pull(
        &self,
        protocol: Protocol,
        url: &str,
        options: SubscriberOptions,
    ) -> Result<PullHandle, ConnectorError>;

    fn open_push(
        &self,
        protocol: Protocol,
        url: &str,
        options: PublisherOptions,
    ) -> Result<PushHandle, ConnectorError>;
}
```

具体 handle 可以实现/包装现有 `SubscriberSource` 与 `PublisherSink`；API 应明确 RTSP/HTTP-FLV 只允许 pull、RTMP/WebRTC 只允许 push 的能力矩阵，并提供 `EngineBuilder`/module registration 的默认组合。

**优先级：P0。**

### Gap 2：没有 in-process/in-memory protocol loopback transport

**当前不足。** RTSP `start_tcp_client`/server 依赖 TCP，RTSP 还可能使用 UDP RTP/RTCP；HTTP-FLV pull/server 依赖 HTTP/WebSocket TCP；RTMP client/server 依赖 TCP；WebRTC full media path 需要 ICE/STUN、UDP 或 RFC 4571 TCP、DTLS/SRTP。当前没有让 protocol core 直接在内存中互连的统一 transport。

WebRTC 的 `InMemoryTransport::pair(capacity: usize) -> (Self, Self)` 位于：

```text
crates/protocols/webrtc/module/src/p2p/transport.rs
```

其 `P2pTransport` trait：

```rust
async fn send(&self, message: P2pMessage) -> Result<(), P2pTransportError>;
async fn recv(&self) -> Result<P2pTransportEvent, P2pTransportError>;
async fn close(&self);
```

是 P2P signaling transport，不是 media transport。`crates/protocols/webrtc/module/tests/cheetah_self_interop.rs` 也只是在进程内调用 module HTTP service、提交 WHIP offer 并验证生成 SDP；它没有完成 media push→pull round-trip。

**为什么阻碍外部 SDK/CI。** 不能在无外部 server、无 socket、无硬件、无 browser/peer 的条件下验证：

```text
push -> embedded protocol runtime -> pull
```

直接走 `Engine::publisher_api()`/`StreamManagerApi::open_publisher` 到 `open_subscriber` 的 loopback 是可行的，但那绕过了 RTSP/HTTP-FLV/RTMP/WebRTC wire behavior，不能替代 protocol conformance test。

**建议的 capability（proposed）。**

提供 protocol-independent 或 protocol-specific 的 in-memory transport/harness，例如：

```rust
// proposed API；当前 checkout 中不存在
pub struct LoopbackPair {
    pub publisher: Box<dyn PublisherSink>,
    pub subscriber: Box<dyn SubscriberSource>,
}

pub async fn open_in_memory_loopback(
    protocol: Protocol,
    options: LoopbackOptions,
) -> Result<LoopbackPair, ConnectorError>;
```

它应能在 CI 中执行 push→protocol runtime→pull，并保留协议 framing、track negotiation、codec metadata 和 backpressure 语义；如果完整 WebRTC ICE/DTLS/SRTP 内存实现成本过高，应至少提供分层 fixture：protocol core loopback、signaling loopback、media fixture，以及明确标注的 optional local-UDP integration test。

**优先级：P0。**

### Gap 3：HTTP-FLV pull 只有 one-shot result，不是 streaming `SubscriberSource`

**当前不足。** `crates/protocols/http-flv/module/src/pull.rs` 暴露：

```rust
pub async fn pull_http_flv_once(
    runtime_api: Arc<dyn RuntimeApi>,
    source_url: &str,
    cancel: &CancellationToken,
    limits: PullReadLimits,
) -> Result<HttpFlvPullResult, HttpFlvPullError>
```

结果是：

```rust
pub struct HttpFlvPullResult {
    pub header: Option<FlvHeader>,
    pub tags: Vec<FlvTag>,
    pub previous_tag_size_mismatch_count: u64,
}
```

同文件还提供 `pull_flv_once(...)`、`pull_ws_flv_once(...)`。这些 API 适合一次性读取和解析，但不提供长生命周期的 frame-by-frame `recv`、reconnect/backoff、bounded queue/backpressure 或 cancellation-aware `SubscriberSource` 生命周期。

**为什么阻碍外部 SDK/CI。** 外部 integrator 必须自行把 `Vec<FlvTag>` 变成 `AVFrame`/`MediaFrame`，自行维护持续连接、断线重连、队列上限和关闭语义；这不能直接接入已有 `SubscriberSource::recv()`。

**建议的 capability（proposed）。**

```rust
// proposed API；当前 checkout 中不存在
pub trait HttpFlvSubscriberSource: Send {
    async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, HttpFlvPullError>;
    async fn close(&mut self) -> Result<(), HttpFlvPullError>;
}

pub async fn open_http_flv_subscriber(
    runtime_api: Arc<dyn RuntimeApi>,
    source_url: &str,
    options: HttpFlvSubscriberOptions,
    cancel: CancellationToken,
) -> Result<Box<dyn SubscriberSource>, HttpFlvPullError>;
```

应规定 FLV audio/video tags 到 `AVFrame` 的 codec/format/PTS/DTS/timebase/flags/extradata 映射、队列上限、重连策略和取消后的终态。

**优先级：P0。**

### Gap 4：WebRTC 没有 in-process media loopback peer

**当前不足。** `spawn_driver(...)` 所在的 `crates/protocols/webrtc/driver-tokio/src/lib.rs` 面向真实 UDP/RFC 4571 TCP、STUN/ICE、DTLS/SRTP driver；`InMemoryTransport` 仅覆盖 signaling。现有 `cheetah_self_interop.rs` 证明了 in-process engine/module/WHIP signaling，但没有一个无需 browser、Pion、ZLM、Janus 或外部 UDP peer 的 WebRTC media peer。

**为什么阻碍外部 SDK/CI。** WebRTC push connector 即使能生成 offer/建立 signaling，也无法在纯进程内测试完整 media publish→receive、codec negotiation、SRTP packetization、track lifecycle 和 frame metadata preservation。将 signaling test 误当作 media loopback 会给外部 SDK 错误的稳定性保证。

**建议的 capability（proposed）。**

提供 `InMemoryWebRtcPeer` 或 documented loopback harness，至少支持：

```text
publish media -> WebRTC path -> receive media
```

并保留 `AVFrame`/`TrackInfo` 中的 codec、format、timebase、PTS/DTS、flags、extradata。若完整 in-memory ICE/DTLS/SRTP peer 暂不现实，建议将验收拆成：

1. in-process signaling/SDP test；
2. deterministic media-path fixture/test transport；
3. optional real UDP integration test。

**优先级：P1。**

### Gap 5：错误接口过于 coarse/stringly，缺少统一 protocol error mapping

**当前不足。** `crates/sdk/cheetah-sdk/src/error.rs` 的 SDK error 是：

```rust
pub enum SdkError {
    NotFound(String),
    AlreadyExists(String),
    InvalidArgument(String),
    Conflict(String),
    Unavailable(String),
    Internal(String),
}
```

协议错误则分散在 `HttpFlvPullError`、RTSP driver/socket errors、RTMP driver errors、`WebRtcCoreError` 和 WebRTC driver errors 中；没有统一的外部 connector error。很多可匹配信息只存在于 `String`，外部 integrator 无法稳定地区分 retryable connect failure、bad URL、HTTP status、protocol rejection、backpressure、codec mismatch 和 terminal close。

**为什么阻碍外部 SDK/CI。** `dg-core::Error` mapping、重试/backoff、telemetry 标签和用户诊断都只能依赖字符串或各协议专用分支，升级 Cheetah 后容易发生语义回归。

**建议的 capability（proposed）。**

```rust
// proposed API；当前 checkout 中不存在
pub enum ConnectorError {
    InvalidUrl { protocol: Protocol, url: String },
    UnsupportedProtocol { protocol: Protocol, direction: Direction },
    Connect { protocol: Protocol, endpoint: Endpoint, source: Box<dyn Error + Send + Sync> },
    Protocol { protocol: Protocol, operation: Operation, source: Box<dyn Error + Send + Sync> },
    Media { codec: Option<CodecId>, source: Box<dyn Error + Send + Sync> },
    Backpressure { protocol: Protocol },
    Closed { protocol: Protocol, reason: CloseReason },
}
```

这是建议形状，不是当前 API；无论具体命名如何，应提供稳定 typed variants、`source()`、protocol、operation、endpoint/stream key、retryable、HTTP/socket status 和 codec/media context。

**优先级：P1。**

### Gap 6：没有 metadata-preserving 的端到端高层 facade

**现状判断。** Cheetah 的数据模型本身足够丰富：`AVFrame` 有 track/media/codec/format/timebase/PTS/DTS/duration/flags/payload/side_data/origin，`TrackInfo` 有 codec parameters 和 `CodecExtradata`。因此缺口不是 `AVFrame` 缺字段。

缺口在于：当前只有底层 driver/module/engine APIs，没有一个高层 connector contract 保证：

```text
protocol input/output
    -> PublisherSink / SubscriberSource
    -> AVFrame / TrackInfo
```

在整个生命周期中保留上述 metadata。外部 integrator 仍需自己把 URL、协议 track negotiation、driver event、`AVFrame` 和 `TrackInfo` 组装起来。`dyun-gu-dev` 当前 bridge 中曾使用 `MediaKind::Data`、`CodecId::Unknown`、`FrameFormat::Unknown` 和 `Timebase::new(1, 1)` placeholder；这是 integrator bridge debt，不应完全归咎于 Cheetah，但也说明上游没有一个可直接消费的 metadata-preservation facade 或 conformance contract。

**建议的 capability（proposed）。** 将 metadata preservation 写入 connector contract：

```rust
// proposed API；当前 checkout 中不存在
pub trait MetadataPreservingConnector {
    async fn open_pull(
        &self,
        protocol: Protocol,
        url: &str,
        options: SubscriberOptions,
    ) -> Result<(Vec<TrackInfo>, Box<dyn SubscriberSource>), ConnectorError>;

    async fn open_push(
        &self,
        protocol: Protocol,
        url: &str,
        options: PublisherOptions,
        tracks: Vec<TrackInfo>,
    ) -> Result<Box<dyn PublisherSink>, ConnectorError>;
}
```

并提供 conformance tests，逐字段断言 `track_id`、`media_kind`、`codec`、`format`、`timebase`、PTS/DTS、duration、flags、payload、side data、origin、extradata 和 readiness/config state 在 protocol adapter 前后不被静默替换或丢失。

**优先级：P1。**

## 4. 验收建议

建议上游提供一个不依赖外部服务的 `examples/external_connector_loopback.rs` 或等价 integration test：

1. 只启用明确的 Rust features，构建时不下载或编译 native SDK；
2. 安装高层 connector，验证 RTSP/HTTP-FLV pull 与 RTMP/WebRTC push 的 capability matrix；
3. 用 in-memory loopback harness 执行 `push -> embedded protocol runtime -> pull`；若某协议只能做分层测试，应明确标记绕过了哪一层；
4. HTTP-FLV 测试逐 frame 验证 streaming `recv`、取消、关闭、bounded queue 和 reconnect policy；
5. WebRTC 测试分别验证 signaling 和 media；SDP 生成不能作为 media round-trip 的替代；
6. protocol conformance test 对 `TrackInfo`/`AVFrame` metadata、`CodecExtradata`、PTS/DTS/timebase、flags 和 keyframe requests 做逐字段断言；
7. error conformance test 验证 typed protocol errors、retryable 语义、`source()` 链和 endpoint/status/codec context；
8. 另提供 direct engine `open_publisher`→`open_subscriber` smoke test，并明确该测试绕过协议 wire behavior，不能替代各协议 loopback。
