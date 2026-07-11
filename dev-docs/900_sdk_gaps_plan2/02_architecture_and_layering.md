# 02 · 架构分层、改/不改清单与命名冻结

> **Agent 用途**：决定 residual 改哪一层、什么不能动。  
> **权威**：`AGENTS.md`、plan1 `02`、当前 `cheetah-connector` 布局。

---

## 1. 分层（与 plan1 相同，本方案只做增量）

```text
外部 integrator / dyun-gu-dev STREAM-01
        │
        ▼
cheetah-connector          【本方案主改：adapter 接线 + 契约修】
        │
   ┌────┼────┬────────────┐
   ▼    ▼    ▼            ▼
engine modules drivers   cheetah-sdk（契约 only，默认不改）
   │
   ▼
cheetah-codec              【R7 可选改 flv_ingress】
```

### 1.1 依赖方向

| From → To | 允许 |
| --- | --- |
| connector → sdk/engine/codec/runtime/modules | **是**（feature 门控） |
| sdk → connector / 协议 module | **否** |
| protocol-core → socket / EngineContext | **否** |

### 1.2 各 residual 改动层

| 层 | R1 | R2 | R3 | R4 | R5 | R6 | R7 | R8 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `cheetah-connector` | **主** | **主** | **主** | **主** | **主** | **主** | 测/文档 | **主** |
| rtsp driver/module | 消费/薄适配 | — | — | — | — | 可选 | — | map |
| webrtc driver/module | — | 消费/薄适配 | — | — | ready 信号 | fixture 保留 | — | map |
| http-flv module | — | — | — | 透传字段 | — | — | — | — |
| rtmp push path | — | — | — | queue | ready | loopback | wire | — |
| cheetah-codec | — | — | — | — | — | — | **可选** | — |
| cheetah-sdk | **不改** | 不改 | 不改 | 不改 | 不改 | 不改 | 不改 | 不改 |

---

## 2. 目标文件布局（增量）

```text
crates/sdk/cheetah-connector/
  src/
    pull/
      mod.rs
      http_flv.rs      # R4 透传
      rtsp.rs          # R1 新建
    push/
      mod.rs
      rtmp.rs          # R4/R5
      webrtc.rs        # R2 新建
    connector.rs       # open_pull/open_push 分支
    protocol.rs        # R3 supports
    options.rs         # R4 扩展（若基线字段不足）
    handles.rs         # R5
    loopback.rs        # R4 queue + R6
    error.rs           # R8
  tests/
    capability_matrix.rs     # R3 修正
    rtsp_pull_*.rs           # R1
    webrtc_push_*.rs         # R2
    options_passthrough.rs   # R4
    wait_ready.rs            # R5
    loopback_layers.rs       # R6
    metadata_conformance.rs  # R7 扩展
    error_conformance.rs     # R8
  examples/
    external_connector_loopback.rs  # 增量覆盖四方向
```

---

## 3. Feature 矩阵（保持 plan1，不改语义）

| Feature | residual 相关 |
| --- | --- |
| `rtsp` | R1 adapter 编译与注册 |
| `webrtc` | R2 adapter；fixture 已有 |
| `http-flv` / `rtmp` / `loopback` | R4/R6 |
| `full` | CI 一键 |

**规则**：feature 关闭时，`supports` 为 false 或 `open_*` 返回 `FeatureDisabled`（与现有风格一致）；**不得** feature 开着却永远 `UnsupportedProtocol`（R3）。

---

## 4. Capability 运行时矩阵（完成后）

| Protocol | Pull | Push |
| --- | --- | --- |
| RTSP | **wired** | unsupported |
| HTTP-FLV | wired（已有） | unsupported |
| RTMP | unsupported | wired（已有） |
| WebRTC | unsupported | **wired** |

---

## 5. 改 / 不改清单

### 5.1 必须改

| 路径 | residual |
| --- | --- |
| `pull/rtsp.rs`（新建） | R1 |
| `push/webrtc.rs`（新建） | R2 |
| `connector.rs` open_* 分支 | R1/R2 |
| `protocol.rs` supports | R3 |
| `pull/http_flv.rs` + options/loopback/rtmp | R4 |
| `handles.rs` wait_ready | R5 |
| `loopback.rs` + rustdoc | R6 |
| `error.rs` From/map | R8 |
| metadata 测 / 可选 flv_ingress | R7 |

### 5.2 默认不改

| 路径 | 原因 |
| --- | --- |
| `cheetah-sdk` 公共 API | 契约稳定 |
| protocol-core 状态机职责 | Sans-I/O |
| 已绿的 HTTP-FLV/RTMP 主路径语义 | 勿回退 |
| `apps/cheetah-server` | 非库消费者必须 |

### 5.3 可选改

| 路径 | 何时 |
| --- | --- |
| rtsp/webrtc module 导出 test harness | 集成测需要 |
| `flv_ingress.rs` | 选择“尽量保真”策略时 |
| `SubscriberSource::tracks` 等 | 仅 additive |

---

## 6. Public API 稳定性

1. **Additive first**：新 adapter 函数、新 options 字段、新 `LoopbackLayer` 变体。  
2. 已有 `open_http_flv_pull` / `open_rtmp_push` / `open_in_memory_loopback` 签名尽量兼容。  
3. `ConnectorError` 保持 `#[non_exhaustive]`；新增变体需测。  
4. `wait_ready` 从 stub → 真等待：**允许** 改变“立即返回”行为（这是 bugfix）。  
5. `supports` 在接线前可暂时变 false：**breaking for 依赖说谎矩阵的测试**；属正确性修复。

---

## 7. 命名冻结

| 概念 | 名 |
| --- | --- |
| RTSP pull 入口 | `open_rtsp_pull` |
| WebRTC push 入口 | `open_webrtc_push` |
| 选项透传 | `ConnectorPullOptions` / `ConnectorPushOptions` / `LoopbackOptions` |
| 就绪 | `PushHandle::wait_ready` |
| socket-free 层 | `LoopbackLayer::EngineOnlyBypassWire` 和/或 `LoopbackTransport::SocketFree` |
| 不保真表 | `WIRE_METADATA_NOT_PRESERVED`（常量/文档名可微调，须稳定可测） |

---

## 8. AGENTS.md 检查表

- [ ] core 仍 Sans-I/O  
- [ ] module 无 `tokio::net` / `tokio::select!`  
- [ ] connector 公共 API 无 `tokio::*` 泄漏  
- [ ] 无在 module 复制 codec 逻辑  
- [ ] 队列有界  
- [ ] capability 诚实  
- [ ] 新 public API 有 rustdoc + 测试  
