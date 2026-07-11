# 11 · 分阶段任务清单、依赖、风险与 Agent 交接协议

> **本文是实现智能体的主工单。**  
> 每个子任务含：改动文件、DoD、验收命令、依赖。  
> 阅读顺序：`README` → `01` → `02` → 按任务读 `03`–`10` → 按本文件开工。

---

## 0. 总依赖图

```text
S0  基线：connector 存在、路径/feature 钉死
 │
 ├── S1  R3 supports 诚实 ─────────────────────────┐
 │                                                  │
 ├── S2  R1 RTSP pull ──► supports Rtsp=true        │ P0
 ├── S3  R2 WebRTC push ─► supports WebRtc=true     │
 │                                                  │
 ├── S4  R4 options 透传 + R5 wait_ready            │ P1
 ├── S5  R6 loopback 诚实 / socket-free             │
 ├── S6  R7 metadata 契约（STREAM-02）              │
 ├── S7  R8 error map 修正                          │ P2
 │                                                  │
 └── S8  example / CI / connector-gaps 状态注记 ◄───┘
```

**串行关键（STREAM-01）**：`S0 → S1 → S2 → S3 → S8(最小)`  
**可并行**：S4 可与 S2/S3 后期并行（注意 handles 冲突）；S5 ∥ S6 ∥ S7；S6 不阻塞 STREAM-01 主验收。

---

## 阶段 0 — 基线核对

**目标**：确认 `cheetah-connector` 可用；钉死路径与命名。  
**前置**：plan1 已合入或等价 crate 在树。  
**文档**：`01`、`02`。

| ID | 描述 | 改动 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S0-T1** | 确认 crate 成员与 features | 只读 | `cargo check -p cheetah-connector --features full` | check | 0.1d |
| **S0-T2** | 核对 open_pull/open_push 中 Rtsp/WebRtc 分支 | 只读 | 与 `01` 一致或更新 `01` | rg | 0.1d |
| **S0-T3** | 核对 HTTP-FLV/RTMP adapter 样板函数名 | 只读 | 记入 03/04 | rg | 0.1d |
| **S0-T4** | 选定 R3 策略 A 或 B | 笔记 | 全文一致 | 人工 | 0.1d |
| **S0-T5** | 选定 R7 策略 A/B 组合 | 笔记 | 契约方向明确 | 人工 | 0.1d |

**验收：**

```bash
test -d crates/sdk/cheetah-connector
cargo check -p cheetah-connector --features full
rg -n 'UnsupportedProtocol|open_http_flv|open_rtmp' crates/sdk/cheetah-connector --glob '*.rs'
```

若 crate 缺失：**停止**，先执行 `dev-docs/900_sdk_gaps_plan`。

---

## 阶段 1 — R3：supports 诚实

**文档**：`05`。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S1-T1** | 修正 `supports()` 对未接线方向 | `protocol.rs` | 无说谎 true | unit | 0.2d |
| **S1-T2** | 修正 capability_matrix 测 | `tests/capability_matrix.rs` | T-CAP-* | test | 0.3d |
| **S1-T3** | rustdoc 矩阵 | `lib.rs`/`protocol.rs` | 一致 | 人工 | 0.1d |

**验收：**

```bash
cargo test -p cheetah-connector --features full --locked capability
```

**注**：S2/S3 完成后把对应 supports 改回 true 并补测（可列 S2-T* / S3-T*）。

---

## 阶段 2 — R1：RTSP pull

**文档**：`03`。  
**前置**：S0；S1 建议已合。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S2-T1** | 新建 `pull/rtsp.rs` 骨架 | `pull/rtsp.rs`、`pull/mod.rs` | 编译 | check | 0.5d |
| **S2-T2** | URL 解析 + InvalidUrl | rtsp.rs | 测 | test | 0.3d |
| **S2-T3** | 连接 `start_tcp_client` + 会话 PLAY | rtsp.rs + 可能 module | 连通 | test | 1–2d |
| **S2-T4** | 事件/媒体 → AVFrame + 有界队列 | rtsp.rs + codec | 出帧 | test | 1–2d |
| **S2-T5** | cancel/close/Drop | rtsp.rs | 终态 | test | 0.5d |
| **S2-T6** | `open_pull` 接线 + supports true | `connector.rs`/`protocol.rs` | 一致 | test | 0.3d |
| **S2-T7** | 集成测 T-RTSP-* | `tests/rtsp_pull.rs` | 绿 | test | 0.5–1d |
| **S2-T8** | 错误 map 用 Protocol::Rtsp | error helpers | 正确协议 | test | 0.2d |

**验收：**

```bash
cargo test -p cheetah-connector --features full --locked rtsp
cargo test -p cheetah-connector --features full --locked capability
```

---

## 阶段 3 — R2：WebRTC push

**文档**：`04`。  
**前置**：S0；S1 建议已合。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S3-T1** | 新建 `push/webrtc.rs` 骨架 | `push/webrtc.rs` | 编译 | check | 0.5d |
| **S3-T2** | URL/WHIP 解析 | webrtc.rs | InvalidUrl 测 | test | 0.3d |
| **S3-T3** | 信令 + 会话创建 webrtc.rs + module | answer 成功 | test | 1–2d |
| **S3-T4** | PushHandle → media 发送 | webrtc.rs | push_frame | test | 1–2d |
| **S3-T5** | wait_ready 信号接入（可与 S4 合并） | handles/webrtc | 非 stub | test | 0.5d |
| **S3-T6** | `open_push` 接线 + supports true | connector/protocol | 一致 | test | 0.3d |
| **S3-T7** | 媒体测 T-WR-05（非纯 SDP） | tests/webrtc_push.rs | 绿 | test | 1d |
| **S3-T8** | fixture 回归不坏 | 既有 webrtc loopback 测 | 绿 | test | 0.3d |

**验收：**

```bash
cargo test -p cheetah-connector --features full --locked webrtc
cargo test -p cheetah-connector --features full --locked capability
```

---

## 阶段 4 — R4 + R5：options 与 wait_ready

**文档**：`06`。  
**前置**：S2/S3 至少 RTMP 路径可用；WebRTC ready 可在 S3 后补。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S4-T1** | options 结构扩展 | options.rs | 编译 | check | 0.3d |
| **S4-T2** | http_flv 透传 read_limits/buffer/queue | pull/http_flv.rs | 非 Default 硬编码 | test | 0.5d |
| **S4-T3** | loopback queue_capacity | loopback.rs | 使用字段 | test | 0.3d |
| **S4-T4** | rtmp/rtsp/webrtc 队列透传 | push/pull | 一致 | test | 0.5d |
| **S4-T5** | Readiness 机制 | handles.rs | stub 删除 | test | 0.5d |
| **S4-T6** | RTMP 就绪事件 | push/rtmp.rs | wait_ready 真等 | test | 0.5d |
| **S4-T7** | WebRTC 就绪事件 | push/webrtc.rs | 同上 | test | 0.5d |
| **S4-T8** | T-OPT / T-RDY 测 | tests/* | 绿 | test | 0.5d |

**验收：**

```bash
cargo test -p cheetah-connector --features full --locked options
cargo test -p cheetah-connector --features full --locked ready
```

---

## 阶段 5 — R6：loopback 诚实 / socket-free

**文档**：`07`。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S5-T1** | rustdoc 标明 localhost 默认 | loopback.rs | 文档句存在 | rg | 0.2d |
| **S5-T2** | layer 字段准确 | loopback.rs | 断言 | test | 0.3d |
| **S5-T3** | EngineOnly 路径 | loopback.rs | T-LB-02 | test | 0.5–1d |
| **S5-T4** | preferred_layer 严格失败 | loopback.rs | 无静默降级 | test | 0.3d |
| **S5-T5** | example 打印 layer | examples | 可见 | run | 0.2d |

**验收：**

```bash
cargo test -p cheetah-connector --features full --locked loopback
```

---

## 阶段 6 — R7：metadata 契约

**文档**：`08`。  
**阻塞 STREAM-02，不阻塞 STREAM-01 最小集。**

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S6-T1** | 冻结 MUST / NOT_PRESERVED 表 | 文档或常量 | 表可测 | 人工 | 0.3d |
| **S6-T2** | 可选 flv_ingress 增强 | cheetah-codec | SHOULD 字段 | test | 1–2d |
| **S6-T3** | metadata_conformance 扩展 | tests | T-MD-* | test | 0.5–1d |
| **S6-T4** | rustdoc 链接契约 | connector | 可见 | 人工 | 0.2d |

**验收：**

```bash
cargo test -p cheetah-connector --features full --locked metadata
# 若改 codec：
cargo test -p cheetah-codec --locked
```

---

## 阶段 7 — R8：error map

**文档**：`09`。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S7-T1** | 改 From\<SdkError\> 去 Rtmp 硬编码 | error.rs | rg 清洁 | test | 0.3d |
| **S7-T2** | 统一 map_sdk_error 用法 | handles + adapters | 协议正确 | test | 0.3d |
| **S7-T3** | T-ERR-* | tests | 绿 | test | 0.2d |

**验收：**

```bash
rg -n 'From<SdkError>|Protocol::Rtmp' crates/sdk/cheetah-connector/src/error.rs
cargo test -p cheetah-connector --features full --locked error
```

---

## 阶段 8 — Example / CI / 状态注记

**文档**：`10`。  
**前置**：S1–S3 必须；理想 S4–S7 已合。

| ID | 描述 | 改动文件 | DoD | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| **S8-T1** | example 增量四方向/layer/wait_ready | examples/* | run 0 | run | 0.5d |
| **S8-T2** | CI job 或脚本 | CI / dev-scripts | 可跑 | run | 0.3d |
| **S8-T3** | connector-gaps 状态 R1–R8 | 根 md 或 release | open→done | 人工 | 0.2d |
| **S8-T4** | 总门禁 | — | `10` §8 | 见下 | 0.3d |

**验收：**

```bash
cargo fmt --check
cargo test -p cheetah-connector --features full --locked
cargo run -p cheetah-connector --example external_connector_loopback --features full --locked
```

---

## 1. 风险登记册

| ID | 风险 | 影响 | 缓解 |
| --- | --- | --- | --- |
| R-A | 本地无 connector | 无法开工 | S0 阻塞；先 plan1 |
| R-B | RTSP 无现成 AVFrame 路径 | S2 延期 | module+engine 编排 |
| R-C | WebRTC open_push 工期 | S3 延期 | MVP 单轨+localhost；禁止 fixture 冒充 |
| R-D | supports 先 false 破坏依赖说谎的下游 | 短暂 | 变更说明；尽快接线 |
| R-E | wait_ready 竞态 flaky | CI 红 | 事件驱动+超时；禁纯 sleep |
| R-F | FLV 无法保真 duration | STREAM-02 | NOT_PRESERVED 契约 |
| R-G | 多 agent 改 handles/error | 冲突 | 见文件所有权 |
| R-H | 回退 HTTP-FLV/RTMP | STREAM-01 回退 | 回归测门禁 |

---

## 2. 多 Agent 文件所有权

| 焦点 | 主写 | 避免 |
| --- | --- | --- |
| RTSP | `pull/rtsp.rs`、rtsp 测 | webrtc push |
| WebRTC | `push/webrtc.rs`、webrtc 测 | rtsp pull |
| Options/ready | `options.rs`/`handles.rs`/`loopback.rs` | 大改 codec |
| Metadata | `flv_ingress` + metadata 测 | 改 supports 矩阵 |
| Error | `error.rs` | 无关大重构 |

共享 `connector.rs` / `protocol.rs`：短 PR，先合 R3 再接线。

---

## 3. Agent 交接模板

```markdown
## 900-2 Residual 阶段 S?
- 完成：S?-T?
- 阻塞：…
- 验收命令与结果：…
- supports 矩阵当前值：…
- BYPASS / layer 标注：…
- 勿回退清单是否仍绿：…
- 后续注意：…
```

禁止：

1. 未跑验收标完成。  
2. 用 fixture/SDP 冒充 R2。  
3. 用 engine-only 冒充 R1。  
4. supports 长期说谎。  
5. 为绿测删除 metadata/error 断言。

---

## 4. 发布切片

| 切片 | 包含 | 对外宣称 |
| --- | --- | --- |
| **STREAM-01 MVP** | S0–S3 + S8 最小 | 四方向 connector 可测 |
| **STREAM-01 稳** | +S4 +S5 | 可配置、少 flaky、layer 诚实 |
| **STREAM-02** | +S6 | metadata 契约 |
| **质量** | +S7 | 错误协议字段正确 |

---

## 5. 完成总检

- [ ] R1 RTSP pull  
- [ ] R2 WebRTC push（媒体路径）  
- [ ] R3 supports 诚实  
- [ ] R4 options 透传  
- [ ] R5 wait_ready  
- [ ] R6 layer/socket-free  
- [ ] R7 metadata 契约  
- [ ] R8 error map  
- [ ] 勿回退清单  
- [ ] example + CI  
- [ ] connector-gaps 状态更新  

---

## 6. Residual → 阶段速查

| Residual | 阶段 | 文档 |
| --- | --- | --- |
| R1 | S2 | `03` |
| R2 | S3 | `04` |
| R3 | S1（+S2/S3 翻转） | `05` |
| R4 | S4 | `06` |
| R5 | S4 | `06` |
| R6 | S5 | `07` |
| R7 | S6 | `08` |
| R8 | S7 | `09` |
| 验收 §4 | S8 + 各阶段 | `10` |
