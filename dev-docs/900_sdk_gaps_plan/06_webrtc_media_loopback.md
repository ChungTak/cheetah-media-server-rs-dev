# 06 · Gap 4：WebRTC In-Process Media Loopback Peer

> **Agent 用途**：阶段 6 主文档——WebRTC media 进程内可测性。  
> **现状**：`InMemoryTransport` 仅 P2P **signaling**；`spawn_driver` 面向真实网络。  
> **严禁**：用 “WHIP 返回 SDP” 测试冒充 media round-trip 完成。

---

## 1. 目标

至少支持文档化的：

```text
publish media -> WebRTC path -> receive media
```

并在可行层保留 `AVFrame`/`TrackInfo` 的 codec、format、timebase、PTS/DTS、flags、extradata。

若完整 in-memory ICE/DTLS/SRTP 成本过高，**允许分层交付**，但每层必须诚实标注。

---

## 2. 分层验收（强制）

| 层 | 名称 | 是否算 media done | 内容 |
| --- | --- | --- | --- |
| **W0** | Engine smoke | 否 | 与协议无关 |
| **W1** | Signaling/SDP | **否**（仅 signaling done） | WHIP/WHEP offer/answer，in-process HTTP |
| **W2** | Media fixture | **是（最低 media DoD）** | 确定性 test transport 跑通 packetize→depacketize→AVFrame |
| **W3** | Local UDP peer | 增强 | 本机 UDP + 真实 driver 子集 |
| **W4** | 外部 interop | 范围外可选 | Pion/浏览器（已有 plans-27 体系） |

**Gap 4 完成定义**：**W1 + W2 必须**；W3 鼓励；W4 不在本方案强制。

---

## 3. 可复用现状

```text
crates/protocols/webrtc/module/src/p2p/transport.rs
  InMemoryTransport::pair  → signaling only

crates/protocols/webrtc/driver-tokio/src/runner.rs
  spawn_driver             → real ICE/UDP/TCP

crates/protocols/webrtc/module/tests/
  p2p_pipeline.rs, interop_harness.rs, cheetah_self_interop（若有）
```

核对：

```bash
rg -n 'InMemoryTransport|spawn_driver|WHIP|WHEP|self_interop' \
  crates/protocols/webrtc --glob '*.rs' | head -80
```

---

## 4. 建议 API（proposed）

### 4.1 统一入口（connector）

```rust
// cheetah-connector
pub async fn open_webrtc_media_loopback(
    options: WebRtcLoopbackOptions,
) -> Result<WebRtcLoopbackPair, ConnectorError>;

pub struct WebRtcLoopbackPair {
    pub publisher: PushHandle,
    pub subscriber: PullHandle, // 或专用 Receiver
    pub layer: LoopbackLayer,   // WebRtcMediaFixture / WebRtcLocalUdp
}
```

也可并入 `open_in_memory_loopback(LoopbackTopology::SameProtocol { protocol: WebRtc })`。

### 4.2 Media fixture 核心（webrtc module）

```rust
// proposed: crates/protocols/webrtc/module/src/testing/media_loopback.rs
// 或 feature = "test-utils" 导出

/// Deterministic media path without public ICE/STUN.
pub struct MediaLoopbackHarness { /* … */ }

impl MediaLoopbackHarness {
    pub async fn new(runtime: Arc<dyn RuntimeApi>, opts: …) -> Result<Self, …>;
    /// 推送侧：接收 AVFrame，走 packetizer → (fixture transport) → depacketizer
    pub fn publisher_sink(&self) -> impl PublisherSink;
    /// 接收侧
    pub async fn open_subscriber(&self) -> impl SubscriberSource;
}
```

### 4.3 可选：InMemory media transport

```rust
// 注意：不是 P2pTransport
pub trait MediaDatagramTransport: Send + Sync {
    async fn send_datagram(&self, bytes: Bytes) -> Result<(), …>;
    async fn recv_datagram(&self) -> Result<Bytes, …>;
    async fn close(&self);
}

pub struct InMemoryDatagramPair { /* mpsc 有界 */ }
```

注入点在 **driver** 或 test harness，**不要**污染 core 的 Sans-I/O 边界（core 继续吃 `Input`）。

---

## 5. W2 Media fixture 实现策略

### 5.1 推荐最小闭环

```text
AVFrame (H264/Opus 等)
  → webrtc payload packetizer (既有 codec/rtp 路径)
  → InMemoryDatagramPair
  → depacketizer / jitter 简化路径
  → AVFrame out
```

要求：

1. 使用 **真实** packetizer/depacketizer 代码路径，而非 `memcpy` 绕过。  
2. 可跳过 ICE consent / STUN binding（fixture 模式）。  
3. 可跳过 DTLS/SRTP **仅当** 明确 `layer = WebRtcMediaFixture` 且测试名含 `fixture`；若跳过，文档写 `BYPASS_DTLS_SRTP=true`。  
4. 若团队选择 fixture 也走 SRTP（自签短期密钥），更佳，但非阻塞。

### 5.2 与 str0m / driver 的关系

本仓库 WebRTC 依赖 `str0m`（workspace）。实现前评估：

| 选项 | 优点 | 缺点 |
| --- | --- | --- |
| A. 在 driver 增加 `TransportMode::Fixture(Datagram)` | 路径真实 | 改动面中等 |
| B. 单测直接调用 core packet 逻辑 | 改动小 | 可能漏 driver 集成 |
| C. 本机 UDP 两个 endpoint | 接近真实 | CI 偶发 flaky |

**推荐**：B 先保证 codec/rtp 往返（W2 最低），A 作为增强，C 为 W3。

### 5.3 Signaling（W1）保持独立

继续用 HTTP WHIP/WHEP + `InMemoryTransport` 测 SDP/状态机。  
**禁止** 在 W1 测试里 `assert!(sdp.contains("m=video"))` 后宣称 media OK。

---

## 6. Connector 集成

```text
EngineConnector::open_push(Protocol::WebRtc, whip_url, opts)
  → 生产路径：真实 driver

open_webrtc_media_loopback / open_in_memory_loopback(WebRtc)
  → 测试路径：MediaLoopbackHarness
```

生产 `open_push` **不得** 在未配置时静默走 fixture。

---

## 7. 测试清单

| ID | 层 | 用例 | 期望 |
| --- | --- | --- | --- |
| T-W1-01 | W1 | in-process WHIP offer/answer | SDP 成功；**不** recv 媒体 |
| T-W2-01 | W2 | fixture 推 1 个关键帧 | recv 得到 payload 语义一致 |
| T-W2-02 | W2 | tracks/extradata/pts | 字段断言（对齐 Gap6） |
| T-W2-03 | W2 | close/cancel | 终态干净 |
| T-W3-01 | W3 | 可选 local UDP | feature 或 `#[ignore]` 默认 |

---

## 8. DoD（阶段 6）

- [ ] W1 测试存在且标注 signaling-only  
- [ ] W2 media fixture 至少 1 条 publish→receive AVFrame  
- [ ] 测试名/常量标明 bypass 的层（ICE/DTLS/SRTP）  
- [ ] connector 可调用或 example 引用 harness  
- [ ] **没有** 仅靠 SDP 的 “media 完成” 声明  

---

## 9. 风险与回退

| 风险 | 回退 |
| --- | --- |
| str0m 难以注入内存 transport | W2 走 codec/rtp 单元路径 + W3 UDP |
| SRTP 强制 | fixture 用固定 test key；文档说明 |
| flaky UDP | W3 ignore in default CI |

---

## 10. 非目标

- 完整 simulcast/BWE/FEC 矩阵（见 plans-27-*）。  
- 浏览器 Playwright 作为本 Gap 必选项。  
- 替换 P2P signaling `InMemoryTransport` 语义。  
