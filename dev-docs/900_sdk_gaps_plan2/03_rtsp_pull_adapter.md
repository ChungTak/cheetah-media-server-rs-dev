# 03 · R1：RTSP Pull Adapter 接线

> **Agent 用途**：阶段 2 主文档——实现 `open_rtsp_pull` 并接入 `open_pull`。  
> **样板**：既有 `open_http_flv_pull`（`pull/http_flv.rs`）的 streaming 生命周期。  
> **低层入口**：`cheetah_rtsp_driver_tokio::start_tcp_client`（包名以 Cargo.toml 为准）。

---

## 1. 目标 / 非目标

| 项 | 首版（P0） | 后置 |
| --- | --- | --- |
| API | `open_rtsp_pull` → `PullHandle` | UDP RTP 精细调优 |
| 传输 | TCP RTSP（`start_tcp_client`） | HTTP tunnel / TLS 变体 |
| 生命周期 | recv / cancel / close / bounded queue | 完整 reconnect 策略对齐 HTTP-FLV |
| 输出 | `AVFrame` + `tracks()` | 全 codec 矩阵 |

**非目标**：重写 RTSP core；在 connector 内实现 RTP 解包（应复用 driver/module/codec）。

---

## 2. 现状与证据

```text
supports(Rtsp, Pull) = true
open_pull(Rtsp) → UnsupportedProtocol
pull/ 无 rtsp.rs
```

```bash
rg -n 'start_tcp_client|RtspClientHandle|RtspClientEvent' \
  crates/protocols/rtsp --glob '*.rs' | head -40
rg -n 'open_http_flv_pull|open_pull' crates/sdk/cheetah-connector --glob '*.rs'
```

---

## 3. proposed API

```rust
// crates/sdk/cheetah-connector/src/pull/rtsp.rs

/// Open a long-lived RTSP pull as PullHandle (SubscriberSource).
pub async fn open_rtsp_pull(
    ctx: &ConnectorContext, // EngineConnector 内部上下文：runtime、cancel root 等
    url: &str,
    options: ConnectorPullOptions,
) -> Result<PullHandle, ConnectorError>;
```

`EngineConnector::open_pull`：

```rust
Protocol::Rtsp => {
    #[cfg(feature = "rtsp")]
    { open_rtsp_pull(self, url, options).await }
    #[cfg(not(feature = "rtsp"))]
    { Err(ConnectorError::FeatureDisabled { … }) }
}
```

### 3.1 选项

复用/扩展 `ConnectorPullOptions`：

| 字段 | 用途 |
| --- | --- |
| `subscriber.queue_capacity` | 帧队列上界（R4） |
| `subscriber.backpressure` / media_filter | 若可映射则映射 |
| `cancel` | 取消 |
| `protocol: Rtsp(RtspPullExtras)` | transport、凭据、超时等（可选） |

```rust
// proposed
pub struct RtspPullExtras {
    pub connect_timeout: Option<Duration>,
    // 其它与 RtspClientConfig 对齐的字段；禁止布尔位置参数堆叠
}
```

### 3.2 PullHandle 行为

| 方法 | 行为 |
| --- | --- |
| `recv` | 异步取 `Arc<AVFrame>`；EOF/关闭 → `Ok(None)` 或 typed Closed |
| `close` | 幂等；停止 client 任务 |
| `tracks` | DESCRIBE/SETUP 后可见的 TrackInfo 快照 |
| `id` | SubscriberId |

实现 `SubscriberSource`（与 HTTP-FLV handle 一致）。

---

## 4. 实现步骤

### 4.1 URL 解析

- 支持 `rtsp://host[:port]/path`（默认端口 554）。  
- 失败 → `ConnectorError::InvalidUrl { protocol: Rtsp, url }`。  
- **复用** RTSP 模块/驱动已有 URL 解析；禁止分叉解析。

```bash
rg -n 'rtsp://|parse.*Rtsp|RtspUrl' crates/protocols/rtsp crates/sdk/cheetah-connector --glob '*.rs' | head
```

### 4.2 连接与会话

1. DNS / `SocketAddr`（注意 async 边界，走 runtime 能力）。  
2. `start_tcp_client(runtime, peer, config, cancel)`。  
3. 发送 DESCRIBE / SETUP / PLAY（经 `RtspClientHandle` command API——以源码为准）。  
4. 订阅媒体事件 → 转为 `AVFrame`。

### 4.3 帧路径（关键）

优先顺序：

1. **若 module 已有“拉流进 engine 再 subscribe”路径**：connector 编排 module + `StreamManagerApi::open_subscriber`。  
2. **否则**：在 adapter 内将 client events/payload 映射为 `AVFrame`，经有界 channel 送出。  

映射必须走 `cheetah-codec`（H264/H265/AAC/…），**禁止** connector 私有 NALU 解析。

### 4.4 有界队列与取消

- 容量：`options.subscriber.queue_capacity`（默认对齐 sdk 150 或 HTTP-FLV 既有默认）。  
- 满：策略文档化（DropOldest / DropUntilKeyframe）；禁止无界堆积。  
- `cancel` / `close` / `Drop`：停止 IO 任务，channel close，`recv` 终态明确。

### 4.5 重连

首版：可 `reconnect: None` 默认；若 HTTP-FLV 已有 `ReconnectPolicy`，可同源 options 透传（R4）。  
DoD 最低：cancel/close 可测；reconnect 有则测有限次。

### 4.6 错误映射

| 来源 | ConnectorError |
| --- | --- |
| 坏 URL | `InvalidUrl` |
| 连接失败 | `Connect { protocol: Rtsp, … }` |
| 协议拒绝 | `Protocol { operation: Play/Handshake, … }` |
| cancel | `Closed { Cancelled }` |

使用带协议上下文的 map helper（R8）。

---

## 5. 测试清单

| ID | 用例 | 期望 |
| --- | --- | --- |
| T-RTSP-01 | 非法 URL | `InvalidUrl` |
| T-RTSP-02 | feature 关闭 | `FeatureDisabled` |
| T-RTSP-03 | localhost RTSP server 或 harness 拉 1 帧 | `recv` 非空 |
| T-RTSP-04 | cancel 后 recv 终态 | 不挂死 |
| T-RTSP-05 | close 幂等 | Ok |
| T-RTSP-06 | queue_capacity 小 | 有界不 OOM |
| T-RTSP-07 | `supports(Rtsp,Pull)` 与 open 一致 | R3 |
| T-RTSP-08 | tracks 非空（若媒体含轨道） | codec ≠ Unknown（合理时） |

集成 server：复用 `rtsp/module/tests` harness；ephemeral `127.0.0.1:0`。

---

## 6. DoD（阶段 2）

- [ ] `open_rtsp_pull` 存在且 `open_pull(Rtsp)` 调用  
- [ ] 至少一条集成测拿到 `AVFrame`（或诚实标注 fixture 层）  
- [ ] cancel/close/bounded queue 测过  
- [ ] 不再对已启用 feature 返回“永远 UnsupportedProtocol”  
- [ ] `supports` 同步更新（R3）  
- [ ] `cargo test -p cheetah-connector --features rtsp,full` 相关绿  

---

## 7. 与其它 R 的衔接

| R | 关系 |
| --- | --- |
| R3 | 接线后 `supports=true`；接线前可 false |
| R4 | queue/reconnect 从 options 来 |
| R6 | RTSP 可作未来 loopback 一侧；非本阶段必须 |
| R8 | 错误带 `Protocol::Rtsp` |

---

## 8. 风险

| 风险 | 缓解 |
| --- | --- |
| Client 只有 raw events 无 AVFrame | 走 module+engine 路径 |
| UDP RTP 使 CI 复杂 | 首版强制 TCP interleaved 若支持 |
| 鉴权/摘要 | 后置；坏凭据 typed 错误即可 |
