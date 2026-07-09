# F-04：webrtc module 去 tokio 化（runtime 中立化）实施计划

> 关联审查发现：`dev-docs/ProjectReviewReport.md` F-04。目标：让 `cheetah-webrtc-module`
> 生产代码不再直接依赖 `tokio::{net,time,sync}`、`tokio::select!`、`tokio::spawn`，
> 网络/TLS/WebSocket I/O 下沉到 `cheetah-webrtc-driver-tokio`，统一走 `RuntimeApi` / SDK 抽象
> （依据 `AGENTS.md` §5/§6）。
>
> 这是全仓最大的单项架构改造，必须分阶段、可编译、可测试地推进；本文件是执行路线图。

---

## 1. 准确范围（本轮实测）

对 `crates/protocols/webrtc/module/src` 全量扫描并区分 `#[cfg(test)]`：
**生产代码 78 处、测试代码 53 处** 使用 tokio 原语。生产用法按“可机械替换(A)”与“需结构性下沉(B)”分类：

| 文件 | 生产用法 | 类别 | 说明 |
|------|---------|------|------|
| `ome_ws.rs` | 12 | **B** | OME WHIP/WHEP WebSocket 服务：`TcpListener` + `tokio_tungstenite::accept_hdr_async` + `spawn` + `select` + `time::timeout` |
| `p2p/ws.rs` | 9 | **B** | P2P WebSocket 客户端：`tokio_tungstenite::connect_async` + `MaybeTlsStream` + `time::timeout` |
| `p2p/bridge.rs` | 8 | A(+计时) | `oneshot`/`mpsc` 通道 + `select!` + `time::timeout`（`BridgeLifecycleSource::subscribe` 返回类型泄漏 tokio） |
| `p2p/supervisor.rs` | 7 | A | `select!` + `time::sleep` + `spawn` |
| `http_client.rs` | 6 | **B** | 裸 HTTP/1.1 客户端：`tokio::io` + `TcpStream` + `tokio_rustls::TlsConnector` + `lookup_host` |
| `module.rs` | 6 | A+B | `TcpListener::bind`(B) + `time::interval`/`select!`(A) |
| `http.rs` | 5 | A(+broadcast) | `broadcast`/`oneshot` + `time::timeout` + `spawn` |
| `jobs.rs` | 5 | A | `select!` + `time::sleep` + `oneshot` + `time::timeout` |
| `p2p/transport.rs` | 5 | A | 内存 transport（测试脚手架）：`mpsc` + async `Mutex` |
| `bridge.rs` | 4 | A | `time::sleep` + `select!` |
| `p2p/hub.rs` | 4 | A | `mpsc` + async `Mutex` + `select!` |
| `p2p/server.rs` | 4 | **B** | P2P WebSocket 服务：`TcpListener` + `accept_async` + `spawn` + `time::timeout` |
| `p2p/lifecycle_dispatcher.rs` | 3 | A | `mpsc` 通道（trait 返回类型泄漏 tokio） |

---

## 2. 可用抽象与缺口

`cheetah-runtime-api::RuntimeApi` 提供：`now / spawn / spawn_local / sleep_until / bind_udp /
connect_tcp / bind_tcp / wrap_*  / oneshot`，以及 `CancellationToken`（SDK）。`futures` 已是依赖，
可用 `futures::channel::{mpsc,oneshot}`、`futures::lock::Mutex`、`futures::select_biased!`。

**关键缺口（决定分阶段方式）：**
1. **计时**：`tokio::time::{sleep,timeout,interval}` 无 futures 等价物；只能 (a) 把 `RuntimeApi` 句柄
   注入到各调用点用 `sleep_until` 组合超时，或 (b) 引入 runtime-neutral 计时库。多数相关函数当前**未持有**
   `RuntimeApi`，因此需先做“句柄注入”准备工作（Stage 2）。
2. **broadcast**：`http.rs` 用 `tokio::sync::broadcast`，futures 无对应；需选型（`async-broadcast` crate /
   SDK 抽象 / 下沉 driver）。
3. **WebSocket / TLS / socket**：`tokio-tungstenite`、`tokio-rustls`、`tokio::net` 本质绑定 tokio，
   **不能就地替换**，必须迁入 `cheetah-webrtc-driver-tokio` 并以中立 trait（参照现有 `P2pTransport`）暴露。
4. **spawn**：`tokio::spawn` → `RuntimeApi::spawn`，同样依赖句柄注入。

---

## 3. 分阶段计划

### Stage 1（本 PR）— 打样：自足文件机械替换 ✅
- `p2p/transport.rs`：`tokio::sync::mpsc` → `futures::channel::mpsc`；`tokio::sync::Mutex` →
  `futures::lock::Mutex`；`recv()`→`next()`；`send()` 经 `sender.clone().send()`（futures `Sender::send`
  需 `&mut self`）。该文件仅被测试消费，public trait `P2pTransport` 本就中立，零外溢。
- 验证：`cargo build/clippy/test -p cheetah-webrtc-module`（transport 3 测试通过）。
- 产出可复用的“通道/锁”替换配方（见 §4）。

### Stage 2 — 注入 runtime 句柄（准备，无行为变化）
- 沿 `Mp4/WebRtc` module → bridge/job/http 调用链传入 `Arc<dyn RuntimeApi>`（或经 `EngineContext`
  暴露的运行时句柄），为 Stage 3 的计时/spawn 替换提供注入点。
- 仅改签名与构造，不改逻辑；逐 crate 编译。

### Stage 3 — 机械替换 A 类（依赖 Stage 2 句柄）
- 通道：`tokio::sync::mpsc/oneshot` → `futures::channel::*`；`try_send`/`send`/`recv` 按配方改写。
- 锁：跨 await 的 `tokio::sync::Mutex` → `futures::lock::Mutex`；不跨 await 的改 `parking_lot`。
- 多路等待：`tokio::select!` → `futures::select_biased!`（配 `.fuse()` / `FusedFuture`）。
- 计时：`sleep/interval/timeout` → `RuntimeApi::sleep_until` + `select_biased!` 组合。
- 任务：`tokio::spawn` → `RuntimeApi::spawn`。
- 涉及文件：`lifecycle_dispatcher.rs`、`p2p/bridge.rs`、`bridge.rs`、`p2p/hub.rs`、
  `p2p/supervisor.rs`、`jobs.rs`、`module.rs`(计时/`select` 部分)。
- 同步修正 `BridgeLifecycleSource::subscribe`、`P2pOfferWaiter` 等 trait 中的 tokio 返回/参数类型。

### Stage 4 — 处理 broadcast + 收尾 http.rs A 类 ✅
- `http.rs` 的 `tokio::sync::broadcast`：该 `AnswerDispatcher::diagnostics` 字段实为
  `#[allow(dead_code)]` 的 shape-only 占位（从未订阅/发布）。按最小改动原则**直接移除**，
  不为死代码引入 `async-broadcast` 依赖；未来真正接入 metrics worker 时再选型 runtime-neutral 广播。
- 顺带收尾 `http.rs` 仅剩的两处 A 类 `tokio::time::timeout`（均为等待驱动回填的 SDP oneshot，
  纯 module 逻辑、非 I/O）：
  - `WebRtcHttpService::wait_answer`（WHIP/WHEP）：经 `self.engine.runtime_api` 用
    `RuntimeApi::sleep_until` + `select_biased!` 定界。
  - OME WS offer 等待：新增 `OmeAnswerWaiter { dispatcher, runtime }` 承载 runtime 句柄，
    取代 `impl OmeWsOfferWaiter for Arc<AnswerDispatcher>`；在 `run_ome_ws_connection`
    以 `engine.runtime_api` 构造。两者共用 `await_answer_with_timeout` 助手。
- **结果**：webrtc module 生产代码已**无 A 类 tokio 原语**；剩余 tokio 命中仅为 B 类
  （`ome_ws.rs`/`p2p/ws.rs`/`p2p/server.rs`/`http_client.rs` 的 WS/TLS/socket）与测试代码。

### Stage 5 — 结构性下沉 B 类（最大工作量）✅
- **驱动层新增中立 WebSocket 抽象**（`driver-tokio/src/ws.rs`）：
  - 帧/错误：`WsFrame{Text/Binary/Closed}`、`WsError{Closed/Protocol/ConnectTimeout/InvalidRequest}`。
  - 客户端：`trait WsConnection{send_text/recv/close}` + `trait WsConnector{connect(url,timeout)}`；
    tokio 实现 `TokioWsConnection`（`connect_async`/`accept_async` 两种上游都可包装）、`TokioWsConnector`。
  - 服务端：`bind_ws_server(addr) -> (WsServerListener, SocketAddr)` + `WsServerListener::serve(cfg, handler, cancel)`；
    accept 循环、握手超时、ping/pong、容量背压、连接计数全部落在 driver。`WsInbound{remote_addr, path_and_query}`
    把握手期需要的路径信息回传给上层 handler。
  - `tokio-tungstenite` / `tokio-rustls` / `TcpListener` 实现全部收敛到 driver。
- **裸 HTTP/1.1+TLS 客户端**（`http_client.rs`）经 `git mv` 迁入 driver（`WhipWhepHttpClient` 等），
  module 侧以 `pub use cheetah_webrtc_driver_tokio::http_client;` 保持旧路径（fuzz harness 兼容）。
- **module 变薄封装**：`p2p/ws.rs`（`WebSocketP2pTransport` 包 `Box<dyn WsConnection>`、
  `WebSocketTransportFactory` 持 `Arc<dyn WsConnector>`）、`p2p/server.rs`（`run_server` 收 `WsServerListener`
  并委派 `serve`）、`ome_ws.rs`（`run_ome_ws_server` 收 `WsServerListener`）、`module.rs`（改用
  `bind_ws_server` 取代 `TcpListener::bind`）。**消息编解码（OmeWsMessage/P2pMessage）与 SSRF/URL 策略仍留在 module。**
- **依赖收敛**：`module/Cargo.toml` 从 `[dependencies]` 删除 `tokio` / `tokio-rustls` / `tokio-tungstenite`
  / `rustls` / `rustls-pemfile` / `webpki-roots`；测试用的 `tokio` / `tokio-tungstenite` 降级为 `[dev-dependencies]`。
- **结果**：webrtc module 生产代码已**无 A 类与 B 类 tokio 原语**（`[dependencies]` 无 tokio，
  生产代码无法再编译进 `tokio::*`）；剩余 tokio 命中仅存在于 `#[cfg(test)]` 与 `tests/`。

### Stage 6 — 固化守卫 ✅（随 Stage 5 一并落地）
- 扩展 `dev-scripts/check_runtime_boundaries.sh`：
  - `cheetah-webrtc-module/src` 纳入 public-API 中立扫描；`cheetah-webrtc-driver-tokio/src` 纳入 driver 中立扫描。
  - 新增**清单级**守卫：断言 `module/Cargo.toml` 的 `[dependencies]` 不含
    `tokio` / `tokio-util` / `tokio-rustls` / `tokio-tungstenite`。相比源码 grep，这直接保证生产代码
    无法依赖 tokio，且不会被内联 `#[cfg(test)]` 单测误伤。

---

## 4. 通道/锁替换配方（Stage 1 定型，供后续阶段复用）

```rust
// before (tokio)
use tokio::sync::{mpsc, Mutex};
let (tx, rx) = mpsc::channel(cap);
sender.send(x).await;          // Sender::send(&self)
guard.recv().await;            // Receiver::recv(&mut self) -> Option
recorder.lock().await;         // tokio async Mutex

// after (runtime-neutral)
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{SinkExt, StreamExt};
let (tx, rx) = mpsc::channel(cap);
sender.clone().send(x).await;  // futures Sender::send(&mut self) -> 用 clone 保留 &self 语义
guard.next().await;            // StreamExt::next -> Option
recorder.lock().await;         // futures::lock::Mutex 保留 .await API
// try_send: futures Sender::try_send(&mut self, x) —— 绑定需 `mut`
```

要点：futures `mpsc::Sender` 是 `Clone`，`send/try_send` 取 `&mut self`；在 `&self` 方法里用
`self.tx.clone().send(..)`。`Receiver` 通过 `StreamExt::next()` 取值。async 锁用 `futures::lock::Mutex`
以保留 `.lock().await`。

---

## 5. 每阶段验证

```bash
cargo fmt -p cheetah-webrtc-module
cargo clippy -p cheetah-webrtc-module
cargo test  -p cheetah-webrtc-module
# Stage 5/6 后：
bash dev-scripts/check_runtime_boundaries.sh
```

## 6. 风险与原则
- 每阶段保持可编译、测试全绿；不一次性大改动 signaling 热路径。
- 不为通过检查而弱化约束；计时/广播缺口以正规抽象解决，不用 hack。
- Stage 5 触及 driver 公共 API 与 module 依赖，属对外边界变更，需同步 `SystemArchitecture.md`
  （另见 F-07：webrtc 尚未在架构文档登记）。
