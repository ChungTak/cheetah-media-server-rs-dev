# 900 · SDK Gaps：外部 Connector / Loopback / Streaming 能力补齐方案

（源需求：仓库根目录 [`cheetah-media-server-rs-gaps.md`](../../../cheetah-media-server-rs-gaps.md)；面向外部 Rust integrator / dependency-light CI）

> **读者对象**：后续编程智能体。读完本目录即可按阶段改代码，无需再猜 API、分层或验收标准。  
> **本目录是设计与开发计划**，不是实现代码。实现时以本目录 + `AGENTS.md` + `SystemArchitecture.md` + 当前源码为准。  
> **文中 “proposed” 均表示建议能力，当前 checkout 中不存在。**

---

## 0. 目标与范围

为外部 integrator（典型：`dyun-gu-dev`）提供 **可在干净 CI 中构建与运行** 的官方高层 SDK 边界，覆盖 `cheetah-media-server-rs-gaps.md` 列出的 6 项缺口：

| ID | 能力 | 优先级 | 主要落点 |
| --- | --- | --- | --- |
| **Gap 1** | 可安装的高层 **connector/facade**（URL → pull/push handle） | P0 | 新建 `cheetah-connector` |
| **Gap 2** | **in-process / in-memory** protocol loopback harness | P0 | connector + 各协议 test transport |
| **Gap 3** | HTTP-FLV **streaming** `SubscriberSource`（非 one-shot） | P0 | `cheetah-http-flv-module` + connector 包装 |
| **Gap 4** | WebRTC **in-process media** loopback peer | P1 | webrtc module harness + connector |
| **Gap 5** | 统一 **typed ConnectorError** 与协议错误映射 | P1 | connector error + 协议适配 |
| **Gap 6** | **metadata-preserving** 端到端 facade 与 conformance 测 | P1 | connector contract + 字段级断言 |

### 外部 SDK 契约（本方案硬定义）

满足 **全部** 条件才算对外部 integrator 可用：

1. 可通过 Cargo feature 安装；**不**强制依赖浏览器、外部媒体服务器、硬件、native codec SDK。
2. 调用者只需 `(protocol, url, options)` → `SubscriberSource` / `PublisherSink`（或其 wrapper handle），**不必**手写 module registration、driver channel、socket 生命周期。
3. CI 中可跑 `push → embedded protocol runtime → pull`（至少分层 fixture；完整 wire 路径优先）。
4. HTTP-FLV 提供 frame-by-frame `recv`，不是 `Vec<FlvTag>` one-shot。
5. 错误有稳定 typed variants + `source()` + retryable 语义，不靠解析 `String`。
6. 端到端路径保留 `AVFrame` / `TrackInfo` 关键 metadata（codec、format、timebase、PTS/DTS、flags、extradata 等）。

### 明确不做（本方案范围外）

- 重写 RTSP/RTMP/HTTP-FLV/WebRTC 的 protocol-core 状态机（只组合既有 core/driver/module）。
- 把 `cheetah-sdk` 反向依赖具体协议 module（违反 `AGENTS.md` 分层）。
- 在 connector 中复制一套时间戳修正、NALU 处理或参数集缓存（必须走 `cheetah-codec`）。
- 完整生产级 WebRTC 全协议栈 in-memory 仿真若成本过高，允许 **分层验收**（signaling / media fixture / optional local-UDP），但必须诚实标注绕过层。
- 本方案不交付 HLS/TS/SRT/GB28181 等 connector 方向（可后置扩展同一 trait）。
- 不要求外部 integrator 使用 `apps/cheetah-server` 二进制；库形态 connector 即可。

### 与参考方案的关系

| 参考 | 路径 | 复用点 |
| --- | --- | --- |
| 缺口源 | `cheetah-media-server-rs-gaps.md` | 问题陈述、proposed API、验收建议 |
| 文档样板 | `avcodec-rs/dev-docs/plans/900_sdk_gaps_plan/` | 分册结构、阶段工单、Agent 协议 |
| 分层权威 | `AGENTS.md`、`SystemArchitecture.md` | core/driver/module 边界、runtime 约束 |
| 组装样板 | `apps/cheetah-server/src/main.rs` | `EngineBuilder` + module factory 注册 |
| 契约样板 | `crates/sdk/cheetah-sdk/src/stream.rs` | `PublisherSink` / `SubscriberSource` |
| 协议入口 | 各 `*-driver-tokio` / `*-module` | 低层 API 证据见 `01` |

### 与 avcodec-rs 900 的关键差异

| 维度 | avcodec-rs 900 | **本方案（cheetah 900）** |
| --- | --- | --- |
| 主题 | native-free codec backend | 外部协议 connector / loopback |
| 是否改 core | 允许改 avcodec-core-model | **禁止**在 protocol-core 引入 I/O/async 业务编排 |
| 新 crate | `avcodec-backend-rust-h264` | **`cheetah-connector`**（组合层） |
| 主路径 | RegistryBuilder / Decoder | RuntimeConnector / LoopbackPair |
| 依赖方向 | backend → core | connector → sdk + engine + modules（**sdk 不依赖 connector**） |

---

## 1. 现状摘要（证据细节见 `01`）

### 1.1 已可用（可复用，非缺口本身）

| 组件 | 路径 | 说明 |
| --- | --- | --- |
| SDK 流契约 | `crates/sdk/cheetah-sdk/src/stream.rs` | `PublisherSink` / `SubscriberSource` / `StreamManagerApi` |
| SDK 错误 | `crates/sdk/cheetah-sdk/src/error.rs` | coarse `SdkError` 六变体（**Gap 5 要在 connector 层补 typed**） |
| Engine | `crates/system/cheetah-engine/src/engine.rs` | `EngineBuilder` / `start` / `stop` / stream APIs |
| 应用组装 | `apps/cheetah-server/src/main.rs` | feature 门控注册各 `*ModuleFactory` |
| RTSP client | `crates/protocols/rtsp/driver-tokio/src/client/mod.rs` | `start_tcp_client(...)` |
| HTTP-FLV one-shot | `crates/protocols/http-flv/module/src/pull.rs` | `pull_http_flv_once` → `HttpFlvPullResult` |
| RTMP client | `crates/protocols/rtmp/driver-tokio/src/client.rs` | `start_client(...)` |
| WebRTC driver | `crates/protocols/webrtc/driver-tokio/src/runner.rs` | `spawn_driver(...)` |
| WebRTC P2P in-mem | `crates/protocols/webrtc/module/src/p2p/transport.rs` | **仅 signaling**，非 media transport |
| 媒体模型 | `cheetah-codec` `AVFrame` / `TrackInfo` / `CodecExtradata` | 字段已足够；缺 facade 保证 |

### 1.2 缺失（本方案要补）

```text
外部调用者今天必须自己：
  解析 URL → 选 protocol API
  建 RuntimeApi / Engine / 注册 ModuleFactory
  处理 driver event/command channel
  把 FLV tags / RTP / RTMP messages 接到 AVFrame
  重连、队列、取消、错误分类、metadata 对齐

本方案交付后：
  Connector::open_pull / open_push
  open_in_memory_loopback(protocol)
  open_http_flv_subscriber → dyn SubscriberSource
  typed ConnectorError
  metadata conformance tests
```

---

## 2. 文件索引（本方案按主题拆分）

| 文件 | 内容 | 主要读者动作 |
| --- | --- | --- |
| `README.md`（本文件） | 目标、现状、架构决策、阶段总览、Agent 协议 | 建立全局图 |
| [`01_gap_inventory_and_evidence.md`](./01_gap_inventory_and_evidence.md) | 六 Gap 证据矩阵、路径表 | 核对现状，禁止臆测 |
| [`02_architecture_and_layering.md`](./02_architecture_and_layering.md) | 分层、crate/feature 矩阵、改/不改清单 | 设计边界 |
| [`03_connector_facade.md`](./03_connector_facade.md) | **Gap 1** RuntimeConnector | **阶段 1 实现** |
| [`04_loopback_transport.md`](./04_loopback_transport.md) | **Gap 2** in-memory loopback | **阶段 2 实现** |
| [`05_http_flv_streaming_subscriber.md`](./05_http_flv_streaming_subscriber.md) | **Gap 3** streaming pull | **阶段 3 实现** |
| [`06_webrtc_media_loopback.md`](./06_webrtc_media_loopback.md) | **Gap 4** WebRTC media peer | **阶段 4 实现** |
| [`07_connector_error_mapping.md`](./07_connector_error_mapping.md) | **Gap 5** ConnectorError | **阶段 5 实现** |
| [`08_metadata_preservation.md`](./08_metadata_preservation.md) | **Gap 6** metadata contract | **阶段 6 实现** |
| [`09_build_test_ci_acceptance.md`](./09_build_test_ci_acceptance.md) | examples、CI、验收命令 | 集成与门禁 |
| [`10_roadmap_risks.md`](./10_roadmap_risks.md) | **分阶段 checkbox、依赖图、风险** | **排期与并行拆分** |

**实现智能体推荐阅读顺序**：

`README` → `01` → `02` → 按任务读 `03`–`08` → `09` → 按 `10` 阶段开工。

---

## 3. 总体架构决策（摘要，细则见 `02`）

1. **新建 `cheetah-connector` crate**（建议路径 `crates/sdk/cheetah-connector`），作为 **唯一** 对外组合层；**不**把协议 module 依赖塞进 `cheetah-sdk`。
2. **Capability matrix 钉死**（首版）：

   | Protocol | Pull | Push |
   | --- | --- | --- |
   | RTSP | **yes** | no |
   | HTTP-FLV | **yes** | no |
   | RTMP | no | **yes** |
   | WebRTC | no | **yes** |

   非法方向 → `ConnectorError::UnsupportedProtocol { protocol, direction }`。
3. **Handle 包装既有契约**：`PullHandle` 实现/包装 `SubscriberSource`；`PushHandle` 实现/包装 `PublisherSink`；禁止另起一套 frame 模型。
4. **Loopback 优先协议语义**：`open_in_memory_loopback` 必须走 **协议 framing / track negotiation** 路径；`Engine open_publisher→open_subscriber` 仅作 **smoke**，不得冒充 protocol conformance。
5. **HTTP-FLV streaming 落在 module**：`open_http_flv_subscriber` 先在 `cheetah-http-flv-module` 实现，connector 再统一导出。
6. **WebRTC 分层诚实**：signaling / media fixture / optional UDP 分测；禁止用 SDP 生成冒充 media round-trip。
7. **错误在 connector 边界统一**：协议内部错误保留；对外统一 `ConnectorError`（可 `From` / map），`SdkError` 保持兼容不破坏现有 module。
8. **Runtime 约束**：connector / module 公共路径遵守 `AGENTS.md`——不暴露 `tokio::*` 到 sdk 公共接口；取消与 spawn 走 `RuntimeApi` / `CancellationToken`。
9. **Feature 门控协议依赖**：`rtsp` / `http-flv` / `rtmp` / `webrtc` / `loopback`；默认 feature 可为空或 `full` 可选，避免强拉全部协议。
10. **单 PR 尽量对应单阶段**（或 `10` 子任务包），避免大爆炸 diff。

### 目标布局（实现后）

```text
crates/sdk/cheetah-connector/                 # 新建：Gap 1/2/5/6 主落点
  Cargo.toml                                  # features: rtsp, http-flv, rtmp, webrtc, loopback
  src/
    lib.rs
    protocol.rs                               # Protocol, Direction, capability matrix
    error.rs                                  # ConnectorError
    options.rs                                # ConnectorSubscriberOptions / PublisherOptions
    connector.rs                              # RuntimeConnector + default EngineConnector
    handles.rs                                # PullHandle / PushHandle
    loopback.rs                               # open_in_memory_loopback
    metadata.rs                               # MetadataPreservingConnector（或并入 connector）
    engine_bootstrap.rs                       # 默认注册 module factories
  examples/
    external_connector_loopback.rs
  tests/
    capability_matrix.rs
    loopback_rtmp_http_flv.rs
    metadata_conformance.rs
    error_conformance.rs

crates/protocols/http-flv/module/src/
  pull.rs                                     # 保留 one-shot
  streaming.rs                                # 新建：open_http_flv_subscriber（Gap 3）

crates/protocols/webrtc/module/src/           # 或 tests/ harness 模块
  loopback/ 或 testing/media_loopback.rs      # Gap 4 media peer / fixture

根 Cargo.toml                                 # workspace member + 可选 workspace dep
```

---

## 4. 分阶段路线图（详单见 `10`）

| 阶段 | 主题 | 主要产出 | 估计 |
| --- | --- | --- | --- |
| **0** | 名称/feature/能力矩阵冻结 + API 表面钉死 | 决策记录；路径核对 | 0.5–1 天 |
| **1** | Gap 1 connector facade 骨架 | crate + EngineConnector + matrix 拒绝 | 2–4 天 |
| **2** | Gap 3 HTTP-FLV streaming subscriber | module API + connector 包装 | 2–4 天 |
| **3** | Gap 2 RTMP↔HTTP-FLV / 多协议 loopback 主路径 | loopback harness + 至少 1 条 e2e | 3–6 天 |
| **4** | Gap 5 typed ConnectorError | error 模块 + 映射 + conformance | 1–3 天 |
| **5** | Gap 6 metadata preservation | contract + 字段级测 | 2–4 天 |
| **6** | Gap 4 WebRTC media loopback（分层） | signaling + media fixture + optional UDP | 3–7 天 |
| **7** | example / CI / 文档收尾 | `external_connector_loopback` + 门禁 | 1–2 天 |

> **并行建议**：S0 后，S2（HTTP-FLV streaming）可与 S1 后期并行；S4 错误类型可在 S1 骨架阶段先落地再逐步 map；S5 依赖至少一条完整 loopback；S6 可与 S3 后期并行但独立验收；S7 最后。

**推荐关键路径（P0 优先）**：

```text
S0 → S1 (facade) → S2 (http-flv stream) → S3 (loopback) → S4 (errors) → S5 (metadata) → S7
                                                                      ↘ S6 (webrtc) ↗
```

---

## 5. Agent 执行总协议（强制）

实现智能体必须遵守：

1. **先读后写**：改模块前读本目录对应分册 + 相关源文件 + `AGENTS.md` 相关条款。
2. **区分 proposed / 现状**：不得把建议 API 写成“已存在”；注释/文档中用 `// proposed` 或 “当前不存在”。
3. **每阶段结束验收**：跑 `10` 该阶段验收命令；失败不得标完成。
4. **分层不破**：
   - protocol-core：继续 Sans-I/O，不引入 socket/async 业务编排。
   - `cheetah-sdk`：**不**依赖具体协议 module。
   - connector 可依赖 engine + modules（组合层）。
5. **runtime 中立公共接口**：sdk / connector 公共 API 禁止直接暴露 `tokio::*` / `tokio_util::*`。
6. **不复制 codec 逻辑**：时间戳、NALU、参数集走 `cheetah-codec`。
7. **诚实能力**：未实现协议方向返回 `Unsupported`，禁止静默降级到绕过协议的 engine 直连。
8. **单 PR 对应单阶段**或 `10` 明确子任务包。
9. **测试分层标注**：engine-only smoke 与 protocol loopback 必须分文件或分 test name，文档写明绕过层。
10. 变更 public API 必须有单元/集成测试 + rustdoc（中英或英文，与邻近代码一致）。
11. 提交前最低检查（改动 crate）：`cargo fmt`、`cargo clippy -p <crate>`、`cargo test -p <crate>`。

---

## 6. 关键风险（摘要，详见 `10`）

| 风险 | 缓解 |
| --- | --- |
| 误把 `StreamManagerApi` open_publisher→open_subscriber 当协议 loopback | 测试命名 + 文档 + 禁止作为 Gap 2 DoD |
| WebRTC in-memory ICE/DTLS/SRTP 成本过高 | 分层 fixture；SDP-only 不得标 media done |
| connector 把 sdk 变成“上帝 crate” | 独立 `cheetah-connector`；sdk 保持契约层 |
| 协议错误映射遗漏导致 stringly 回归 | error conformance 固定变体 + retryable 表 |
| HTTP-FLV streaming 与 one-shot 双轨复杂度 | one-shot 保留；streaming 新 API；共享 demux 路径 |
| feature 组合爆炸 | 矩阵测 + CI 只跑 `connector` 指定 feature 集 |
| 多 agent 改同一协议 module 冲突 | S2 主改 http-flv；S6 主改 webrtc；S1/S4/S5 主改 connector |

---

## 7. 交付物汇总

### 本方案文档（本目录，已交付）

11 个 markdown，见 §2。

### 实现阶段代码交付（由实现 agent 完成）

```text
crates/sdk/cheetah-connector/                 # 新建
crates/protocols/http-flv/module/src/streaming.rs  # 或等价模块
crates/protocols/webrtc/module/...            # media loopback harness
根 Cargo.toml                                 # workspace member
examples 或 connector examples/
# 可选
dev-docs 交叉引用 / gaps.md 状态注记
```

### 验收总标准（全部阶段完成后）

- [ ] `cheetah-connector` 可 feature 安装；构建不强制 native SDK / 浏览器 / 外部媒体服务器
- [ ] capability matrix：RTSP/HTTP-FLV pull、RTMP/WebRTC push；非法方向 typed 拒绝
- [ ] 至少一条协议路径完成 `push → protocol runtime → pull`（优先 RTMP push + HTTP-FLV pull 或同协议 pair）
- [ ] HTTP-FLV streaming：`recv` 逐帧、cancel、close、bounded queue、reconnect policy 可测
- [ ] WebRTC：signaling 与 media **分别**验收；metadata 在 media 路径可断言（或明确 fixture 层）
- [ ] `ConnectorError` typed + retryable + source chain
- [ ] metadata conformance：`TrackInfo`/`AVFrame` 关键字段不静默丢失
- [ ] engine-only smoke 存在且标注 **不替代** protocol loopback
- [ ] `AGENTS.md` 分层与 Sans-I/O 约束未破

---

## 8. 与 `cheetah-media-server-rs-gaps.md` 的映射

| gaps.md 章节 | 本目录 |
| --- | --- |
| §1 背景/目标 | README §0 |
| §2 现状 | README §1、`01` |
| §3 Gap 1 | `03` + `01` §Gap1 |
| §3 Gap 2 | `04` + `01` §Gap2 |
| §3 Gap 3 | `05` + `01` §Gap3 |
| §3 Gap 4 | `06` + `01` §Gap4 |
| §3 Gap 5 | `07` + `01` §Gap5 |
| §3 Gap 6 | `08` + `01` §Gap6 |
| §4 验收建议 | `09` + `10` 阶段验收 |

实现完成后，应在 `cheetah-media-server-rs-gaps.md` 或 release note 中交叉引用本目录，标注各 Gap 状态（open / in-progress / done）。
