# 07 · R6：Loopback Socket-Free 语义与诚实标注

> **Agent 用途**：阶段 5 主文档。  
> **问题**：默认 `open_in_memory_loopback` 实际是 **localhost TCP 协议 framing**，不是零 socket 纯内存。

---

## 1. 目标 / 非目标

| 目标 | 说明 |
| --- | --- |
| T1 | API/文档/Layer **诚实**描述默认路径用 ephemeral localhost |
| T2 | 提供 **可选** socket-free 路径供严格 CI（engine-only 或真内存管道） |
| 非目标 | 强制删除 ProtocolFraming 默认（其对 STREAM-01 有价值） |
| 非目标 | 全协议零 socket media 仿真（WebRTC 已有 fixture 分层） |

---

## 2. 现状

```text
open_in_memory_loopback
  topology 默认 Cross { Rtmp, HttpFlv }
  → engine service registry 取 TCP 端点
  → rtmp://127.0.0.1:… + http://127.0.0.1:…
  → layer = ProtocolFraming
```

connector-gaps：这是嵌入式 engine + localhost 集成测，满足“无外部媒体服务器”，**不满足**“无任何 socket”。

---

## 3. 分层模型（与 plan1 对齐，写进 API）

| Layer | Socket | Wire | 用途 |
| --- | --- | --- | --- |
| `EngineOnlyBypassWire` | 否（可不绑监听） | 否 | 严格 CI / 快速 smoke |
| `ProtocolFraming` | localhost TCP | 是 | 默认 STREAM-01 主路径 |
| `WebRtcMediaFixture` | 否或最少 | 部分 bypass ICE/DTLS/SRTP | 已有 |
| `WebRtcLocalUdp` | 是 | 是 | 可选 |

### 3.1 proposed API

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopbackLayer {
    EngineOnlyBypassWire,
    ProtocolFraming,
    WebRtcSignalingOnly,
    WebRtcMediaFixture,
    WebRtcLocalUdp,
}

#[derive(Debug, Clone)]
pub struct LoopbackOptions {
    pub stream_name: String,
    pub queue_capacity: usize, // R4
    pub topology: LoopbackTopology,
    /// 请求的层；若无法满足 → 错误或降级策略见下
    pub preferred_layer: LoopbackLayer,
    pub cancel: CancellationToken,
    // subscriber/publisher options …
}

pub struct LoopbackPair {
    pub publisher: PushHandle,
    pub subscriber: PullHandle,
    pub layer: LoopbackLayer, // 实际使用的层，必须准确
}
```

### 3.2 降级策略（钉死一种）

**推荐**：严格模式——`preferred_layer` 无法满足则 `Err(Unsupported…)`，**禁止**静默降级到更弱/更强层。

可选宽松模式仅用于内部调试，不得作为默认。

### 3.3 Socket-free 实现选项

| 选项 | 实现 | DoD 是否足够 |
| --- | --- | --- |
| **A. EngineOnly** | `open_publisher` + `open_subscriber` 同 stream key | **是**（标注 bypass wire） |
| B. Memory byte duplex 注入 driver | 工程量大 | 增强 |
| C. 仅文档标注默认用 socket | 无 API | **部分**（T1）；T2 不满足 |

本 residual **最低**：T1 文档 + rustdoc + example 打印 layer；**推荐完成** T2 选项 A。

```rust
// proposed
pub async fn open_engine_only_loopback(options: LoopbackOptions)
    -> Result<LoopbackPair, ConnectorError>;
// 或 topology/layer 参数进入 open_in_memory_loopback
```

文件头/测试：

```rust
/// LOOPBACK_LAYER = EngineOnlyBypassWire
/// BYPASS_WIRE = true
/// This does NOT satisfy protocol framing acceptance.
```

---

## 4. 文档要求

1. `open_in_memory_loopback` rustdoc 首段写明默认 **localhost ephemeral TCP**。  
2. 名称 `in_memory` 的历史包袱：文档解释 = “in-process / no external server”，≠ “no socket”。  
3. connector-gaps / example 输出 `layer=…`。  

---

## 5. 测试清单

| ID | 用例 | 期望 |
| --- | --- | --- |
| T-LB-01 | 默认 Cross Rtmp/HttpFlv | layer=ProtocolFraming；≥1 帧 |
| T-LB-02 | EngineOnly 路径 | layer=EngineOnlyBypassWire；≥1 帧；无 listen 或文档允许的最小绑定 |
| T-LB-03 | preferred 不支持 | typed 错误，不静默改 layer |
| T-LB-04 | WebRTC fixture | layer 正确；不称为 ProtocolFraming |
| T-LB-05 | rustdoc/example 含 localhost 说明 | 人工 |

验证“无 socket”（可选增强）：

```bash
# 仅当 T-LB-02 声称 zero socket 时
# 可用 strace/ss 抽查；或代码路径断言未 bind TCP
```

---

## 6. DoD（阶段 5）

- [ ] rustdoc 诚实描述默认 localhost  
- [ ] `LoopbackPair.layer` 准确  
- [ ] 至少一条 EngineOnly（或其它 socket-free）可测路径 **或** 产品明确拒绝并在 connector-gaps 关闭 T2  
- [ ] 既有 ProtocolFraming 测不回归  
- [ ] 无静默降级  

---

## 7. 衔接

- STREAM-01 主验收可继续用 ProtocolFraming。  
- 严格 CI 用 EngineOnly + 另跑一条 ProtocolFraming job。  
- R5 wait_ready 改善 ProtocolFraming flaky。  
