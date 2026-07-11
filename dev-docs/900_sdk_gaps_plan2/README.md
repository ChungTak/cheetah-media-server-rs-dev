# 900-2 · Connector Residual Gaps：STREAM-01 仍缺能力补齐方案

（源需求：仓库根目录 [`cheetah-media-server-rs-connector-gaps.md`](../../cheetah-media-server-rs-connector-gaps.md)；  
前序方案：[`dev-docs/900_sdk_gaps_plan/`](../900_sdk_gaps_plan/)；面向 `dyun-gu-dev` STREAM-01 / STREAM-02）

> **读者对象**：后续编程智能体。读完本目录即可按阶段改代码，无需再猜 API、分层或验收标准。  
> **本目录是设计与开发计划**，不是实现代码。实现时以本目录 + `AGENTS.md` + `SystemArchitecture.md` + 当前源码为准。  
> **文中 “proposed” 均表示建议能力或待接线行为；“现状”以 connector-gaps 基线 / 当前树为准。**

---

## 0. 目标与范围

在 **已落地的 `cheetah-connector`** 之上，补齐 `cheetah-media-server-rs-connector-gaps.md` 列出的 residual 缺口 **R1–R8**，使外部 integrator 能完成：

| 验收 | 要求 |
| --- | --- |
| **STREAM-01** | 真实 connector：RTSP/HTTP-FLV pull + RTMP/WebRTC push + 本地 loopback 集成测试 |
| **STREAM-02** | frame 元数据在 push/pull 路径上的保真契约（R7 为主） |

### Residual 清单

| ID | 能力 | 优先级 | 主要落点 | STREAM |
| --- | --- | --- | --- | --- |
| **R1** | RTSP pull **接线**（`open_rtsp_pull`） | P0 | `pull/rtsp.rs` + `open_pull` | 01 阻塞 |
| **R2** | WebRTC push **接线**（`open_webrtc_push`） | P0 | `push/webrtc.rs` + `open_push` | 01 阻塞 |
| **R3** | `supports()` 与 `open_*` **行为一致** | P0 | `protocol.rs` + capability 测 | 01 正确性 |
| **R4** | connector options **透传**（read limits / buffer / queue） | P1 | options + http_flv/rtmp/loopback | 01 体验 |
| **R5** | `PushHandle::wait_ready()` **真就绪** | P1 | `handles.rs` + 各 push adapter | 01 flaky |
| **R6** | loopback **socket-free 或诚实标注** | P1 | `loopback.rs` + 文档/层枚举 | 01 CI 视约束 |
| **R7** | RTMP→HTTP-FLV **wire metadata 保真/契约** | P1 | flv_ingress + conformance | **02 阻塞** |
| **R8** | `SdkError→ConnectorError` **去 RTMP 臆测** | P2 | `error.rs` | 质量 |

### 前置条件（强制）

本方案假设 **plan1（900_sdk_gaps_plan）已实现** 或等价的 `cheetah-connector` 已合入：

```text
crates/sdk/cheetah-connector/   # 必须存在
  RuntimeConnector / EngineConnector / ConnectorBuilder
  open_http_flv_pull / open_rtmp_push / open_in_memory_loopback
  ConnectorError / Protocol / supports() / PullHandle / PushHandle
```

**当前仓库若无该 crate**：先完成 [`../900_sdk_gaps_plan/`](../900_sdk_gaps_plan/) 或合入含 connector 的分支，再执行本目录阶段。

connector-gaps 复核基线（历史）：

```text
HEAD 206fc11（Merge PR #81 sdk-gaps-900）
提交 dc1582c feat: implement SDK gaps S1-S7
```

实现时以 **工作区实际源码** 为准；路径漂移时先 `rg` 再改代码，不要盲信旧路径。

### 明确不做

- 不重写 plan1 已交付的 facade 骨架、HTTP-FLV streaming、RTMP push、基础 `ConnectorError`。
- 不把 `cheetah-sdk` 改为依赖协议 module。
- 不在 protocol-core 引入 socket / async 业务编排。
- 不在 connector 复制 NALU/时间戳/参数集逻辑（走 `cheetah-codec`）。
- 不把 WebRTC **media fixture / SDP 生成** 冒充真实 `open_push` 完成。
- 不把 engine-only `open_publisher→open_subscriber` 冒充 RTSP/WebRTC 接线完成。
- 不实现 `dyun-gu-dev` 侧 bridge（仅上游 cheetah）。

---

## 1. 已可消费（勿回退）

以下在 connector-gaps 基线已落地，实现 residual 时 **保持稳定**：

| 能力 | 路径（基线） |
| --- | --- |
| `RuntimeConnector` / `EngineConnector` / `ConnectorBuilder` | `connector.rs`、`engine_bootstrap.rs` |
| `PullHandle` / `PushHandle` / `LoopbackPair` | `handles.rs` |
| HTTP-FLV streaming pull | `http-flv/module` streaming + `pull/http_flv.rs` |
| RTMP push | `push/rtmp.rs` |
| RTMP→HTTP-FLV loopback（localhost TCP，`ProtocolFraming`） | `loopback.rs` |
| WebRTC media fixture loopback（绕过 ICE/DTLS/SRTP） | webrtc harness + loopback `SameProtocol` |
| `ConnectorError` + `protocol()` / `retryable()` / `source()` | `error.rs` |
| `Protocol` / `Direction` / `supports()` | `protocol.rs` |
| features：`rtsp` / `http-flv` / `rtmp` / `webrtc` / `loopback` / `full` | `Cargo.toml` |
| `AVFrame` / `TrackInfo` 等 bridge 类型兼容 | `cheetah-codec`（无 breaking rename） |

**结论摘要（connector-gaps §1）**：HTTP-FLV pull + RTMP push + 二者 loopback **已可用**；RTSP pull / WebRTC push **未接线**；metadata wire 路径 **部分保真**。

---

## 2. 文件索引

| 文件 | 内容 | 主要读者动作 |
| --- | --- | --- |
| `README.md`（本文件） | 目标、勿回退、阶段总览、Agent 协议 | 建立全局图 |
| [`01_residual_inventory_and_evidence.md`](./01_residual_inventory_and_evidence.md) | R1–R8 证据矩阵 | 核对现状 |
| [`02_architecture_and_layering.md`](./02_architecture_and_layering.md) | 分层、改/不改、命名 | 设计边界 |
| [`03_rtsp_pull_adapter.md`](./03_rtsp_pull_adapter.md) | **R1** | **S2 实现** |
| [`04_webrtc_push_adapter.md`](./04_webrtc_push_adapter.md) | **R2** | **S3 实现** |
| [`05_capability_matrix_honesty.md`](./05_capability_matrix_honesty.md) | **R3** | **S1 实现** |
| [`06_options_and_wait_ready.md`](./06_options_and_wait_ready.md) | **R4 + R5** | **S4 实现** |
| [`07_socket_free_loopback.md`](./07_socket_free_loopback.md) | **R6** | **S5 实现** |
| [`08_metadata_wire_fidelity.md`](./08_metadata_wire_fidelity.md) | **R7** | **S6 实现** |
| [`09_error_mapping_fix.md`](./09_error_mapping_fix.md) | **R8** | **S7 实现** |
| [`10_build_test_ci_acceptance.md`](./10_build_test_ci_acceptance.md) | 验收命令 / example / CI | 门禁 |
| [`11_roadmap_risks.md`](./11_roadmap_risks.md) | **阶段 checkbox、风险** | **排期开工** |

**推荐阅读顺序**：`README` → `01` → `02` → 按任务读 `03`–`09` → `10` → 按 `11` 开工。

---

## 3. 总体架构决策（摘要）

1. **增量接线，不重造 facade**：在既有 `open_pull` / `open_push` 分支替换 `UnsupportedProtocol`。
2. **样板对齐 HTTP-FLV / RTMP adapter**：RTSP pull 仿 `open_http_flv_pull`；WebRTC push 仿 `open_rtmp_push` 的会话/句柄模式。
3. **`supports()` 诚实**：已接线 ≡ `true`；未接线期间可暂 `false`，但 **禁止** 长期 `true` + 立即 `UnsupportedProtocol`。
4. **options 透传到底层**：禁止硬编码 `Default::default()` 吞掉 `ConnectorPullOptions` / `LoopbackOptions`。
5. **`wait_ready` 可测**：协议就绪信号，测试禁止纯 `sleep` 作为唯一同步。
6. **loopback 分层诚实**：默认 localhost TCP 必须文档化；socket-free 提供独立 layer 或 API 开关。
7. **metadata 二选一写死契约**：FLV 可表达字段尽量保真；不可表达字段列入 **官方不保真集合** 并测契约，禁止静默丢字段却声称 full fidelity。
8. **错误映射带协议上下文**：禁止 `From<SdkError>` 硬编码 `Protocol::Rtmp`。
9. **单 PR 对应单阶段或 `11` 子任务包**。

### 目标布局（实现后增量）

```text
crates/sdk/cheetah-connector/src/
  pull/
    http_flv.rs          # 已有；R4 透传
    rtsp.rs              # R1 新建
  push/
    rtmp.rs              # 已有；R4/R5
    webrtc.rs            # R2 新建
  protocol.rs            # R3
  options.rs             # R4 扩展字段（若需）
  handles.rs             # R5 wait_ready
  loopback.rs            # R4 queue + R6 layer
  error.rs               # R8
  connector.rs           # open_pull/open_push 接线

crates/foundation/cheetah-codec/src/
  flv_ingress.rs         # R7 可选保真增强

tests / examples         # capability / rtsp / webrtc / metadata / error 增量
```

---

## 4. 分阶段路线图（详单见 `11`）

| 阶段 | 主题 | 产出 | 估计 |
| --- | --- | --- | --- |
| **0** | 基线核对（crate 存在、路径、feature） | 决策笔记 | 0.5d |
| **1** | R3 诚实矩阵 | `supports` + 测一致 | 0.5–1d |
| **2** | R1 RTSP pull | `open_rtsp_pull` + 测 | 3–6d |
| **3** | R2 WebRTC push | `open_webrtc_push` + 测 | 3–7d |
| **4** | R4 + R5 options / wait_ready | 透传 + 就绪信号 | 1–3d |
| **5** | R6 socket-free / 标注 | layer API + 文档 | 1–3d |
| **6** | R7 metadata 契约 | 保真或 not-preserved 表 | 2–4d |
| **7** | R8 error map | 去硬编码协议 | 0.5–1d |
| **8** | example / CI / gaps 状态 | 门禁绿 | 1d |

**P0 关键路径**：`S0 → S1 → S2 → S3 → S8(最小)`  
**P1 增强**：S4、S5、S6；**P2**：S7

---

## 5. Agent 执行总协议（强制）

1. **先读后写**：对应分册 + 现状源文件 + `AGENTS.md`。  
2. **区分 proposed / 现状**；勿把建议 API 写成已存在。  
3. **每阶段跑 `11` 验收命令**；失败不标完成。  
4. **勿回退** §1 已可消费 API 的对外语义。  
5. **分层不破**：sdk 不依赖 protocol module；core Sans-I/O；公共 API 不暴露 `tokio::*`。  
6. **诚实能力**：`supports` / rustdoc / 测试层标注一致。  
7. **测试分层**：fixture / localhost / socket-free / engine-bypass 命名或常量标明。  
8. **单 PR 单阶段**或明确子任务包。  
9. 提交前：`cargo fmt`、`clippy -p <crate>`、`test -p <crate>`（`AGENTS.md` §12）。

---

## 6. 关键风险（摘要）

| 风险 | 缓解 |
| --- | --- |
| 本地无 connector crate | S0 阻塞；先 plan1 |
| RTSP client 事件→AVFrame 适配复杂 | 优先复用 module 既有拉流路径 |
| WebRTC 真实 push 成本高 | 分 WHIP+media；fixture 不替代 open_push |
| supports 先改 false 再实现 | S1 可暂 false；S2/S3 后必须 true |
| FLV 无法表达 duration/side_data | R7 官方不保真集合 |
| 多 agent 冲突 | 见 `11` 文件所有权 |

---

## 7. 与文档映射

| 来源 | 本目录 |
| --- | --- |
| connector-gaps §1 结论 | README §0–§1 |
| connector-gaps §2 已可消费 | README §1、`01` |
| connector-gaps §3 R1–R8 | `03`–`09` + `01` |
| connector-gaps §4 验收 | `10` + `11` |
| plan1 全量 connector | 前置；本目录不重做 |
| 原文 gaps.md Gap1–6 | 已由 plan1/connector 覆盖；本目录只 residual |

实现完成后：在 `cheetah-media-server-rs-connector-gaps.md` 或 release note 标注 R1–R8 状态（open / done）。

---

## 8. 交付物

### 本方案文档（本目录）

12 个 markdown（含 README），见 §2。

### 实现阶段代码交付（由实现 agent 完成）

```text
crates/sdk/cheetah-connector/src/pull/rtsp.rs
crates/sdk/cheetah-connector/src/push/webrtc.rs
# 以及 protocol/options/handles/loopback/error/connector 增量
# 可选 cheetah-codec flv_ingress
# tests + examples 增量
```

### 验收总标准

- [ ] `supports(Rtsp,Pull)` / `supports(WebRtc,Push)` 与 `open_*` 一致且可成功路径  
- [ ] RTSP pull：streaming recv / cancel / close / bounded queue（reconnect 按 DoD）  
- [ ] WebRTC push：真实 `PublisherSink`；signaling≠media 分测  
- [ ] options 透传可测；`wait_ready` 非 stub  
- [ ] loopback 文档/层诚实；可选 socket-free  
- [ ] metadata 保真或官方 not-preserved 表 + 测  
- [ ] `From<SdkError>` 不臆测 RTMP  
- [ ] 勿回退清单 API 仍绿  
