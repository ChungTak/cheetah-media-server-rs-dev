# 04 · R2：WebRTC Push Adapter 接线

> **Agent 用途**：阶段 3 主文档——实现真实 URL 的 `open_webrtc_push`。  
> **严禁**：用 media fixture loopback 或“仅 SDP 生成”冒充本 residual 完成。  
> **低层入口**：`spawn_driver`、module WHIP/publish 路径。

---

## 1. 目标 / 非目标

| 项 | 首版（P0） | 后置 |
| --- | --- | --- |
| API | `open_webrtc_push` → `PushHandle` | WHEP pull、simulcast |
| 信令 | WHIP（HTTP POST SDP）或文档约定 URL | 自定义 WS 信令 |
| Media | 可 `push_frame` / `update_tracks` 进入发送路径 | 完整 BWE/FEC |
| 就绪 | `wait_ready` 等待可推（R5） | — |

**非目标**：

- 删除或破坏既有 `MediaLoopbackHarness` / fixture loopback（plan1 Gap4 / connector-gaps 已可用）。  
- 浏览器 Playwright 作为本 residual 必选项。  
- 把 `open_in_memory_loopback(SameProtocol::WebRtc)` 改名为 `open_push`。

---

## 2. 现状与证据

```text
supports(WebRtc, Push) = true
open_push(WebRtc) → UnsupportedProtocol
push/ 无 webrtc.rs
fixture 仅经 loopback SameProtocol
```

```bash
rg -n 'spawn_driver|WHIP|WHEP|MediaLoopback|open_push' \
  crates/protocols/webrtc crates/sdk/cheetah-connector --glob '*.rs' | head -50
```

driver：

```text
crates/protocols/webrtc/driver-tokio/src/runner.rs
  pub async fn spawn_driver(config, cancel) -> io::Result<Arc<WebRtcDriverHandle>>
```

---

## 3. proposed API

```rust
// crates/sdk/cheetah-connector/src/push/webrtc.rs

pub async fn open_webrtc_push(
    ctx: &ConnectorContext,
    url: &str,
    options: ConnectorPushOptions,
) -> Result<PushHandle, ConnectorError>;
```

`EngineConnector::open_push`：

```rust
Protocol::WebRtc => open_webrtc_push(self, url, options).await  // feature 门控
```

### 3.1 URL 约定（钉死并文档化）

首版推荐一种，实现前与 module 路由对齐：

| 方案 | 示例 | 说明 |
| --- | --- | --- |
| **A（推荐）** | `http(s)://host/whip/app/stream` | 标准 WHIP POST |
| B | `webrtc+whip://host/path` | 需自定义解析 |

非法 → `InvalidUrl { protocol: WebRtc, … }`。

### 3.2 PushHandle 行为

| 方法 | 行为 |
| --- | --- |
| `update_tracks` | 设置/更新 TrackInfo（含 extradata） |
| `push_frame` | 送入 packetize/发送路径；未 ready 时背压或错误（文档钉死） |
| `take_keyframe_requests` | 透传 PLI/FIR 计数 |
| `wait_ready` | 等待 ICE/DTLS/信令完成至可发媒体（R5） |
| `close` | 结束会话 |

实现 `PublisherSink`。

---

## 4. 实现步骤

### 4.1 组装路径（优先 module 编排）

```text
1. 解析 WHIP URL + options.tracks
2. 确保 engine 已注册 WebRtcModuleFactory（ConnectorBuilder）
3. 创建/绑定发布会话（module HTTP WHIP 或 client 侧 WHIP）
4. 将 PushHandle 接到协议 publish sink
5. wait_ready ← 信令 answer + 传输连通
6. push_frame → RTP/SRTP 发送
```

若存在“向本进程 WHIP 发布”的集成测 harness，优先复用（localhost），避免外部 peer。

### 4.2 与 fixture 的关系

| 路径 | API | 层标注 |
| --- | --- | --- |
| 真实 push | `open_push(WebRtc, url)` | production / localhost integration |
| fixture | `open_in_memory_loopback(SameProtocol{WebRtc})` | `WebRtcMediaFixture` |

两者并存；测试文件名不得混淆。

### 4.3 分层验证（验收）

| 层 | 内容 | 算 R2 done？ |
| --- | --- | --- |
| S | WHIP offer/answer 成功 | **否**（单独测） |
| M | `push_frame` 后对端或 loopback peer 收到媒体语义 | **是（最低）** |
| F | 仅 fixture 无 open_push | **否** |

最低 DoD：在 **本进程** 内完成 open_push → wait_ready → push 至少 1 关键帧，并由对端接收路径（本机 WHEP/subscriber 或受控 peer）验证；若对端暂不可用，可用 **标注的** local integration harness，但必须经过 `open_webrtc_push` 代码路径。

### 4.4 错误映射

| 场景 | ConnectorError |
| --- | --- |
| 坏 URL | `InvalidUrl` |
| 信令 HTTP 失败 | `Protocol { Negotiate }` / `Connect` |
| ICE 失败 | `Connect` |
| 未 ready 强推（若策略为错误） | `Protocol { Publish }` 或 `Backpressure` |

---

## 5. 测试清单

| ID | 用例 | 期望 |
| --- | --- | --- |
| T-WR-01 | 非法 URL | `InvalidUrl` |
| T-WR-02 | feature 关闭 | `FeatureDisabled` |
| T-WR-03 | open_push 返回可用 PushHandle | 非 UnsupportedProtocol |
| T-WR-04 | wait_ready 完成（超时失败可测） | 非立即 Ok 空转（除非已 ready） |
| T-WR-05 | push 1 keyframe 媒体可达 | 对端/harness 断言 |
| T-WR-06 | update_tracks + extradata | 不丢 codec |
| T-WR-07 | close 清理 | 无任务泄漏 |
| T-WR-08 | signaling-only 测 **不** 命名 media_roundtrip | 审查 |
| T-WR-09 | supports 一致 | R3 |

---

## 6. DoD（阶段 3）

- [ ] `open_webrtc_push` + `open_push(WebRtc)` 接线  
- [ ] `PushHandle` 可 `push_frame`  
- [ ] 至少一条媒体路径测（非纯 SDP）  
- [ ] fixture loopback 仍绿（勿回归）  
- [ ] `supports(WebRtc, Push)` 诚实  
- [ ] `cargo test -p cheetah-connector --features webrtc,full`  

---

## 7. 与其它 R 的衔接

| R | 关系 |
| --- | --- |
| R3 | 接线后 supports true |
| R5 | wait_ready 必须接 WebRTC 就绪 |
| R6 | fixture 仍是 socket-light 路径；真实 push 可能用 UDP |
| R8 | 错误协议=WebRtc |

---

## 8. 风险与回退

| 风险 | 回退 |
| --- | --- |
| 完整 ICE/DTLS 难测 | localhost 双端 + 长 timeout；或可控 test peer |
| spawn_driver 强依赖 UDP | CI 允许 loopback UDP；文档注明 |
| 工期爆炸 | MVP：WHIP + 单轨 H264；高级特性后置 |

**禁止回退**：为赶工把 `open_push` 内部重定向到 media fixture 却不文档化。若临时如此，必须 `LoopbackLayer`/`TransportMode` 显式且 `supports` 语义单独讨论。  
