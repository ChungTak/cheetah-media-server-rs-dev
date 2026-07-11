# 04 · Gap 2：In-Process Protocol Loopback Transport

> **Agent 用途**：阶段 3 主文档——`open_in_memory_loopback` 与分层 harness。  
> **严禁**：把 `StreamManagerApi::open_publisher`→`open_subscriber` 标为 Gap 2 完成。

---

## 1. 目标

在 **无外部媒体服务器、无浏览器、无硬件** 的 CI 中验证：

```text
push -> embedded protocol runtime -> pull
```

保留（尽可能多的）：

- 协议 framing / 消息边界  
- track negotiation / metadata 路径  
- codec 相关 payload 形态（经 `cheetah-codec` 归一化前后可断言）  
- backpressure / 有界队列语义  

---

## 2. 分层定义（必须写进测试名或常量）

| 层 | 名称 | 是否算 Gap2 DoD | 说明 |
| --- | --- | --- | --- |
| **L0** | Engine smoke | **否** | 仅 stream manager 分发 |
| **L1** | Protocol loopback | **是（主）** | 至少一条完整 wire 语义路径 |
| **L2** | WebRTC 分层 | 见 `06` | signaling / media fixture / UDP |

测试模块建议：

```text
tests/engine_smoke_bypass_wire.rs      # L0，文件名含 bypass
tests/loopback_protocol_*.rs           # L1
tests/webrtc_signaling_only.rs         # L2a
tests/webrtc_media_fixture.rs          # L2b
```

在测试开头：

```rust
/// LOOPBACK_LAYER = L1
/// BYPASS_WIRE = false
```

---

## 3. Public API（proposed）

```rust
// cheetah-connector src/loopback.rs

#[derive(Debug, Clone)]
pub struct LoopbackOptions {
    /// 逻辑流名 / app/stream 等
    pub stream_name: String,
    pub subscriber: cheetah_sdk::SubscriberOptions,
    pub publisher: cheetah_sdk::PublisherOptions,
    /// 有界队列；必须 > 0
    pub queue_capacity: usize,
    pub cancel: CancellationToken,
    /// 选择 loopback 拓扑
    pub topology: LoopbackTopology,
}

#[derive(Debug, Clone)]
pub enum LoopbackTopology {
    /// 同协议两端（若支持）
    SameProtocol { protocol: Protocol },
    /// 跨协议：例如 RTMP push + HTTP-FLV pull（首版推荐）
    Cross {
        push: Protocol,
        pull: Protocol,
    },
}

pub struct LoopbackPair {
    pub publisher: PushHandle,      // 或 Box<dyn PublisherSink>
    pub subscriber: PullHandle,     // 或 Box<dyn SubscriberSource>
    /// 测试可查询：实际使用的层
    pub layer: LoopbackLayer,
    // 内部：Engine/server guards，Drop 时清理
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopbackLayer {
    EngineOnlyBypassWire,
    ProtocolFraming,
    WebRtcSignalingOnly,
    WebRtcMediaFixture,
    WebRtcLocalUdp,
}

pub async fn open_in_memory_loopback(
    options: LoopbackOptions,
) -> Result<LoopbackPair, ConnectorError>;
```

`open_in_memory_loopback` 在 `topology` 指向 WebRTC 时转到 `06` 实现；非法组合返回 `UnsupportedProtocol`。

---

## 4. 首版推荐拓扑（可落地性优先）

### 4.1 主路径：RTMP publish → 本进程 RTMP module → HTTP-FLV pull

理由：

- RTMP push client API 已存在。  
- HTTP-FLV module 可对已有 stream 提供 HTTP 播放；pull streaming（Gap3）可读回。  
- 两端均可在本进程用 ephemeral 端口（127.0.0.1 + 随机 port），**无需**完整 “零 socket” 内核——**允许 loopback TCP 到本机**。

**重要澄清（契约）**：

| 术语 | 本方案含义 |
| --- | --- |
| in-memory | 优先无 **外部** 进程/服务；允许 **本机 loopback socket** |
| zero-socket pure core | 可选增强；非 P0 必须 |

若某协议后续提供真正 `MemoryDuplex` 字节管道并接入 driver，可替换本机 TCP，API 保持不变。

### 4.2 备选路径

| 拓扑 | 前置 | 备注 |
| --- | --- | --- |
| RTMP push → RTMP play client | RTMP play 客户端可收 `AVFrame` | 同协议 |
| RTSP publish/play | 视 module 是否支持 | 可能更重 |
| WebRTC | 见 `06` | P1 |

---

## 5. 实现步骤（RTMP → HTTP-FLV）

```text
1. ConnectorBuilder::with_default_modules (rtmp + http-flv)
2. Engine start；绑定 127.0.0.1:0（或配置固定测试端口）
3. 构造 rtmp://127.0.0.1:{port}/app/stream
4. open_push(Rtmp, url, tracks+options) → PushHandle
5. wait_ready
6. 构造 http://127.0.0.1:{http_port}/... 播放 URL（对齐 HttpFlv 路由）
7. open_pull(HttpFlv, url) → PullHandle（Gap3 streaming）
8. push 若干 AVFrame（含 keyframe + extradata 就绪）
9. recv 侧断言帧到达与 metadata（Gap6）
10. close 双方；engine shutdown
```

### 5.1 端口与配置

- 使用 `ConfigStore` 或测试用 config provider 注入 listen 地址。  
- 参考现有 module 集成测：

```bash
rg -n '127.0.0.1|listen|EngineBuilder' \
  crates/protocols/rtmp/module/tests \
  crates/protocols/http-flv/module \
  --glob '*.rs' | head -50
```

- 禁止硬编码生产端口冲突；CI 并行用 OS 分配端口。

### 5.2 帧注入

- 构造最小 H.264/AAC 或已有测试 fixture 的 `AVFrame` + `TrackInfo`。  
- 复用 `cheetah-codec` 与各协议 tests 中的 helper；不要复制 NALU 组装逻辑。  
- 至少 1 个 random access / keyframe，避免订阅者 GOP 策略饿死。

### 5.3 背压

- `SubscriberOptions.queue_capacity` 设小值，推送超量时验证 `DispatchResult` / drop policy **可观测**（不断言某一种生产策略正确，但必须有界、不 OOM）。  

---

## 6. L0 Engine smoke（必须存在，单独文件）

```rust
// 概念伪代码
let sink = stream_manager.open_publisher(key, opts).await?;
let mut src = stream_manager.open_subscriber(key, opts).await?;
sink.update_tracks(tracks)?;
sink.push_frame(frame)?;
let got = src.recv().await?;
// 断言 payload 相等
```

文件头注释：

```text
/// This test BYPASSES protocol wire behavior.
/// It must NOT be counted as protocol loopback acceptance.
```

---

## 7. “纯内存字节管道”可选设计（后置）

若实现 agent 有余力且 driver 可注入 transport：

```rust
// proposed future
pub trait ByteTransport: Send + Sync { /* read/write/close */ }
pub struct MemoryByteChannel { /* mpsc/Bytes duplex */ }
```

约束：

- **不得** 把 transport trait 放进 protocol-core 公共状态机，除非保持 Sans-I/O 边界清晰。  
- driver 层注入；core 仍吃 `Input` bytes。  

P0 **不阻塞** 于此。

---

## 8. 测试清单

| ID | 用例 | 层 | 期望 |
| --- | --- | --- | --- |
| T-L0-01 | engine publish→subscribe | L0 | 帧到达；标记 bypass |
| T-L1-01 | RTMP push → HTTP-FLV pull 至少 1 视频帧 | L1 | recv 非空 |
| T-L1-02 | 多帧顺序 / keyframe 后可播 | L1 | 可断言 pts 单调或策略文档化 |
| T-L1-03 | cancel/close 终态 | L1 | recv 返回 None 或 Closed |
| T-L1-04 | 小 queue 不炸内存 | L1 | 有界 |
| T-L1-05 | 非法 topology | — | typed error |

---

## 9. DoD（阶段 3）

- [ ] `open_in_memory_loopback` 至少支持 **一条 L1** 拓扑  
- [ ] L0 smoke 存在且 **明确 bypass**  
- [ ] 无外部 server / 浏览器依赖  
- [ ] 测试可在 CI 单机跑绿  
- [ ] 文档/测试标明 layer  

---

## 10. 非目标

- 弱网 netem 仿真（可后置）。  
- 多订阅者 fan-out 压测（engine 已有原则即可）。  
- 全协议矩阵笛卡尔积（首版 1 条主路径 + 扩展位）。  
