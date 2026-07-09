# Phase 06: Driver 与 Module 集成回归

- 状态：已完成（任务 1-4 已完成）
- 范围：将 vendor 客户端连接语义映射到本地服务端主线，补全 driver 分段收包与 module 业务回归。
- 对应用例：`fuzz_rtsp_client.rs`、`fuzz_rtsp_limits.rs` 的连接语义部分，以及本地 RTSP 集成场景测试。
- 完成标准：driver/module 在主线场景下稳定承接 core 原语，无退化、无重复解析、无明显边界漂移。

## 目标

- 把 vendor `RtspClientConnection` 所覆盖的“接收缓冲、mixed RTSP/interleaved 数据、限制、状态更新、不崩溃”语义映射为本地 server-side 行为测试。
- 在不新增独立客户端对外 API 的前提下，把连接级健壮性校验做实到 driver 与 module 主线中。

## 具体任务

### 1. `cheetah-rtsp-driver-tokio` 集成测试补齐（已完成）

新增测试场景，覆盖：

- [x] 同一 TCP 段中同时包含 RTSP 消息与 interleaved 数据。
- [x] 多段输入拼接成一条完整 RTSP 消息。
- [x] 部分 interleaved frame 留在缓冲区，后续数据补齐后成功解析。
- [x] 限制命中时连接被关闭，关闭原因可追踪。
- [x] 写队列溢出时连接被强制关闭且不会污染其他连接。
- [x] peer close、command close、cancel close 的行为分离。

### 2. `cheetah-rtsp-module` 集成测试拆分与补强（已完成）

现有 `tests/keepalive.rs` 过大，迁移中拆分为更清晰的场景：

- [x] 基础 keepalive：
  - `GET_PARAMETER` / `SET_PARAMETER`
- [x] 发布主线：
  - `ANNOUNCE -> SETUP -> RECORD`
  - `PAUSE -> RECORD` 连续性
- [x] 播放主线：
  - `DESCRIBE -> SETUP -> PLAY`
  - `PAUSE -> PLAY` 恢复
  - `TEARDOWN`
- [x] UDP/TCP 差异：
  - interleaved RTP/RTCP
  - UDP RTP/RTCP continuity
- [x] 多播放器/多轨：
  - 一个播放器关闭不影响另一个
  - 双轨 `RTP-Info`
  - BYE 发射次数与 Range 保留

### 3. vendor 连接语义映射原则（已完成）

- [x] `feed_recv_buf` 的分段与混包语义：映射到 driver 连接循环测试。
- [x] `state`、`session_id` 变化：映射到 module 会话状态与响应头校验。
- [x] `pending_methods` 触发的状态转换：映射为模块业务状态机与响应驱动的断言。
- [x] `redirect` 这类客户端特有消费语义：本地主线暂无消费需求，仅保留 core/response 表达能力，不向 module 引入客户端重定向消费状态。

### 4. Fuzz 映射（已完成）

- [x] `fuzz_rtsp_core.rs`：用本地 `RtspCore` / message decoder 作为主入口，不断喂入随机字节与随机 command。
- [x] `fuzz_rtsp_limits.rs`：在小限制配置下反复喂入随机 RTSP / interleaved 数据，验证不 panic。
- [x] driver 不单独做复杂网络 fuzz；其稳健性主要通过 core fuzz + 集成测试组合保证。

## 完成判定

- driver 集成测试覆盖分段、混包、close、overflow、limit。
- module 集成测试按场景拆分完成，旧大文件中的关键主线全部有等价新测试承接。
- 现有 keepalive、publish/play/udp/multitrack 主线回归通过。
- 相关注释已全部翻译为中文。

## 最新进展

- 2026-04-19：已完成任务 4（Fuzz 映射）：重构 `crates/cheetah-rtsp-fuzz/fuzz_targets/common.rs`，统一 `RtspCore` 分块喂入、随机 `RtspCommand` 构造、request/response message decoder 稳健性驱动与 RTSP/interleaved 混合输入构造；`fuzz_rtsp_core.rs` 与 `fuzz_rtsp_limits.rs` 改为复用该入口，在默认限制与小限制配置下反复喂入随机字节和混合协议负载，覆盖命令发送、限制命中、解析失败容忍（允许报错但不得 panic）等路径。已完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-fuzz`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo clippy -p cheetah-rtsp-driver-tokio --tests`、`cargo clippy -p cheetah-rtsp-module --tests`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo test` 回归；后续进入 Phase 07（注释中文化与收口）。
- 2026-04-19：已完成任务 3（vendor 连接语义映射原则收口）：新增 `tests/state_mapping.rs::failed_transition_and_session_mismatch_do_not_corrupt_play_session`，覆盖播放态下“非法状态迁移请求（`RECORD`）返回 455 不改变既有播放任务”以及“`Session` 不匹配返回 454 且不污染后续正确请求”的响应驱动语义；同时对 `tests/common/mod.rs::read_response` 做通用修复，支持在同一 TCP 流中跳过前置 interleaved 帧后再增量解析 RTSP 响应，避免播放态测试把 `$` 帧误解为 RTSP 头。已完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-module --tests`、`cargo clippy -p cheetah-rtsp-driver-tokio --tests`、`cargo test -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；后续进入任务 4（Fuzz 映射）。
- 2026-04-19：已完成任务 2（`cheetah-rtsp-module` 集成测试拆分与补强）：补充 `tcp_interleaved_play_pause_play_rtp_rtcp_continuity`，覆盖 TCP interleaved 下 `PLAY -> PAUSE -> PLAY` 的 RTP/RTCP 连续性与恢复；同时修复 `tests/common/mod.rs` 中 `read_interleaved_frame` 的通用读帧缺陷（原实现会在同次 `read` 含多帧时丢弃后续帧），改为基于 `read_exact` 的按帧读取并补齐带阶段标签的超时/IO 错误信息。已完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-module --tests`、`cargo test -p cheetah-rtsp-module`、`cargo clippy -p cheetah-rtsp-driver-tokio --tests`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml` 回归；后续进入任务 3（vendor 连接语义映射原则收口）。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-rtsp-driver-tokio --tests`
- `cargo clippy -p cheetah-rtsp-module --tests`
- `cargo test -p cheetah-rtsp-driver-tokio`
- `cargo test -p cheetah-rtsp-module`
- `cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`
