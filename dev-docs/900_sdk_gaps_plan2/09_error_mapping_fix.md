# 09 · R8：`SdkError → ConnectorError` 去协议臆测

> **Agent 用途**：阶段 7 主文档。  
> **问题**：泛化 `From<SdkError>` 把 `Unavailable` 硬编码为 `Protocol::Rtmp`。

---

## 1. 目标 / 非目标

**目标**：映射不臆测协议；RTSP/WebRTC 错误不会被标成 RTMP。

**非目标**：重写全仓库 `SdkError`；破坏 `ConnectorError` 既有变体语义。

---

## 2. 现状

```rust
// error.rs（基线）
SdkError::Unavailable(msg) => Self::Connect {
    protocol: Protocol::Rtmp, // 错误
    endpoint: msg.clone(),
    ...
}
```

`handles.rs` 的 `map_sdk_error(protocol, err)` 在句柄已知协议时会纠正，但：

- 直接 `ConnectorError::from(sdk_err)` 仍误标；
- 新 adapter 若漏用 map helper 会踩坑。

---

## 3. proposed 规则

### 3.1 废弃无上下文的协议臆测

```rust
// proposed
impl From<SdkError> for ConnectorError {
    fn from(err: SdkError) -> Self {
        match err {
            SdkError::InvalidArgument(m) => Self::InvalidArgument(m),
            SdkError::NotFound(m) => Self::InvalidArgument(m), // 或独立变体
            SdkError::AlreadyExists(m) => Self::InvalidArgument(m),
            SdkError::Conflict(m) => Self::InvalidArgument(m),
            SdkError::Unavailable(m) => Self::Internal(format!("unavailable: {m}")),
            // 更好：新增 ConnectWithoutProtocol / 使用 Option 协议
            SdkError::Internal(m) => Self::Internal(m),
        }
    }
}
```

**推荐**：

```rust
pub fn map_sdk_error(protocol: Protocol, err: SdkError) -> ConnectorError {
    match err {
        SdkError::Unavailable(msg) => ConnectorError::Connect {
            protocol,
            endpoint: redact(&msg),
            source: /* … */,
        },
        // …
    }
}
```

- 公开文档：**prefer** `map_sdk_error`；`From` 仅兜底且 **不** 填假协议。  
- 若 `ConnectorError::Connect` 需要 protocol：兜底用 `Internal` 或扩展 `protocol: Option<Protocol>`（若改枚举，保持 non_exhaustive）。

### 3.2 禁止

```rust
// 禁止
protocol: Protocol::Rtmp  // 在通用 From 中
```

### 3.3 各 adapter

| Adapter | 调用 |
| --- | --- |
| http_flv / rtsp pull | `map_sdk_error(Protocol::HttpFlv \| Rtsp, e)` |
| rtmp / webrtc push | `map_sdk_error(Protocol::Rtmp \| WebRtc, e)` |
| loopback | 按失败侧协议 |

---

## 4. 测试清单

| ID | 用例 | 期望 |
| --- | --- | --- |
| T-ERR-01 | `From: Unavailable` | 不出现 `protocol()==Some(Rtmp)` 除非输入本就 RTMP 上下文 |
| T-ERR-02 | `map_sdk_error(Rtsp, Unavailable)` | `protocol()==Rtsp` |
| T-ERR-03 | `map_sdk_error(WebRtc, …)` | WebRtc |
| T-ERR-04 | handles 路径仍正确 | 回归 |
| T-ERR-05 | retryable 语义不回退 | 与 plan1 表一致 |

```bash
rg -n 'Protocol::Rtmp' crates/sdk/cheetah-connector/src/error.rs
# From 实现中不应再出现硬编码 Rtmp（除 RTMP 专用 helper）
```

---

## 5. DoD（阶段 7）

- [ ] 通用 `From<SdkError>` 无硬编码 `Protocol::Rtmp`  
- [ ] `map_sdk_error(protocol, _)` 为推荐路径并被 adapter 使用  
- [ ] T-ERR-* 绿  
- [ ] RTSP/WebRTC 错误协议字段正确（接线后）  

---

## 6. 衔接

- R1/R2 接线时同步使用 map helper。  
- 不改变 `retryable()` 对 InvalidUrl 等的语义。  
