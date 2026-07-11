# 10 · 分阶段任务清单、依赖、风险与 Agent 交接协议

> **本文是实现智能体的主工单。**  
> 每个子任务含：改动文件、完成定义（DoD）、验收命令、依赖。  
> 阅读顺序：`README` → `01` → `02` → 按任务读 `03`–`09` → 按本文件阶段开工。

---

## 0. 总依赖图

```text
S0  名称/feature/矩阵/路径钉死
 │
 ├── S1  Gap1 connector facade 骨架 ──────────────┐
 │         │                                       │
 │         ▼                                       │
 ├── S2  Gap3 HTTP-FLV streaming  ◄── 可与 S1 后期并行
 │         │                                       │
 │         ▼                                       │
 ├── S3  Gap2 L1 loopback（依赖 S1+S2 主路径）      │
 │         │                                       │
 ├── S4  Gap5 ConnectorError（可在 S1 末落地骨架）  │
 │         │                                       │
 ├── S5  Gap6 metadata conformance（依赖 S3）      │
 │         │                                       │
 ├── S6  Gap4 WebRTC media 分层（可与 S3 后期并行） │
 │         │                                       │
 └── S7  example / CI / gaps 状态注记（依赖 P0 路径）
```

**串行关键路径（P0）**：`S0 → S1 → S2 → S3 → S7`  
**P1 增强**：`S4`（建议尽早）、`S5`、`S6`

**可并行**：

| 并行组 | 说明 |
| --- | --- |
| S2 ∥ S1 后期 | streaming 主要改 http-flv-module |
| S4 ∥ S2/S3 | error 类型先合，map 逐步补 |
| S6 ∥ S3 后期 | 只动 webrtc 树 |
| S5 需 S3 | 字段断言需要真实路径 |

---

## 阶段 0 — 审计与名称冻结

**目标**：钉死 crate/feature/API 名与真实源码路径；无代码或仅文档补丁。  
**前置**：无。  
**文档**：`01`、`02`、`03`。

| ID | 描述 | 改动文件 | DoD | 验收命令 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S0-T1** | 核对低层 API 路径与签名 | 只读 | 与 `01` 表一致或更新 `01` | `rg` 命令见 `01` §0 | 0.2d |
| **S0-T2** | 核对各 module package name / factory 类型名 | 只读 | 记入 `02`/`03` 表 | `rg 'ModuleFactory' crates/protocols/*/module` | 0.2d |
| **S0-T3** | 冻结命名：`cheetah-connector`、`RuntimeConnector`、`ConnectorError`、features | 若不一致则改本目录 | 全文一致 | `rg 'cheetah-connector\|RuntimeConnector' dev-docs/plans/900_sdk_gaps_plan` | 0.1d |
| **S0-T4** | 选定首版 L1 拓扑（默认 RTMP→HTTP-FLV） | 决策记 PR/本文件注记 | 拓扑明确 | 人工 | 0.1d |
| **S0-T5** | 列出 engine 测试用 config/listen 注入方式 | 工作笔记 | 可复制样例路径 | `rg listen EngineBuilder` 测试 | 0.2d |

**阶段 0 验收：**

```bash
rg -n 'fn start_tcp_client|fn pull_http_flv_once|fn start_client|fn spawn_driver' --glob '*.rs'
rg -n 'struct.*ModuleFactory|impl ModuleFactory' crates/protocols --glob '**/module/**/*.rs' | head
```

---

## 阶段 1 — Gap 1：Connector facade 骨架

**目标**：`cheetah-connector` 可编译；capability matrix；builder/engine bootstrap；非法方向拒绝。  
**前置**：S0。  
**文档**：`03`、`02`。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S1-T1** | 新建 crate + workspace member | `crates/sdk/cheetah-connector/**`、根 `Cargo.toml` | `cargo check -p cheetah-connector` | check | 0.5d |
| **S1-T2** | `Protocol`/`Direction`/`supports` | `src/protocol.rs` | 单测矩阵 | test | 0.2d |
| **S1-T3** | 最小 `ConnectorError` | `src/error.rs` | 非法方向可用 | test | 0.3d |
| **S1-T4** | `ConnectorBuilder` + 默认 module 注册（cfg feature） | `engine_bootstrap.rs` | build/shutdown | test | 1d |
| **S1-T5** | `RuntimeConnector` + `EngineConnector` open_* 分发 | `connector.rs`/`handles.rs` | 未实现路径明确错误 | test | 1d |
| **S1-T6** | feature 门控依赖 | `Cargo.toml` | 关 feature 不拉多余协议 | tree/check | 0.5d |
| **S1-T7** | capability_matrix 集成测 | `tests/capability_matrix.rs` | T-C-01…05 | test | 0.5d |
| **S1-T8** | rustdoc：矩阵、features、分层 | `lib.rs` | 文档齐全 | 人工 | 0.2d |

**验收：**

```bash
cargo fmt
cargo clippy -p cheetah-connector --features full
cargo test -p cheetah-connector --features full
```

**DoD 额外：** `cheetah-sdk` 的 `Cargo.toml` **未** 增加协议 module 依赖。

---

## 阶段 2 — Gap 3：HTTP-FLV streaming subscriber

**目标**：`open_http_flv_subscriber` 逐帧 `recv`；connector pull 可接。  
**前置**：S1-T1 建议已有；不依赖完整 loopback。  
**文档**：`05`。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S2-T1** | 抽取 one-shot 与 streaming 共享 IO/demux | `http-flv/module/src/*` | 无大段复制 | review | 0.5–1d |
| **S2-T2** | `HttpFlvSubscriberOptions` + `open_http_flv_subscriber` | `streaming.rs` | API 可调用 | check | 0.5d |
| **S2-T3** | 后台读循环 + bounded queue + tag→AVFrame | `streaming.rs`/`map` | 出帧 | test | 1–2d |
| **S2-T4** | cancel/close/Drop 清理 | 同上 | 无任务泄漏 | test | 0.5d |
| **S2-T5** | reconnect 策略（有限次） | 同上 | 测覆盖 | test | 0.5d |
| **S2-T6** | `retryable` 变 pub（若需要） | `pull.rs` | 外部可调用 | test | 0.1d |
| **S2-T7** | connector `open_pull(HttpFlv)` 接线 | `cheetah-connector` | 集成 | test | 0.5d |
| **S2-T8** | one-shot 回归 | module tests | 仍绿 | test | 0.2d |

**验收：**

```bash
cargo test -p cheetah-http-flv-module --locked
cargo test -p cheetah-connector --features http-flv,full --locked
```

---

## 阶段 3 — Gap 2：L1 Protocol loopback

**目标**：至少一条 `push → protocol runtime → pull`；L0 bypass 分离。  
**前置**：S1 + S2（若选 RTMP→HTTP-FLV）。  
**文档**：`04`、`09`。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S3-T1** | `LoopbackOptions`/`LoopbackPair`/`open_in_memory_loopback` | `connector/src/loopback.rs` | API 存在 | check | 0.5d |
| **S3-T2** | 测试用 ephemeral listen 配置注入 | bootstrap/tests | 端口不冲突 | test | 0.5–1d |
| **S3-T3** | RTMP `open_push` 真路径 | connector push/rtmp | wait_ready | test | 1–2d |
| **S3-T4** | 组合 L1：push 帧 + HTTP-FLV recv | tests/loopback_* | ≥1 帧 | test | 1–2d |
| **S3-T5** | L0 engine smoke 独立文件 | tests/engine_smoke_bypass_wire.rs | 标注 bypass | test | 0.3d |
| **S3-T6** | close/cancel/有界队列 | tests | 不挂死/不 OOM | test | 0.5d |
| **S3-T7** | 非法 topology 错误 | tests | typed | test | 0.2d |

**验收：**

```bash
cargo test -p cheetah-connector --features full --locked loopback
cargo test -p cheetah-connector --features full --locked engine_smoke
```

---

## 阶段 4 — Gap 5：ConnectorError 完善

**目标**：结构化错误 + 映射 + retryable conformance。  
**前置**：S1 最小错误可先存在；本阶段升级。  
**文档**：`07`。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S4-T1** | 完整 `ConnectorError`/`Operation`/`CloseReason` | `error.rs` | 非 exhaustive | check | 0.5d |
| **S4-T2** | map helpers：http-flv/rtmp/rtsp/webrtc/sdk | `error.rs` 或 `map_*.rs` | source 链 | unit | 1d |
| **S4-T3** | `retryable()` 表 | 同上 | 单测 | test | 0.3d |
| **S4-T4** | open/pull/push 路径改用 map | connector | 无纯字符串唯一错误 | review | 0.5d |
| **S4-T5** | error_conformance 集成测 | tests/error_conformance.rs | T-E-* | test | 0.5d |

**验收：**

```bash
cargo test -p cheetah-connector --features full --locked error
```

**注意**：**不要** 破坏性改写 `cheetah_sdk::SdkError` 变体集。

---

## 阶段 5 — Gap 6：Metadata preservation

**目标**：字段级 conformance；tracks 可查询。  
**前置**：S3。  
**文档**：`08`。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S5-T1** | `PullHandle::tracks` / push tracks API | handles | 可查询 | test | 0.3d |
| **S5-T2** | `assert_frame_metadata_eq` helpers | tests/support | 复用 | test | 0.3d |
| **S5-T3** | L0 全字段相等 baseline | tests | 绿 | test | 0.2d |
| **S5-T4** | L1 视频+音频关键字段 | tests/metadata_* | 非 Unknown | test | 1d |
| **S5-T5** | extradata / keyframe flags | tests | 断言 | test | 0.5d |
| **S5-T6** | 规范化策略文档 | rustdoc 或本目录注记 | 写明比较模式 | 人工 | 0.2d |

**验收：**

```bash
cargo test -p cheetah-connector --features full --locked metadata
```

---

## 阶段 6 — Gap 4：WebRTC media loopback 分层

**目标**：W1 signaling-only + W2 media fixture；可选 W3。  
**前置**：S1；与 S3 可并行。  
**文档**：`06`。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S6-T1** | 梳理既有 WHIP/self interop 测试并标注 W1 | webrtc tests | 命名/注释 | review | 0.3d |
| **S6-T2** | Media fixture harness / datagram pair | webrtc module testing | 可推帧 | test | 2–4d |
| **S6-T3** | packetize→fixture→depacketize→AVFrame | 同上 | T-W2-01 | test | 1–2d |
| **S6-T4** | connector `open_push(WebRtc)` 或 loopback 接线 | connector | 可调用 | test | 0.5–1d |
| **S6-T5** | 文档标明 BYPASS 层 | tests + rustdoc | 诚实 | 人工 | 0.2d |
| **S6-T6** | 可选 local UDP W3 | tests `#[ignore]` | 可选 | manual | 1d |

**验收：**

```bash
cargo test -p cheetah-webrtc-module --locked
cargo test -p cheetah-connector --features webrtc,full --locked webrtc
```

---

## 阶段 7 — Example / CI / 文档收尾

**目标**：`external_connector_loopback` + CI 门禁 + gaps 状态。  
**前置**：S1–S3 必须；理想 S4–S6 已合。  
**文档**：`09`。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S7-T1** | example `external_connector_loopback` | `examples/*.rs` | run 退出 0 | run | 0.5d |
| **S7-T2** | CI job 或 dev-scripts 入口 | CI yaml / `dev-scripts/` | 命令可跑 | run | 0.5d |
| **S7-T3** | `cheetah-media-server-rs-gaps.md` 状态注记 | 根 gaps 或 release note | 映射 900 | 人工 | 0.2d |
| **S7-T4** | README 短节（可选） | 根 `README.md` | 不误导 | 人工 | 0.2d |
| **S7-T5** | 总门禁跑通 | — | `09` §8 | 见下 | 0.3d |

**验收：**

```bash
cargo fmt --check
cargo test -p cheetah-connector --features full --locked
cargo run -p cheetah-connector --example external_connector_loopback --features full --locked
```

---

## 1. 风险登记册

| ID | 风险 | 影响 | 缓解 | 触发时动作 |
| --- | --- | --- | --- | --- |
| R1 | 把 L0 engine smoke 当协议完成 | 外部 SDK 假绿 | 文件名/常量强制；CR checklist | 重开 S3 DoD |
| R2 | WebRTC 全内存 SRTP 不可行 | S6 延期 | W2 fixture 诚实 bypass | 文档标注；不阻塞 P0 |
| R3 | RTMP→HTTP-FLV 路由/鉴权配置复杂 | S3 延期 | 复制 module 既有 harness 最小配置 | S0-T5 钉样例 |
| R4 | connector 依赖过重 | 安装体验差 | feature 门控；default=[] | tree 审查 |
| R5 | sdk 被误加协议依赖 | 分层破坏 | PR 检查 `cheetah-sdk/Cargo.toml` | 立刻回滚 |
| R6 | HTTP-FLV one-shot 与 streaming 分叉 | 双倍 bug | 共享 demux/IO | S2-T1 |
| R7 | 错误映射回退 stringly | Gap5 失败 | conformance 测 | S4-T5 门禁 |
| R8 | metadata 只测 L0 | Gap6 空心 | 强制 L1 断言 | S5-T4 |
| R9 | 端口 flaky CI | 假红 | `:0` + 重试有限 | 修测试不删断言 |
| R10 | 多 agent 改同一文件 | 冲突 | 按阶段文件所有权 | 见下 |

---

## 2. 多 Agent 文件所有权（建议）

| Agent 焦点 | 主写路径 | 避免 |
| --- | --- | --- |
| Connector | `crates/sdk/cheetah-connector/**` | 大改 protocol-core |
| HTTP-FLV | `crates/protocols/http-flv/module/**` | 改 webrtc |
| WebRTC | `crates/protocols/webrtc/**` | 改 http-flv |
| Docs only | `dev-docs/plans/900_sdk_gaps_plan/**`、gaps.md | — |

共享接口变更（`PublisherSink` 等）：**禁止** 本方案修改签名；若必须，单独立项。

---

## 3. Agent 交接协议（强制）

每个阶段结束时，实现 agent 在 PR 描述中写：

```markdown
## 900 SDK Gaps 阶段 S?
- 完成任务：S?-T?
- 未完成 / 阻塞：…
- 验收命令与结果：…
- 已知绕过层（BYPASS_*）：…
- 后续 agent 注意：…
```

禁止：

1. 未跑验收命令标 completed。  
2. 把 proposed API 写进用户文档称 “已存在” 却未实现。  
3. 为绿测删除 metadata/error 断言。  
4. 在 protocol-core 引入 socket/async 业务。  
5. 用外部网络服务做默认 CI。

---

## 4. 优先级与发布切片

| 切片 | 包含 | 可对外宣称 |
| --- | --- | --- |
| **MVP-P0** | S0–S3 + S7 最小 example | connector + HTTP-FLV streaming + 一条 L1 loopback |
| **P1-errors-meta** | S4+S5 | typed errors + metadata 契约 |
| **P1-webrtc** | S6 | WebRTC media 可测（分层诚实） |

MVP 未完成前，不要在 README 宣称 “四协议 CI 全绿”。

---

## 5. 完成总检（全部阶段）

- [ ] `cheetah-connector` workspace 成员可测  
- [ ] capability matrix  
- [ ] HTTP-FLV streaming  
- [ ] L1 loopback + L0 bypass 分离  
- [ ] ConnectorError + retryable  
- [ ] metadata L1 断言  
- [ ] WebRTC W1+W2  
- [ ] example + CI  
- [ ] `AGENTS.md` 分层未破  
- [ ] gaps.md 交叉引用本目录状态  

---

## 6. 与 `cheetah-media-server-rs-gaps.md` 任务映射速查

| Gap | 阶段 | 主文档 |
| --- | --- | --- |
| Gap 1 connector | S1 | `03` |
| Gap 2 loopback | S3 | `04` |
| Gap 3 HTTP-FLV stream | S2 | `05` |
| Gap 4 WebRTC media | S6 | `06` |
| Gap 5 errors | S4 | `07` |
| Gap 6 metadata | S5 | `08` |
| 验收 §4 | S7 + 各阶段 | `09` |
