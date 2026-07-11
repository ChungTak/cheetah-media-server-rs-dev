# 01 · Gap 清单与代码证据

> **Agent 用途**：动手前核对“现状到底是什么”；禁止把 proposed 当现状，禁止跳过证据臆测依赖。  
> **权威缺口源**：仓库根 [`cheetah-media-server-rs-gaps.md`](../../../cheetah-media-server-rs-gaps.md)。

---

## 0. 核对 revision 说明

gaps 文档基于历史 pinned checkout `182621c`。本方案实现时以 **当前工作区源码** 为准；若路径迁移，先 `rg` 再改计划中的路径，不要盲信旧路径。

核对命令：

```bash
rg -n 'fn start_tcp_client|fn pull_http_flv_once|fn start_client|fn spawn_driver' --glob '*.rs'
rg -n 'trait PublisherSink|trait SubscriberSource|enum SdkError|struct EngineBuilder' --glob '*.rs'
rg -n 'InMemoryTransport|struct HttpFlvPullResult' --glob '*.rs'
```

---

## 1. 分层与依赖现状（摘要）

| 层 | crate 示例 | 职责 |
| --- | --- | --- |
| Foundation | `cheetah-codec` | `AVFrame` / `TrackInfo` / 封装视图 |
| Runtime | `cheetah-runtime-api` / `cheetah-runtime-tokio` | `RuntimeApi`、`CancellationToken` |
| SDK | `cheetah-sdk` | module 契约、流 API、`SdkError` |
| System | `cheetah-engine` / `cheetah-config` | Engine 组装、分发 |
| Protocol | `cheetah-<proto>-{core,driver-tokio,module}` | 状态机 / I/O / 引擎接入 |
| App | `cheetah-server` | 二进制组装 |

**结论**：协议能力在 module/driver 中 **已存在**；缺口是 **对外组合层 + 测试 transport + streaming pull + 错误/metadata 契约**。

---

## 2. Gap 1：没有可安装的高层 connector/facade

### 2.1 当前不足

外部调用者只能直接使用低层入口，没有稳定的 `(protocol, url, options) → pull/push handle` facade，也没有与外部 repository 同名的通用 public `Connector` trait。

### 2.2 代码证据

| 协议/方向 | 当前 API | 源码路径 |
| --- | --- | --- |
| RTSP pull | `start_tcp_client(runtime_api, peer, config, cancel) -> io::Result<RtspClientHandle>` | `crates/protocols/rtsp/driver-tokio/src/client/mod.rs` |
| HTTP-FLV pull | `pull_http_flv_once(runtime_api, source_url, cancel, limits) -> Result<HttpFlvPullResult, …>` | `crates/protocols/http-flv/module/src/pull.rs` |
| RTMP push | `start_client(runtime_api, url, mode, config, cancel) -> io::Result<RtmpClientHandle>` | `crates/protocols/rtmp/driver-tokio/src/client.rs` |
| WebRTC push | `spawn_driver(...)` | `crates/protocols/webrtc/driver-tokio/src/runner.rs` |

引擎组装（应用层样板，**不是**库 facade）：

```text
apps/cheetah-server/src/main.rs
  EngineBuilder::new(config, config, runtime)
    .register_module_factory(RtmpModuleFactory / RtspModuleFactory / …)
```

SDK 已有契约（可被 facade 包装）：

```text
crates/sdk/cheetah-sdk/src/stream.rs
  trait PublisherSink { update_tracks, push_frame, close, take_keyframe_requests }
  trait SubscriberSource { recv, close, id }
  trait StreamManagerApi { open_publisher, open_subscriber, … }
```

SDK 依赖面（**不含**协议 module）：

```text
crates/sdk/cheetah-sdk/Cargo.toml
  cheetah-codec, cheetah-runtime-api, …  # 无 rtmp/rtsp/http-flv/webrtc module
```

### 2.3 为何阻碍外部 SDK/CI

Integrator 必须自行：URL 解析、runtime/module 注册、driver channel、track discovery、生命周期、重连、backpressure、frame 转换。无法提供小而稳定的 `cheetah` feature 表面，也难以对四方向写一致 CI。

### 2.4 建议 capability（proposed）

见 [`03`](./03_connector_facade.md)。摘要：

```rust
// proposed；当前不存在
pub enum Protocol { Rtsp, HttpFlv, Rtmp, WebRtc }

pub trait RuntimeConnector: Send + Sync {
    fn open_pull(&self, protocol: Protocol, url: &str, options: SubscriberOptions)
        -> Result<PullHandle, ConnectorError>;
    fn open_push(&self, protocol: Protocol, url: &str, options: PublisherOptions)
        -> Result<PushHandle, ConnectorError>;
}
```

**优先级：P0。**

---

## 3. Gap 2：没有 in-process/in-memory protocol loopback transport

### 3.1 当前不足

| 协议 | 生产 transport | 进程内 media loopback |
| --- | --- | --- |
| RTSP | TCP + 可能 UDP RTP/RTCP | **无** 统一内存 media transport |
| HTTP-FLV | HTTP/WS TCP | **无** |
| RTMP | TCP | **无** |
| WebRTC | ICE/STUN + UDP 或 RFC4571 TCP + DTLS/SRTP | **无** media；仅有 P2P **signaling** in-memory |

### 3.2 代码证据

WebRTC `InMemoryTransport`（**signaling only**）：

```text
crates/protocols/webrtc/module/src/p2p/transport.rs
  InMemoryTransport::pair(capacity) -> (Self, Self)
  trait P2pTransport { send(P2pMessage), recv -> P2pTransportEvent, close }
```

模块注释明确：生产接 WebSocket；测试驱动 **signaling 状态机**，不是 media path。

`cheetah_self_interop` 类测试（若存在）：进程内 WHIP/HTTP/SDP，**不等于** media push→pull round-trip。实现前用：

```bash
rg -n 'self_interop|whip|whep' crates/protocols/webrtc --glob '*.rs' | head
```

Engine 直连 loopback **可行但绕过 wire**：

```rust
// 概念：StreamManagerApi::open_publisher + open_subscriber
// 不经过 RTSP/HTTP-FLV/RTMP/WebRTC framing
```

### 3.3 为何阻碍

无法在无外部 server、无真实 socket peer、无 browser 条件下验证：

```text
push -> embedded protocol runtime -> pull
```

### 3.4 建议 capability（proposed）

见 [`04`](./04_loopback_transport.md)。摘要：

```rust
// proposed
pub struct LoopbackPair {
    pub publisher: Box<dyn PublisherSink>,
    pub subscriber: Box<dyn SubscriberSource>,
}

pub async fn open_in_memory_loopback(
    protocol: Protocol,
    options: LoopbackOptions,
) -> Result<LoopbackPair, ConnectorError>;
```

**优先级：P0。**

---

## 4. Gap 3：HTTP-FLV pull 只有 one-shot，不是 streaming `SubscriberSource`

### 4.1 当前不足

`pull_http_flv_once` / `pull_flv_once` / `pull_ws_flv_once` 返回一次性 `HttpFlvPullResult { header, tags, … }`，无长生命周期 `recv`、无 bounded queue 背压语义接入 `SubscriberSource`。

### 4.2 代码证据

```text
crates/protocols/http-flv/module/src/pull.rs
  pub struct PullReadLimits { … }
  pub struct HttpFlvPullResult {
      pub header: Option<FlvHeader>,
      pub tags: Vec<FlvTag>,
      pub previous_tag_size_mismatch_count: u64,
  }
  pub enum HttpFlvPullError { InvalidUrl, Connect, BadStatusCode, Cancelled, FlvDemux, … }
  pub async fn pull_http_flv_once(...) -> Result<HttpFlvPullResult, HttpFlvPullError>
```

`HttpFlvPullError::retryable()` 存在但为 **私有** 方法（`fn retryable`，非 `pub`），外部无法稳定复用。

FLV → `AVFrame` 映射在 codec/property 测试中有相关工具，但 **没有** 对外 streaming subscriber API：

```bash
rg -n 'map_frame_to_rtmp_flv|FlvDemux|AVFrame' crates/protocols/http-flv crates/foundation/cheetah-codec --glob '*.rs' | head
```

### 4.3 为何阻碍

Integrator 必须自行：`Vec<FlvTag>` → `AVFrame`、长连接、重连、队列上限、close 语义，无法直接 `SubscriberSource::recv()`。

### 4.4 建议 capability（proposed）

见 [`05`](./05_http_flv_streaming_subscriber.md)。

**优先级：P0。**

---

## 5. Gap 4：WebRTC 没有 in-process media loopback peer

### 5.1 当前不足

- `spawn_driver` 面向真实网络 path。
- `InMemoryTransport` 仅 P2P signaling。
- 现有 self interop 证明 in-process engine/module/WHIP signaling，**无** 无需 browser/Pion/ZLM/Janus/外部 UDP 的 media peer。

### 5.2 代码证据

```text
crates/protocols/webrtc/driver-tokio/src/runner.rs   # spawn_driver
crates/protocols/webrtc/module/src/p2p/transport.rs  # signaling InMemoryTransport
crates/protocols/webrtc/module/tests/…               # interop harness / p2p_pipeline（signaling）
```

### 5.3 为何阻碍

WebRTC push connector 即使能生成 offer/建立 signaling，也无法在纯进程内测完整 media publish→receive、codec negotiation、packetization、track lifecycle、frame metadata。

### 5.4 建议 capability（proposed）

见 [`06`](./06_webrtc_media_loopback.md)。分层验收：

1. in-process signaling/SDP  
2. deterministic media-path fixture / test transport  
3. optional real UDP integration（可 feature 门控）

**优先级：P1。**

---

## 6. Gap 5：错误接口 coarse/stringly，缺统一 protocol error mapping

### 6.1 当前不足

SDK：

```rust
// crates/sdk/cheetah-sdk/src/error.rs
pub enum SdkError {
    NotFound(String),
    AlreadyExists(String),
    InvalidArgument(String),
    Conflict(String),
    Unavailable(String),
    Internal(String),
}
```

协议错误分散：`HttpFlvPullError`、RTSP/RTMP `io::Error`、`WebRtcCoreError`、driver errors 等。可匹配信息常在 `String` 中。

### 6.2 为何阻碍

外部 `Error` mapping、重试/backoff、telemetry 标签、用户诊断只能靠字符串或各协议专用分支；升级后易语义回归。

### 6.3 建议 capability（proposed）

见 [`07`](./07_connector_error_mapping.md)。**不**强制替换全仓库 `SdkError`；优先在 **connector 边界** 提供 typed `ConnectorError`。

**优先级：P1。**

---

## 7. Gap 6：没有 metadata-preserving 端到端高层 facade

### 7.1 现状判断（字段本身足够）

`AVFrame`（`crates/foundation/cheetah-codec/src/frame.rs`）含：

`track_id`, `media_kind`, `codec`, `format`, `pts`, `dts`, `timebase`, `pts_us`, `dts_us`, `duration`, `duration_us`, `flags`, `payload`, `side_data`, `origin`。

`TrackInfo`（`track.rs`）含 track/media/codec、clock/sample rate/channels、width/height/FPS、bitrate、`CodecExtradata`、readiness/config state。

**缺口不是字段缺失**，而是没有高层 connector **保证** 在

```text
protocol I/O → PublisherSink / SubscriberSource → AVFrame / TrackInfo
```

生命周期中这些字段不被静默替换或丢失；也没有 conformance 测试约束协议 adapter。

### 7.2 为何阻碍

外部 bridge 易退化为 `MediaKind::Data` / `CodecId::Unknown` / `Timebase::new(1,1)` placeholder；根因是上游缺可消费 facade + 契约测。

### 7.3 建议 capability（proposed）

见 [`08`](./08_metadata_preservation.md)。

**优先级：P1。**

---

## 8. gaps.md 验收建议 ↔ 本方案

| # | gaps.md §4 | 本方案落点 |
| --- | --- | --- |
| 1 | 明确 Rust features；无 native artifact 下载 | Gap 1 feature 矩阵 + `09` |
| 2 | 安装高层 connector；验证 capability matrix | Gap 1 + `03` |
| 3 | in-memory loopback `push→runtime→pull` | Gap 2 + `04` |
| 4 | HTTP-FLV streaming recv/cancel/queue/reconnect | Gap 3 + `05` |
| 5 | WebRTC signaling 与 media 分测 | Gap 4 + `06` |
| 6 | TrackInfo/AVFrame metadata 字段断言 | Gap 6 + `08` |
| 7 | typed errors / retryable / source | Gap 5 + `07` |
| 8 | engine smoke 且标注绕过 wire | `04` / `09` 分测 |

---

## 9. Non-goals 再确认

- 不在 connector 实现协议状态机。
- 不让 `cheetah-sdk` 依赖协议 modules。
- 不把 engine 直连 smoke 当作 protocol conformance。
- 不要求首版覆盖 HLS/TS/SRT/GB28181 connector。
- 不把 WebRTC SDP 生成测当作 media loopback done。
