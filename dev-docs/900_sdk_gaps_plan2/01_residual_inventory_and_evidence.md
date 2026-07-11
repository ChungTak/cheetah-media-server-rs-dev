# 01 · Residual 清单与代码证据

> **Agent 用途**：动手前核对 residual 现状；禁止把 proposed 当现状。  
> **权威缺口源**：[`cheetah-media-server-rs-connector-gaps.md`](../../cheetah-media-server-rs-connector-gaps.md)。  
> **前序全量缺口**：[`cheetah-media-server-rs-gaps.md`](../../cheetah-media-server-rs-gaps.md) + plan1。

---

## 0. 基线与核对命令

connector-gaps 基线 rev：`206fc11`（PR #81）。**实现时以当前树为准**。

```bash
# connector 是否存在
test -d crates/sdk/cheetah-connector && echo OK || echo MISSING_PLAN1

# R1/R2 接线状态
rg -n 'Protocol::Rtsp|Protocol::WebRtc|UnsupportedProtocol|open_rtsp|open_webrtc' \
  crates/sdk/cheetah-connector --glob '*.rs'

# supports 矩阵
rg -n 'fn supports|Protocol::' crates/sdk/cheetah-connector/src/protocol.rs

# options / wait_ready
rg -n 'wait_ready|read_limits|queue_capacity|buffer_size' \
  crates/sdk/cheetah-connector --glob '*.rs'

# loopback layer
rg -n 'LoopbackLayer|ProtocolFraming|EngineOnly|open_in_memory_loopback' \
  crates/sdk/cheetah-connector --glob '*.rs'

# metadata / error
rg -n 'From<SdkError>|Protocol::Rtmp|duration_us|FrameOrigin' \
  crates/sdk/cheetah-connector crates/foundation/cheetah-codec/src/flv_ingress.rs \
  --glob '*.rs' | head -60
```

---

## 1. 原文 Gap → HEAD 状态 → residual

| 原文 Gap | HEAD（connector-gaps） | residual |
| --- | --- | --- |
| Gap 1 facade | **部分**（facade 有；RTSP/WebRTC 未接线） | **R1、R2、R3** |
| Gap 2 loopback | **部分**（localhost TCP；WebRTC fixture 有） | **R6** |
| Gap 3 HTTP-FLV streaming | **已实现** | — |
| Gap 4 WebRTC media peer | **fixture 已实现** | R2 要 **真实 open_push** |
| Gap 5 typed error | **已实现** | **R8** 小瑕疵 |
| Gap 6 metadata facade | **部分** | **R7** |

---

## 2. 已可消费（stable / do not regress）

详见 connector-gaps §2 与本目录 README §1。实现 residual 的 PR **不得**：

- 删除或改坏 `open_http_flv_pull` / `open_rtmp_push` / 默认 RTMP→HTTP-FLV loopback。
- 破坏 `PullHandle`/`PushHandle` 对外方法集（可 additive）。
- 把 `ConnectorError` 变回纯 string 六变体。
- 移除 feature 门控导致全量协议强制依赖。

---

## 3. R1：RTSP pull 未接线（P0）

### 3.1 现状

- `supports(Protocol::Rtsp, Direction::Pull) == true`
- `open_pull` 对 RTSP 返回 `UnsupportedProtocol`
- `pull/` 下无 RTSP adapter（仅 HTTP-FLV）

基线证据形态：

```rust
// connector.rs open_pull（基线）
#[cfg(feature = "rtsp")]
Protocol::Rtsp => Err(ConnectorError::UnsupportedProtocol {
    protocol,
    direction: Direction::Pull,
}),
```

低层可复用：

```text
crates/protocols/rtsp/driver-tokio/src/client/mod.rs
  start_tcp_client(runtime_api, peer, config, cancel) -> io::Result<RtspClientHandle>
```

### 3.2 为何阻塞 STREAM-01

验收要求 RTSP pull；无法通过 connector 得到 RTSP `SubscriberSource`。

### 3.3 proposed

```rust
// proposed
pub async fn open_rtsp_pull(
    engine: &EngineConnector /* 或内部上下文 */,
    url: &str,
    options: ConnectorPullOptions,
) -> Result<PullHandle, ConnectorError>;
```

在 `open_pull(Protocol::Rtsp, …)` 调用。长生命周期 streaming：track 发现、bounded queue、cancel、reconnect（对齐 HTTP-FLV streaming 语义）。

**设计见 [`03`](./03_rtsp_pull_adapter.md)。**

---

## 4. R2：WebRTC push 未接线（P0）

### 4.1 现状

- `supports(WebRtc, Push) == true`
- `open_push` 返回 `UnsupportedProtocol`
- 仅有 `MediaLoopbackHarness` fixture（loopback `SameProtocol`），无真实 URL `open_push`

低层可复用：

```text
crates/protocols/webrtc/driver-tokio/src/runner.rs
  spawn_driver(config, cancel) -> io::Result<Arc<WebRtcDriverHandle>>
# 以及 module WHIP / publish 路径（实现时 rg）
```

### 4.2 为何阻塞

STREAM-01 要求 WebRTC push → `PublisherSink`。

### 4.3 proposed

```rust
// proposed
pub async fn open_webrtc_push(...) -> Result<PushHandle, ConnectorError>;
```

基于 WHIP/信令 + media 发布；**禁止**仅返回 SDP 字符串。

**设计见 [`04`](./04_webrtc_push_adapter.md)。**

---

## 5. R3：能力矩阵说谎（P0）

`supports()` 宣称 RTSP pull / WebRTC push，但 `open_*` 立即失败。`tests/capability_matrix.rs` 若编码该矛盾须修正。

**规则**：`supports(p,d) == true` 当且仅当 feature 启用 **且** adapter 已接线并可进入成功路径（坏 URL 除外）。

**设计见 [`05`](./05_capability_matrix_honesty.md)。**

---

## 6. R4：options 未透传（P1）

基线形态：

```rust
// pull/http_flv.rs（基线）
let subscriber_options = HttpFlvSubscriberOptions {
    read_limits: Default::default(),
    reconnect,
    buffer_size: 64,
    cancel: options.cancel,
};
```

`ConnectorPullOptions.subscriber` 仅部分校验；`LoopbackOptions.queue_capacity` 声明未用。

**设计见 [`06`](./06_options_and_wait_ready.md)。**

---

## 7. R5：`wait_ready()` stub（P1）

```rust
// handles.rs（基线）
// TODO: wire protocol-specific readiness signalling.
Ok(())
```

导致 loopback/真实推流依赖 sleep，测试 flaky。

**设计见 [`06`](./06_options_and_wait_ready.md)。**

---

## 8. R6：默认 loopback 非 socket-free（P1）

默认 `Cross { Rtmp, HttpFlv }` 走 `127.0.0.1` 真实 client，layer=`ProtocolFraming`。

原文 Gap2 期望“无外部 server”允许 localhost；若 CI 要求 **零 socket**，则不满足。

**设计见 [`07`](./07_socket_free_loopback.md)。**

---

## 9. R7：wire metadata 未完全保真（P1 / STREAM-02）

已保真（受测）：`track_id/media_kind/codec/format/pts/dts/timebase/key/payload` 与 track 级 codec/rate/channels/extradata。

wire 丢失/改写：

| 字段 | 行为 |
| --- | --- |
| `duration` / `duration_us` | 重建为 0 |
| `origin` | 固定 `Ingest` |
| `side_data` | 不整体保留；新建 `SourceTimestamp::Rtmp` |
| 音频 flags | 恒 `START_OF_AU \| END_OF_AU` |
| 视频非关键 flags | DISCONTINUITY 等不保证 |
| `pts_us`/`dts_us` | 重算非独立透传 |
| extradata | 规范化（avcc/ASC） |

证据：`cheetah-codec` `flv_ingress.rs`、connector `push/rtmp.rs`、`tests/metadata_conformance.rs`。

**设计见 [`08`](./08_metadata_wire_fidelity.md)。**

---

## 10. R8：`From<SdkError>` 硬编码 RTMP（P2）

```rust
// error.rs（基线）
SdkError::Unavailable(msg) => Self::Connect {
    protocol: Protocol::Rtmp, // 臆测
    ...
}
```

`handles.map_sdk_error` 在已知协议时可纠正，但泛化 `From` 误标。

**设计见 [`09`](./09_error_mapping_fix.md)。**

---

## 11. connector-gaps §4 验收 ↔ 本方案

| # | 验收项 | 落点 |
| --- | --- | --- |
| 1 | supports 与 open 四方向一致 | R3 + S1/S2/S3 |
| 2 | RTSP pull streaming 行为 | R1 + R4 |
| 3 | WebRTC push + signaling≠media | R2 |
| 4 | socket-free 或标注 localhost | R6 |
| 5 | metadata 字段/不保真契约 | R7 |
| 6 | wait_ready 真语义 | R5 |

---

## 12. Non-goals

- 重做 HTTP-FLV streaming / RTMP push / 基础 loopback。
- 浏览器/Pion 外部 interop 作为 R2 必选项。
- 全协议（HLS/TS/…）connector 扩展。
- 修改 `dyun-gu-dev` 仓库。
