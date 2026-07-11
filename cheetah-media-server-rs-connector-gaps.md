# cheetah-media-server-rs：`cheetah-connector` 复核与 STREAM-01 仍缺失能力

> 本文是对 [`cheetah-media-server-rs-gaps.md`](cheetah-media-server-rs-gaps.md) 的更新复核。
> 原文基于 pinned rev `182621c`，彼时上游缺少任何高层 connector；本文基于上游最新
> `main` HEAD 复核，确认上游已新增 `cheetah-connector` crate 并落地了原文大部分缺口，
> 仅记录 `dyun-gu-dev` 实现 [STREAM-01](../design-remaining-tasks.md)（真实
> cheetah connector：RTSP/HTTP-FLV pull + RTMP/WebRTC push + 本地 loopback 集成测试）
> 时**仍需上游补齐**的能力。

## 0. 复核基线

- `dyun-gu-dev` 当前锁定 rev（`crates/dg-stream-cheetah/Cargo.toml`）：
  `182621c393eff754660a445d174a61e41622c660`。
- 本文复核的上游 HEAD：
  `206fc11bf137394293ef274576058a3b8ce060cc`（`Merge pull request #81 ... sdk-gaps-900`）。
- 新增 crate：`crates/sdk/cheetah-connector`（提交 `dc1582c feat: implement SDK gaps S1-S7`
  及后续修复）。
- 本文中的“现有 API/行为”均指 HEAD `206fc11` 可核对的代码；“建议 capability”明确标为
  proposed。

## 1. 结论摘要

| 原文缺口 | HEAD 现状 | STREAM-01 是否还阻塞 |
|---|---|---|
| Gap 1 高层 connector facade | **部分**：facade/builder/handles 已就绪，但 RTSP pull、WebRTC push 声明支持却返回 `UnsupportedProtocol` | 是（RTSP pull / WebRTC push） |
| Gap 2 内存 loopback | **部分**：RTMP→HTTP-FLV loopback 走 localhost TCP（`ProtocolFraming`），非纯内存；WebRTC 有真进程内 media fixture | 视 CI 是否要求 socket-free 而定 |
| Gap 3 流式 HTTP-FLV `SubscriberSource` | **已实现** | 否 |
| Gap 4 WebRTC 进程内 media peer | **已实现（fixture 级）**，绕过 ICE/DTLS/SRTP/UDP | 否（fixture 足够）/ 是（若要真 transport） |
| Gap 5 typed error | **已实现**（`ConnectorError`），个别 `SdkError` 映射仍粗糙 | 否（有小瑕疵） |
| Gap 6 metadata 保真 facade | **部分**：track 与关键帧元数据在受测路径保真，但 wire 重建丢失若干字段 | STREAM-02 阻塞 |

**一句话**：上游 `cheetah-connector` 已让 **HTTP-FLV pull + RTMP push + 二者 loopback**
可用；`dyun-gu-dev` 可据此实现 STREAM-01 的 loopback 验收路径。但 **RTSP pull 与
WebRTC push 尚未接线**，且若干可配置项/保真度/socket-free 语义仍缺，需上游补齐。

## 2. 已可直接消费的能力（无需上游改动）

以下已在 HEAD 落地，`dyun-gu-dev` 直接消费即可，请上游**保持稳定不要回退**：

- 高层 facade：`RuntimeConnector` trait、`EngineConnector`、`ConnectorBuilder`
  （`crates/sdk/cheetah-connector/src/connector.rs`、`.../src/engine_bootstrap.rs`）。
- 句柄：`PullHandle`（`tracks()`/`recv()`/`close()`，并实现 `SubscriberSource`）、
  `PushHandle`（`update_tracks()`/`push_frame()`/`take_keyframe_requests()`/`close()`，
  并实现 `PublisherSink`）、`LoopbackPair`（`.../src/handles.rs`）。
- 流式 HTTP-FLV pull：`open_http_flv_subscriber`（`crates/protocols/http-flv/module/src/pull/streaming.rs`），
  含 reconnect/backoff、bounded queue、cancel，经 `open_http_flv_pull` 接入 `PullHandle`。
- RTMP push：`open_rtmp_push`（`.../src/push/rtmp.rs`）。
- 内存 loopback：`open_in_memory_loopback`（`.../src/loopback.rs`），默认拓扑
  RTMP push → HTTP-FLV pull。
- typed error：`ConnectorError`（`.../src/error.rs`），带 `protocol()`/`retryable()`/`source()`。
- 能力矩阵：`Protocol`/`Direction`/`supports()`（`.../src/protocol.rs`）。
- feature 门控：`rtsp`/`http-flv`/`rtmp`/`webrtc`/`loopback`/`full`（`.../Cargo.toml`）。
- **bridge 兼容性已确认**：`AVFrame`、`TrackInfo`、`MediaKind`、`CodecId`、`FrameFormat`、
  `Timebase`、`CodecExtradata`、`TrackReadiness`、`AacRtpPacketization`、`Rational32`、
  `DispatchResult` 在 `182621c` → `206fc11` 之间**无重命名/移动/删除/签名变化**；
  `SubscriberSource::tracks()` 为带默认实现的新增方法，向后兼容。故 `dyun-gu-dev` 升级
  rev 不会破坏现有 `crates/dg-stream/src/bridge.rs`。

## 3. 仍缺失能力清单（STREAM-01 所需）

### R1：RTSP pull 未接线（声明支持但返回 `UnsupportedProtocol`）

**优先级：P0。**

**现状。** `supports(Protocol::Rtsp, Direction::Pull)` 返回 `true`
（`crates/sdk/cheetah-connector/src/protocol.rs`，`supports()`），但
`EngineConnector::open_pull` 对 RTSP 直接返回错误：

```rust
// crates/sdk/cheetah-connector/src/connector.rs（open_pull 内）
#[cfg(feature = "rtsp")]
Protocol::Rtsp => Err(ConnectorError::UnsupportedProtocol {
    protocol,
    direction: Direction::Pull,
}),
```

`crates/sdk/cheetah-connector/src/pull/` 下也没有 RTSP adapter（仅 `http_flv.rs`）。

**为何阻塞 STREAM-01。** STREAM-01 验收明确要求 RTSP pull。当前无法通过 connector 获得
RTSP 的 `SubscriberSource`。

**建议 capability（proposed）。** 参照 `open_http_flv_pull` 增加
`crate::pull::rtsp::open_rtsp_pull(engine, url, options) -> Result<PullHandle, ConnectorError>`，
基于 `crates/protocols/rtsp/driver-tokio` 的 `start_tcp_client` 组合出长生命周期、
带 track 发现、reconnect、bounded queue、cancel 的流式 `SubscriberSource`，并在
`open_pull` 的 `Protocol::Rtsp` 分支调用它。

### R2：WebRTC push 未接线（声明支持但返回 `UnsupportedProtocol`）

**优先级：P0。**

**现状。** `supports(Protocol::WebRtc, Direction::Push)` 返回 `true`，但
`EngineConnector::open_push` 对 WebRTC 返回：

```rust
// crates/sdk/cheetah-connector/src/connector.rs（open_push 内）
#[cfg(feature = "webrtc")]
Protocol::WebRtc => Err(ConnectorError::UnsupportedProtocol {
    protocol,
    direction: Direction::Push,
}),
```

WebRTC 目前只有 `MediaLoopbackHarness` fixture（经 `open_in_memory_loopback` 的
`SameProtocol` 拓扑可达），没有面向真实 URL 的 `open_push` adapter。

**为何阻塞 STREAM-01。** STREAM-01 验收要求 WebRTC push。当前无法通过 connector 获得
WebRTC 的 `PublisherSink`。

**建议 capability（proposed）。** 增加
`crate::push::webrtc::open_webrtc_push(engine, url, options) -> Result<PushHandle, ConnectorError>`，
基于 `crates/protocols/webrtc/driver-tokio` 的 `spawn_driver` 组合出 WHIP/信令 + media
发布路径，产出实现 `PublisherSink` 的句柄，并在 `open_push` 的 `Protocol::WebRtc` 分支
调用它。

### R3：能力矩阵与实现不一致（`supports()` 说谎）

**优先级：P0（正确性）。**

`supports()` 声明 RTSP pull 与 WebRTC push 支持，但实际调用返回 `UnsupportedProtocol`
（见 R1、R2）。这让外部 integrator 无法据 `supports()` 做可靠的能力判定。

**建议。** 二选一并加 conformance 测试：要么按 R1/R2 实现，使 `supports()` 与
`open_*` 行为一致；要么在实现落地前，让 `supports()` 对尚未接线的组合返回 `false`
（避免宣称能力）。当前 `tests/capability_matrix.rs` 编码了这一矛盾，应随之修正。

### R4：connector 层选项未透传（HTTP-FLV read limits/buffer、loopback queue）

**优先级：P1。**

**现状。** `open_http_flv_pull` 硬编码读参数，未透传 `ConnectorPullOptions`：

```rust
// crates/sdk/cheetah-connector/src/pull/http_flv.rs
let subscriber_options = HttpFlvSubscriberOptions {
    read_limits: Default::default(),
    reconnect,
    buffer_size: 64,
    cancel: options.cancel,
};
```

`ConnectorPullOptions.subscriber` 仅校验队列容量；`LoopbackOptions.queue_capacity`
声明了但 RTMP→HTTP-FLV loopback 未使用（`src/loopback.rs`、`src/push/rtmp.rs` 用各自
固定队列）。

**为何影响 STREAM-01。** integrator 无法通过公共 options 控制 HTTP-FLV 的
read limits/buffer size 与 loopback 队列深度（背压/内存上限不可调）。

**建议。** 将 `ConnectorPullOptions`（含 `SubscriberOptions` 的 queue capacity、
HTTP-FLV read limits/buffer size）与 `LoopbackOptions.queue_capacity` 真正透传到底层
subscriber/loopback。

### R5：`PushHandle::wait_ready()` 为 stub，恒返回 Ok

**优先级：P1。**

```rust
// crates/sdk/cheetah-connector/src/handles.rs
// TODO: wire protocol-specific readiness signalling.
Ok(())
```

**为何影响 STREAM-01。** push 侧无法可靠知道对端/track 就绪，loopback 与真实推流都要靠
sleep/轮询兜底，测试易 flaky。

**建议。** 实现协议相关的 readiness 信号（RTMP publish onStatus、WebRTC connected 等），
让 `wait_ready()` 真正等待可写状态。

### R6：默认 loopback 非 socket-free（走 localhost TCP）

**优先级：P1（取决于 CI 约束）。**

**现状。** `open_in_memory_loopback` 的默认 `Cross { push: Rtmp, pull: HttpFlv }` 会
从 engine 的 service registry 取 TCP 端点、拼 `rtmp://127.0.0.1:.../` 与
`http://127.0.0.1:.../` 并开真实协议 client，上报 `LoopbackLayer::ProtocolFraming`
（`crates/sdk/cheetah-connector/src/loopback.rs`）。这是“嵌入式 engine + localhost
协议 framing”集成测试，并非原文 Gap 2 期望的“无 socket、纯内存 transport”。

**为何影响 STREAM-01。** 若 `dyun-gu-dev` CI 要求**不开任何 socket**（原文验收第 3 条），
则该 loopback 不满足；且并发/端口占用下 ephemeral 端口路径可能 flaky。

**建议。** 提供真正的进程内 transport（protocol core 直连内存管道），或至少在 API/文档中
明确标注默认 loopback 使用 ephemeral localhost，并提供一个 socket-free 的
`LoopbackLayer`（如 `EngineOnlyBypassWire`）作为可选路径，供严格 CI 使用。

### R7：RTMP→HTTP-FLV 路径 metadata 未完全保真

**优先级：P1（阻塞 STREAM-02，不阻塞 STREAM-01 主验收）。**

**现状。** 受测路径保真了 `track_id/media_kind/codec/format/pts/dts/timebase/key-flag/
payload` 与 track 级 `codec/clock_rate/sample_rate/channels/extradata`
（`tests/metadata_conformance.rs`）。但 wire 重建（RTMP FLV 序列化 → `flv_ingress`
重建 `AVFrame`）丢失或改写：

- `duration`/`duration_us`：未随 FLV 传输，重建后为 0。
- `origin`：ingress 固定为 `FrameOrigin::Ingest`。
- `side_data`：不整体保留，ingress 新建 `SourceTimestamp::Rtmp(...)`。
- 音频 `flags`：ingress 恒置 `START_OF_AU | END_OF_AU`；视频非关键帧的
  `DISCONTINUITY/CORRUPT/DROPPABLE/GENERATED` 等不保证保留。
- `pts_us`/`dts_us`：由重建 timebase 重新计算，非独立透传。
- track extradata 会被规范化（H.264 `avcc: None` → `Some(...)`；AAC ASC 规范化）。

证据：`crates/foundation/cheetah-codec/src/flv_ingress.rs`（H.264/H.265/AAC 帧重建段）、
`crates/sdk/cheetah-connector/src/push/rtmp.rs`。

**为何影响。** STREAM-02（cheetah frame 元数据保真）要求 push/pull 全程保留上述字段，
当前 wire 路径无法逐字段保真。

**建议。** 在 FLV 映射/ingress 中携带并还原 duration、原始 flags、必要 side_data；对无法经
FLV 表达的字段，在 connector 文档中明确列为“不保真”，并提供 conformance 契约说明。

### R8：`SdkError → ConnectorError` 泛化映射把 `Unavailable` 硬编码为 RTMP

**优先级：P2。**

```rust
// crates/sdk/cheetah-connector/src/error.rs
SdkError::Unavailable(msg) => Self::Connect {
    protocol: Protocol::Rtmp,
    endpoint: msg.clone(),
    ...
}
```

`handles.rs` 的 `map_sdk_error()` 在句柄已知协议时会纠正，但直接的
`From<SdkError>` 仍是 RTMP-specific。随 RTSP/WebRTC adapter 落地会误标协议。

**建议。** 让泛化映射不臆测协议（用 `None`/上下文注入协议），或要求各 adapter 统一走
带协议上下文的 `map_sdk_error()`。

## 4. 建议的上游验收（对齐原文第 4 节）

在 `crates/sdk/cheetah-connector` 内补齐后，`examples/external_connector_loopback.rs` 与
`tests/` 应覆盖：

1. `supports()` 与 `open_pull`/`open_push` 行为对四个方向逐一一致（关闭 R3）。
2. RTSP pull：真实（或 fixture 分层标注的）streaming `recv`、取消、关闭、bounded queue、
   reconnect（关闭 R1、R4）。
3. WebRTC push：真实 `open_push` 产出 `PublisherSink`；signaling 与 media 分层验证，
   SDP 生成不得替代 media round-trip（关闭 R2；对齐原文 Gap 4）。
4. 提供并测试一个 socket-free 的 loopback 路径，或明确标注默认 loopback 走 localhost
   （关闭 R6）。
5. metadata conformance 对 `duration`、`origin`、`side_data`、精确 flags、`pts_us/dts_us`
   逐字段断言，或在契约中显式声明不保真集合（关闭 R7）。
6. `wait_ready()` 有真实就绪语义并被测试（关闭 R5）。

## 5. `dyun-gu-dev` 侧可并行推进的部分

- 现在即可：升级 `dg-stream-cheetah` 的 cheetah rev 至含 `cheetah-connector` 的提交，
  基于 `EngineConnector` 实现可安装的 embedded `CheetahRuntimeConnector`，并以
  `open_in_memory_loopback`（RTMP→HTTP-FLV）写本地 loopback 集成测试，完成 STREAM-01 的
  loopback 主验收路径。
- 待上游关闭 R1/R2 后：补齐 dg-stream 的 RTSP pull 与 WebRTC push 分支及对应测试，
  完成 STREAM-01 全能力矩阵。
- 待上游关闭 R7 后：推进 STREAM-02 的逐字段元数据保真。
