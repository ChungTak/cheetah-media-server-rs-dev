# 03 · Gap 1：可安装高层 Connector / Facade

> **Agent 用途**：阶段 1 主文档——新建 `cheetah-connector` 与 `RuntimeConnector`。  
> **组装样板**：`apps/cheetah-server/src/main.rs`（**不要**复制业务逻辑，只对齐 module 注册模式）。  
> **契约样板**：`crates/sdk/cheetah-sdk/src/stream.rs`。

---

## 1. 目标

| 项 | 首版（P0） | 可选后置 |
| --- | --- | --- |
| 安装面 | `cheetah-connector` + features | 顶层 `cheetah` 元 crate 再导出 |
| API | `RuntimeConnector` + `EngineConnector` | 自定义 connector 实现 |
| 方向 | RTSP/HTTP-FLV pull；RTMP/WebRTC push | 反向能力、多 URL 批处理 |
| 生命周期 | builder → open → close/drop | 自动重连策略可配置（HTTP-FLV 在 Gap3） |
| 依赖 | feature 门控协议 module | — |

Package：**`cheetah-connector`**  
Trait：**`RuntimeConnector`**  
默认实现：**`EngineConnector`**

---

## 2. 模块布局

```text
crates/sdk/cheetah-connector/
  Cargo.toml
  src/
    lib.rs                 # 模块导出 + crate 级 rustdoc
    protocol.rs            # Protocol, Direction, supports()
    error.rs               # ConnectorError（可与 Gap5 同文件演进）
    options.rs             # 选项类型；复用或包装 sdk Subscriber/PublisherOptions
    handles.rs             # PullHandle, PushHandle
    connector.rs           # RuntimeConnector trait + EngineConnector
    engine_bootstrap.rs    # ConnectorBuilder / 默认 module 注册
    pull/                  # 可选：按协议拆分
      mod.rs
      rtsp.rs
      http_flv.rs
    push/
      mod.rs
      rtmp.rs
      webrtc.rs
  tests/
    capability_matrix.rs
  examples/
    external_connector_loopback.rs   # 阶段 7 可先 stub
```

单文件若超过 ~500 行，按协议拆 `pull/` `push/`（`AGENTS.md` 模块大小约定）。

---

## 3. Public API 规范（proposed）

### 3.1 Protocol / Direction

```rust
// src/protocol.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Rtsp,
    HttpFlv,
    Rtmp,
    WebRtc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Pull,
    Push,
}

/// Returns whether the first-party capability matrix allows this pair.
pub fn supports(protocol: Protocol, direction: Direction) -> bool { /* 见 02 */ }

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rtsp => "rtsp",
            Self::HttpFlv => "http-flv",
            Self::Rtmp => "rtmp",
            Self::WebRtc => "webrtc",
        }
    }
}
```

### 3.2 Options

优先 **复用** `cheetah_sdk::{SubscriberOptions, PublisherOptions}`，并在 connector 层增加协议扩展字段：

```rust
// proposed
#[derive(Debug, Clone)]
pub struct ConnectorPullOptions {
    pub subscriber: cheetah_sdk::SubscriberOptions,
    pub cancel: Option<CancellationToken>,
    /// 协议扩展（headers、transport preference、auth 等）
    pub protocol: ProtocolPullExtras,
}

#[derive(Debug, Clone, Default)]
pub enum ProtocolPullExtras {
    #[default]
    None,
    Rtsp(RtspPullExtras),
    HttpFlv(HttpFlvSubscriberOptions), // 见 05
}

#[derive(Debug, Clone)]
pub struct ConnectorPushOptions {
    pub publisher: cheetah_sdk::PublisherOptions,
    pub cancel: Option<CancellationToken>,
    pub tracks: Vec<TrackInfo>, // push 前已知轨道；WebRTC/RTMP 可能需要
    pub protocol: ProtocolPushExtras,
}
```

**禁止** 布尔位置参数堆叠；扩展用 enum/struct（`AGENTS.md`）。

### 3.3 Handles

```rust
// proposed
pub struct PullHandle {
    protocol: Protocol,
    // 内部 source + 可选会话 guard
}

impl PullHandle {
    pub fn protocol(&self) -> Protocol;
    pub fn id(&self) -> SubscriberId;
    /// 若已发现 tracks，返回快照；未就绪可 Ok(None) 或空 Vec（文档钉死一种）
    pub fn tracks(&self) -> Vec<TrackInfo>;
    pub async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, ConnectorError>;
    pub async fn close(&mut self) -> Result<(), ConnectorError>;
}

// 可选：实现 From/Into 或 as_subscriber()
// 若实现 SubscriberSource，错误类型需 map SdkError → ConnectorError
```

```rust
pub struct PushHandle {
    protocol: Protocol,
}

impl PushHandle {
    pub fn protocol(&self) -> Protocol;
    pub fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), ConnectorError>;
    pub fn push_frame(&self, frame: Arc<AVFrame>) -> Result<DispatchResult, ConnectorError>;
    pub fn take_keyframe_requests(&self) -> u64;
    pub fn close(&self) -> Result<(), ConnectorError>;
    /// 等待协议会话 ready（连接完成、发布许可等）
    pub async fn wait_ready(&self) -> Result<(), ConnectorError>;
}
```

### 3.4 RuntimeConnector

```rust
// proposed
#[async_trait]
pub trait RuntimeConnector: Send + Sync {
    async fn open_pull(
        &self,
        protocol: Protocol,
        url: &str,
        options: ConnectorPullOptions,
    ) -> Result<PullHandle, ConnectorError>;

    async fn open_push(
        &self,
        protocol: Protocol,
        url: &str,
        options: ConnectorPushOptions,
    ) -> Result<PushHandle, ConnectorError>;
}
```

> gaps.md 中的同步 `fn open_pull` 可改为 async——本仓库全异步路径更自然；若坚持 sync 返回 handle 而内部 spawn，须文档说明。**推荐 async**。

### 3.5 EngineConnector + Builder

```rust
pub struct EngineConnector {
    engine: Arc<Engine>, // 或持有 Engine + runtime
    runtime: Arc<dyn RuntimeApi>,
    // cancel root, etc.
}

pub struct ConnectorBuilder { /* … */ }

impl ConnectorBuilder {
    pub fn new(runtime: Arc<dyn RuntimeApi>) -> Self;
    pub fn with_default_modules(self) -> Self;
    pub fn with_config(self, config: Arc<dyn ConfigProvider>) -> Self;
    pub async fn build(self) -> Result<EngineConnector, ConnectorError>;
}

impl RuntimeConnector for EngineConnector { /* … */ }

impl EngineConnector {
    pub async fn shutdown(self) -> Result<(), ConnectorError>;
}
```

#### 默认 module 注册对照

对齐 `apps/cheetah-server/src/main.rs` 中 `#[cfg(feature = …)]` 注册：

| connector feature | factory（名称以源码为准） |
| --- | --- |
| `rtsp` | `RtspModuleFactory` |
| `http-flv` | `HttpFlvModuleFactory` |
| `rtmp` | `RtmpModuleFactory` |
| `webrtc` | `WebRtcModuleFactory` |

---

## 4. URL 与路由规则

| Protocol | 接受的 URL 形态（首版） | 解析失败 |
| --- | --- | --- |
| RTSP | `rtsp://host[:port]/path` | `InvalidUrl` |
| HTTP-FLV | `http(s)://…` 或 `ws(s)://…` | `InvalidUrl` / `UnsupportedScheme` |
| RTMP | `rtmp://…` / `rtmps://…`（与现有 `RtmpUrl` 对齐） | `InvalidUrl` |
| WebRTC | 文档定义：WHIP URL 或 `webrtc+whip://` 约定 | `InvalidUrl` |

实现前读取现有 URL 解析类型：

```bash
rg -n 'struct RtmpUrl|parse.*url|SourceUrl' crates/protocols --glob '*.rs' | head -40
```

**不要** 自己发明与 driver 不一致的解析逻辑；调用既有 parser。

---

## 5. 各协议 open 流程（实现要点）

### 5.1 RTSP pull

1. 解析 URL → host/port/path。  
2. 需要时 DNS（通过 runtime 或 std，注意 async 边界）。  
3. 调用 `start_tcp_client`（或 module 级更高 API，若有）。  
4. 将会话事件/帧路径适配为 `PullHandle`：优先走 module 已有 “拉流进 engine 再 subscribe” 路径；若仅有 client handle，则在 connector 内适配 **直到** 能产出 `AVFrame`。  
5. 取消：options.cancel 或 handle.close。

> 若 RTSP 现状是 client handle 而不是 `SubscriberSource`，connector 的核心工作是 **适配器**，不是重写 RTSP。

### 5.2 HTTP-FLV pull

- **依赖 Gap 3** 的 `open_http_flv_subscriber`。  
- S1 可先返回 `Unsupported` 或临时桥接 one-shot（**不推荐**作为完成态）；DoD 要求 streaming API。  
- 阶段顺序：S1 骨架 + matrix；S2 完成 HTTP-FLV 真路径。

### 5.3 RTMP push

1. 解析 `RtmpUrl`。  
2. `start_client` publish mode。  
3. `wait_ready` 直到可推。  
4. `push_frame` / `update_tracks` 映射到协议 publish path（可能经 engine publisher 或直接 client——以现有 module 最佳实践为准）。  
5. 优先复用 module 测试 harness 中的 publish 模式（`rtmp_test_harness` 等）。

### 5.4 WebRTC push

1. 解析 WHIP/signaling URL。  
2. 启动 module 发布路径 / `spawn_driver`。  
3. S1 可先完成 “能 open + 明确错误”；完整 media 就绪与 Gap 4 联动。  
4. 禁止在完成态仅返回 “SDP 字符串” 而无 `PushHandle::push_frame`。

---

## 6. 错误（与 Gap 5 衔接）

S1 最小集：

```rust
pub enum ConnectorError {
    InvalidUrl { protocol: Protocol, url: String },
    UnsupportedProtocol { protocol: Protocol, direction: Direction },
    FeatureDisabled { protocol: Protocol, feature: &'static str },
    Connect { protocol: Protocol, message: String }, // S4 再结构化
    Internal(String),
}
```

S4 升级为完整 `ConnectorError`（见 `07`），保持 `#[non_exhaustive]`。

---

## 7. 测试清单（阶段 1）

| ID | 用例 | 期望 |
| --- | --- | --- |
| T-C-01 | `supports` 矩阵 | 仅四合法对为 true |
| T-C-02 | `open_pull(Rtmp, …)` | `UnsupportedProtocol` |
| T-C-03 | `open_push(Rtsp, …)` | `UnsupportedProtocol` |
| T-C-04 | feature 关闭时 `open_pull(HttpFlv)` | `FeatureDisabled` 或等价 |
| T-C-05 | 非法 URL | `InvalidUrl` |
| T-C-06 | `ConnectorBuilder::with_default_modules().build()` | 不 panic；可 shutdown |
| T-C-07 | rustdoc 示例编译（若有） | 通过 |

真实 pull/push 互通放到 Gap2/3 阶段。

---

## 8. DoD（阶段 1）

- [ ] workspace 可 `cargo check -p cheetah-connector`  
- [ ] `RuntimeConnector` + `EngineConnector` + capability 拒绝测通过  
- [ ] **未** 把协议依赖加入 `cheetah-sdk`  
- [ ] public API 无 `tokio::` 类型泄漏  
- [ ] rustdoc 说明 capability matrix 与 feature  
- [ ] `cargo fmt` / `clippy -p cheetah-connector` / `test -p cheetah-connector`  

---

## 9. 非目标

- 完整重连策略框架（HTTP-FLV 在 `05` 局部定义）。  
- 服务端 listen 管理 UI / 控制面 API。  
- 与 `dyun-gu-dev` 仓库内的 bridge 代码双向同步（只提供上游 facade）。  
