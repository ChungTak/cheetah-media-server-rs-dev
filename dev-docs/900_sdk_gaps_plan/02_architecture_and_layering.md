# 02 · 架构分层、Feature 矩阵与改/不改清单

> **Agent 用途**：决定“改哪一层、加什么 feature、什么绝对不能动”。  
> 权威：`AGENTS.md`、`SystemArchitecture.md`。  
> SDK 契约权威：`crates/sdk/cheetah-sdk`。Engine 权威：`crates/system/cheetah-engine`。

---

## 1. 分层职责

```text
┌──────────────────────────────────────────────────────────────────┐
│ 外部 integrator (dyun-gu-dev) / CI                               │
│  使用：cheetah_connector::{RuntimeConnector, open_in_memory_…}   │
└───────────────────────────────┬──────────────────────────────────┘
                                │
┌───────────────────────────────▼──────────────────────────────────┐
│ crates/sdk/cheetah-connector          【新建 · 本方案主落点】      │
│  - features: rtsp, http-flv, rtmp, webrtc, loopback              │
│  - RuntimeConnector / EngineConnector / Loopback / Error         │
│  - 组合 Engine + ModuleFactory + protocol client APIs            │
└───────┬─────────────────┬─────────────────┬──────────────────────┘
        │                 │                 │
        ▼                 ▼                 ▼
┌───────────────┐ ┌──────────────┐ ┌──────────────────────────────┐
│ cheetah-engine│ │ protocol     │ │ cheetah-sdk（契约 only）       │
│ 组装/分发     │ │ modules +    │ │ PublisherSink/SubscriberSource│
│               │ │ drivers      │ │ SdkError / RuntimeApi re-export│
└───────┬───────┘ └──────┬───────┘ └──────────────▲───────────────┘
        │                │                        │
        └────────────────┴────────────────────────┘
                         │
              ┌──────────▼──────────┐
              │ cheetah-codec       │
              │ AVFrame / TrackInfo │
              └─────────────────────┘
                         │
              ┌──────────▼──────────┐
              │ runtime-api / tokio │
              └─────────────────────┘
```

### 1.1 依赖方向（强制）

| From → To | 允许？ | 说明 |
| --- | --- | --- |
| connector → sdk / engine / codec / runtime | **是** | 组合层 |
| connector → `*-module` / `*-driver-tokio` | **是**（feature 门控） | 协议能力 |
| sdk → connector | **否** | 避免反向依赖 |
| sdk → 具体 protocol module | **否** | `AGENTS.md` |
| protocol-core → tokio / socket / EngineContext | **否** | Sans-I/O |
| module → `tokio::net` / `tokio::select!` | **否** | 走 RuntimeApi |

### 1.2 各 Gap 改动层

| 层 | Gap1 | Gap2 | Gap3 | Gap4 | Gap5 | Gap6 |
| --- | --- | --- | --- | --- | --- | --- |
| `cheetah-connector` | **新建主改** | **主改** | 包装 | 包装/API | **主改** | **主改** |
| `cheetah-http-flv-module` | 消费 | 可选 harness | **主改 streaming** | — | map 错误 | FLV→frame 字段 |
| `cheetah-rtmp-*` | 消费 client | loopback 一侧 | — | — | map 错误 | 字段 |
| `cheetah-rtsp-*` | 消费 client | loopback 一侧 | — | — | map 错误 | 字段 |
| `cheetah-webrtc-*` | 消费 driver | 可选 | — | **主改 harness** | map 错误 | 字段 |
| `cheetah-sdk` | **默认不改** 或极小 re-export 文档 | 不改 | 不改 | 不改 | **不替换 SdkError** | 不改 |
| `cheetah-codec` | 不改 | 不改 | 复用 demux/map | 不改 | 不改 | 字段权威 |
| `cheetah-engine` | 默认 bootstrap 使用 | 可起 embedded engine | 否 | 可 | 否 | smoke |
| protocol-core | **不改状态机职责** | 可加 test-only pure harness | 否 | 可 pure media fixture | 否 | 否 |
| `cheetah-server` | **不强制改** | 否 | 否 | 否 | 否 | 否 |

---

## 2. Crate 与 Feature 矩阵

### 2.1 新建 package（钉死名称）

| 项 | 值 |
| --- | --- |
| Package name | `cheetah-connector` |
| 目录 | `crates/sdk/cheetah-connector` |
| 对外 crate 名 | `cheetah_connector` |

若实现时发现与现有命名冲突，仅允许在 S0 阶段修订并同步全文，不得 silently 改名。

### 2.2 目标 `Cargo.toml`（proposed）

```toml
# crates/sdk/cheetah-connector/Cargo.toml
[package]
name = "cheetah-connector"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "High-level protocol connector facade for external Cheetah integrators"

[features]
default = []
# 协议门控
rtsp = ["dep:cheetah-rtsp-module", "dep:cheetah-rtsp-driver-tokio"]  # 实际依赖以实现时 rg 为准
http-flv = ["dep:cheetah-http-flv-module"]
rtmp = ["dep:cheetah-rtmp-module", "dep:cheetah-rtmp-driver-tokio"]
webrtc = ["dep:cheetah-webrtc-module", "dep:cheetah-webrtc-driver-tokio"]
# 测试/loopback 辅助（可依赖更多 test harness 代码）
loopback = ["rtmp", "http-flv"]  # 首版主 loopback 路径；可扩展
# 一键启用四方向 + loopback（CI 用）
full = ["rtsp", "http-flv", "rtmp", "webrtc", "loopback"]

[dependencies]
cheetah-sdk = { path = "../cheetah-sdk" }
cheetah-codec = { path = "../../foundation/cheetah-codec" }
cheetah-engine = { path = "../../system/cheetah-engine" }
cheetah-runtime-api = { path = "../../runtime/cheetah-runtime-api" }
cheetah-runtime-tokio = { path = "../../runtime/cheetah-runtime-tokio" }
cheetah-config = { path = "../../system/cheetah-config" }  # 若 bootstrap 需要
async-trait.workspace = true
thiserror.workspace = true
# optional protocol deps …
```

根 workspace：

```toml
# 根 Cargo.toml members 追加
"crates/sdk/cheetah-connector",
```

> **注意**：optional 依赖的 **真实 package path** 以实现时 `Cargo.toml` 为准；上表是形态，不是可粘贴即过的最终版本。S0 用 `cargo metadata` / 现有 module Cargo.toml 钉死。

### 2.3 Feature 语义

| Feature | 保证 | 典型用途 |
| --- | --- | --- |
| `rtsp` | 可 `open_pull(Protocol::Rtsp, …)` | RTSP 拉流 |
| `http-flv` | 可 `open_pull(Protocol::HttpFlv, …)` + streaming API | HTTP-FLV 拉流 |
| `rtmp` | 可 `open_push(Protocol::Rtmp, …)` | RTMP 推流 |
| `webrtc` | 可 `open_push(Protocol::WebRtc, …)` | WebRTC 推流 |
| `loopback` | `open_in_memory_loopback` 可用 | CI 无外部 peer |
| `full` | 上表全集 | 外部 SDK CI job |

### 2.4 Capability matrix（运行时 + 编译时）

```rust
// proposed
pub fn supports(protocol: Protocol, direction: Direction) -> bool {
    matches!(
        (protocol, direction),
        (Protocol::Rtsp, Direction::Pull)
            | (Protocol::HttpFlv, Direction::Pull)
            | (Protocol::Rtmp, Direction::Push)
            | (Protocol::WebRtc, Direction::Push)
    )
}
```

编译期：对应 feature 未启用时，`open_*` 返回 `UnsupportedProtocol` **或** `cfg` 隐藏实现（推荐：**API 始终存在**，feature 缺失时返回明确错误，避免 integrator 面对“方法不存在”）。

---

## 3. 默认 Engine / Module bootstrap

外部调用者不该复制 `cheetah-server` 的 200 行组装。connector 提供：

```rust
// proposed
pub struct ConnectorBuilder { /* runtime, config, features */ }

impl ConnectorBuilder {
    pub fn new(runtime: Arc<dyn RuntimeApi>) -> Self;
    pub fn with_config_provider(self, …) -> Self;
    /// 按已启用 feature 注册 ModuleFactory（对齐 cheetah-server）
    pub fn with_default_modules(self) -> Self;
    pub fn build(self) -> Result<EngineConnector, ConnectorError>;
}
```

规则：

1. 只注册 **已启用 feature** 对应 factory。  
2. 不默认监听生产端口；loopback/测试使用 ephemeral / in-memory。  
3. `Engine::start()` / `stop()` 生命周期由 connector 或 `ConnectorGuard` 管理，文档写明 drop 语义。  
4. 不绕过 `ModuleRestartRequired` 等引擎语义。

---

## 4. Handle 与既有契约关系

```text
PullHandle
  - 内部：Box<dyn SubscriberSource> 或协议专用 streaming 源
  - 对外：实现 SubscriberSource 或 Deref/as_source()
  - 额外：protocol(), url(), close(), tracks()（若 Gap6）

PushHandle
  - 内部：Box<dyn PublisherSink> + 协议 client 会话
  - 对外：实现 PublisherSink 或 as_sink()
  - 额外：protocol(), wait_ready(), close()
```

**禁止** 新造 `MediaFrame` 替代 `AVFrame`。

---

## 5. Loopback 分层模型

```text
L0  Engine-only smoke
    open_publisher → push_frame → open_subscriber → recv
    标注：BYPASS_WIRE=true；不得作为 Gap2 DoD

L1  Protocol framing loopback（目标主路径）
    协议 A push client  →  本进程 protocol server/module  →  协议 B pull
    或 同协议 pair（若协议支持）
    保留 framing / track negotiation / backpressure

L2  WebRTC special
    L2a signaling loopback（已有 InMemoryTransport 可复用）
    L2b media fixture / deterministic packet path
    L2c optional local UDP peer（CI 可选 job）
```

详见 [`04`](./04_loopback_transport.md)、[`06`](./06_webrtc_media_loopback.md)。

---

## 6. 改 / 不改清单

### 6.1 必须改（按阶段）

| 路径 | 动作 |
| --- | --- |
| `crates/sdk/cheetah-connector/**` | **新建** |
| 根 `Cargo.toml` | workspace member |
| `crates/protocols/http-flv/module/src/*` | Gap3 streaming |
| `crates/protocols/webrtc/module/...` | Gap4 harness（路径可新建子模块） |
| connector tests / example | 见 `09` |

### 6.2 默认不改

| 路径 | 原因 |
| --- | --- |
| `cheetah-sdk` 公共 `SdkError` 变体集 | 兼容；typed 错误放 connector |
| protocol-core 热路径状态机 | 不混写业务 facade |
| `apps/cheetah-server` | 非库消费者必须路径 |
| 全仓库把 `io::Error` 换成 `ConnectorError` | 过大；只在边界 map |
| 其它协议（HLS/TS/…）connector | 范围外 |

### 6.3 可选改

| 路径 | 何时 |
| --- | --- |
| 将 `HttpFlvPullError::retryable` 改为 `pub` | Gap3/5 需要时 |
| 协议 module 导出 test harness 模块 `#[cfg(test)]` 或 feature `test-utils` | loopback 需要共享 server 启动 |
| README 用户文档短节 | 阶段 7 |

---

## 7. Public API 稳定性原则

1. **Additive first**：新 crate 新 API；不破坏现有 module/driver 签名。  
2. `PublisherSink` / `SubscriberSource` 签名 **不改**（除非全仓库协调；本方案不依赖改签名）。  
3. one-shot `pull_http_flv_once` **保留**。  
4. connector 的 `ConnectorError` 可 `#[non_exhaustive]` 便于演进。  
5. 能力未实现时返回 typed `Unsupported*`，禁止 panic。  
6. 异步 API 使用 `async_trait` 或具体 Future，与邻近 sdk 风格一致；取消统一 `CancellationToken`。

---

## 8. 与 AGENTS.md 对齐检查表

实现 PR 自检：

- [ ] core 仍 Sans-I/O：无 socket、无 `async fn` 状态机核心、无 `Instant::now()`  
- [ ] module 无 `tokio::net` / `tokio::select!`  
- [ ] sdk 公共接口无 `tokio::*`  
- [ ] 无跨层偷依赖；connector 是明确组合层  
- [ ] 未在 module 复制 codec 时间戳/NALU/参数集逻辑  
- [ ] 热路径无无界队列；loopback/streaming 队列有上界  
- [ ] 新 public API 有 rustdoc + 测试  
- [ ] 测试区分 engine smoke vs protocol loopback  

---

## 9. 命名冻结表（S0 钉死）

| 概念 | 钉死名 |
| --- | --- |
| crate | `cheetah-connector` |
| trait | `RuntimeConnector` |
| 默认实现 | `EngineConnector` |
| builder | `ConnectorBuilder` |
| error | `ConnectorError` |
| protocol enum | `Protocol` |
| direction enum | `Direction` |
| pull/push handle | `PullHandle` / `PushHandle` |
| loopback | `open_in_memory_loopback` / `LoopbackPair` / `LoopbackOptions` |
| HTTP-FLV streaming | `open_http_flv_subscriber` / `HttpFlvSubscriberOptions` |
| WebRTC peer | `InMemoryWebRtcMediaPeer` 或 `open_webrtc_media_loopback` |
| example | `external_connector_loopback` |
| feature `full` | 四协议 + loopback |
