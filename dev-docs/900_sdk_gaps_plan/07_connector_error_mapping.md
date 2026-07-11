# 07 · Gap 5：统一 ConnectorError 与协议错误映射

> **Agent 用途**：阶段 4 主文档——对外 typed 错误。  
> **原则**：不破坏现有 `SdkError`；在 **connector 边界** 统一映射。  
> **现状**：`crates/sdk/cheetah-sdk/src/error.rs` 为 stringly 六变体。

---

## 1. 目标

外部 integrator 可稳定区分：

| 类别 | 用途 |
| --- | --- |
| 坏 URL / 非法参数 | 不重试 |
| 不支持的协议方向 / feature 关闭 | 不重试 |
| 连接失败 | 可重试 |
| 协议拒绝 / 坏状态码 | 视 status |
| 媒体/codec 问题 | 多数不重试 |
| 背压 | 可降速/可重试策略 |
| 正常/异常关闭 | 终态 |

提供：`source()` 链、protocol、operation、endpoint 上下文、`retryable()`。

---

## 2. API 形状（proposed）

```rust
// crates/sdk/cheetah-connector/src/error.rs

use std::error::Error;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConnectorError {
    #[error("invalid url for {protocol:?}: {url}")]
    InvalidUrl {
        protocol: Protocol,
        url: String,
    },

    #[error("unsupported protocol {protocol:?} for {direction:?}")]
    UnsupportedProtocol {
        protocol: Protocol,
        direction: Direction,
    },

    #[error("feature disabled for {protocol:?}: {feature}")]
    FeatureDisabled {
        protocol: Protocol,
        feature: &'static str,
    },

    #[error("connect failed for {protocol:?} endpoint={endpoint}")]
    Connect {
        protocol: Protocol,
        endpoint: String,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },

    #[error("protocol error {protocol:?} op={operation}")]
    Protocol {
        protocol: Protocol,
        operation: Operation,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },

    #[error("media error codec={codec:?}")]
    Media {
        codec: Option<cheetah_codec::CodecId>,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },

    #[error("backpressure on {protocol:?}")]
    Backpressure {
        protocol: Protocol,
    },

    #[error("closed {protocol:?}: {reason:?}")]
    Closed {
        protocol: Protocol,
        reason: CloseReason,
    },

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("internal: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Open,
    Connect,
    Handshake,
    Publish,
    Play,
    Read,
    Write,
    Negotiate,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    User,
    Remote,
    Cancelled,
    Error(String),
}

impl ConnectorError {
    pub fn protocol(&self) -> Option<Protocol> { /* match */ }

    pub fn retryable(&self) -> bool {
        match self {
            Self::InvalidUrl { .. }
            | Self::UnsupportedProtocol { .. }
            | Self::FeatureDisabled { .. }
            | Self::InvalidArgument(_) => false,
            Self::Connect { .. } | Self::Backpressure { .. } => true,
            Self::Protocol { source, .. } => {
                // 尝试 downcast 协议错误；未知默认 false
                retryable_protocol_source(source.as_ref())
            }
            Self::Media { .. } => false,
            Self::Closed { reason, .. } => matches!(reason, CloseReason::Error(_)),
            Self::Internal(_) => false,
        }
    }
}
```

> 形状与 gaps.md 对齐；字段名可微调，但 **语义分类不得合并回纯 String 枚举**。

---

## 3. 映射表（实现时补全 downcast）

### 3.1 HTTP-FLV

| `HttpFlvPullError` | `ConnectorError` | retryable |
| --- | --- | --- |
| `InvalidUrl` / `UnsupportedScheme` | `InvalidUrl` | false |
| `Resolve` / `Connect` | `Connect` | true |
| `BadStatusCode` | `Protocol { operation: Play, … }` | 5xx true / 4xx false（建议） |
| `Cancelled` | `Closed { reason: Cancelled }` | false |
| `FlvDemux` | `Media` 或 `Protocol { Read }` | false |
| `ReadBody` | `Protocol { Read }` | true |

### 3.2 RTSP / RTMP driver

| 来源 | 映射 |
| --- | --- |
| `io::ErrorKind::ConnectionRefused` 等 | `Connect` |
| 握手失败 | `Protocol { Handshake }` |
| 发布拒绝 | `Protocol { Publish }` |
| 超时 | `Connect` 或 `Protocol` + retryable true |

实现时用 helper：

```rust
fn map_io(protocol: Protocol, op: Operation, err: std::io::Error) -> ConnectorError
```

### 3.3 WebRTC

| 来源 | 映射 |
| --- | --- |
| SDP/signaling 失败 | `Protocol { Negotiate }` |
| ICE 失败 | `Connect` |
| DTLS/SRTP | `Protocol` / `Media` |
| 用户 close | `Closed { User }` |

### 3.4 `SdkError`

| `SdkError` | `ConnectorError` |
| --- | --- |
| `InvalidArgument` | `InvalidArgument` |
| `NotFound` | `Protocol` 或 `InvalidArgument`（文档选一） |
| `Unavailable` | `Connect` / retryable true |
| `Internal` | `Internal` |
| 其它 | `Internal` + source |

```rust
impl From<SdkError> for ConnectorError { … }
```

**不要** 反向强制 `SdkError: From<ConnectorError>` 污染 sdk。

---

## 4. 与 `SdkError` 共存策略

```text
module 内部 / 引擎 API     → 继续 SdkError
http-flv pull 细错误       → HttpFlvPullError
connector 对外             → ConnectorError
```

可选后置（**非本方案必须**）：增强 `SdkError` 结构化；若做，另开设计，避免与本阶段耦合。

---

## 5. 可观测性字段约定

每个对外错误尽量带：

1. `protocol`  
2. `operation`  
3. `endpoint` 或 URL（注意 **脱敏**：用户信息/token 不要完整打进 Display；可用 redacted）  
4. `source()` 链非空（对 Connect/Protocol/Media）  

日志：`tracing` 在 map 边界 `warn!(error = %err, retryable = err.retryable(), …)`。

---

## 6. 测试清单

| ID | 用例 | 期望 |
| --- | --- | --- |
| T-E-01 | 非法 URL | `InvalidUrl` + !retryable |
| T-E-02 | 错误方向 | `UnsupportedProtocol` |
| T-E-03 | feature 关闭 | `FeatureDisabled` |
| T-E-04 | 连接 refused（若可模拟） | `Connect` + retryable |
| T-E-05 | HTTP 404 FLV | Protocol + !retryable（若按表） |
| T-E-06 | cancel | `Closed { Cancelled }` |
| T-E-07 | `source()` 链 | `error.source().is_some()` 对 Connect |
| T-E-08 | `From<SdkError>` | 不丢消息 |

---

## 7. DoD（阶段 4）

- [ ] `ConnectorError` 公共稳定分类落地  
- [ ] 主要协议 map helper 存在  
- [ ] `retryable()` 单测覆盖表中关键行  
- [ ] connector public API **不再** 仅返回 `String` 错误作为唯一信息  
- [ ] `SdkError` 变体集未被破坏性替换  

---

## 8. 非目标

- 一次性改写全仓库所有 `io::Error` 类型。  
- gRPC status code 一一对应（可后置）。  
- i18n 错误文案。  
