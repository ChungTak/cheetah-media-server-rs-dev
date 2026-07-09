# Cheetah Media Server 审查报告（首轮执行）

> 依据 `dev-docs/ProjectReviewPlan.md` 的阶段划分（P0→P5）执行。审查基线：`AGENTS.md`、
> `SystemArchitecture.md`、README、`config.example.yaml`。
>
> 分类：**违规**（明确违反约束，应修）｜**风险**（架构/一致性隐患，建议修）｜**观察**（记录，不一定要动）。
> 每条给出 `证据(文件:行) + 依据 + 建议动作 + 状态`。本轮已直接修复标注为 **已修复** 的项。

---

## 摘要

| 编号 | 类别 | 简述 | 严重度 | 状态 |
|------|------|------|--------|------|
| F-01 | 工具/CI | `check_runtime_boundaries.sh` 引用旧扁平路径且依赖 PCRE2 → 静默空跑（守卫失效） | 高 | 已修复 |
| F-02 | 违规 | `cheetah-hls-module` 生产路径直接使用 `tokio::time::timeout` | 中 | 已修复 |
| F-03 | 环境 | 代码使用 `is_multiple_of`（Rust 1.87+），环境默认 1.83 无法编译 | 高 | 已修复(工具链)+待固化 |
| F-04 | 违规 | `cheetah-webrtc-module` 生产代码大量直接依赖 `tokio::{net,time,sync}` 与 `tokio::select!` | 高 | 待处理 |
| F-05 | 风险 | `cheetah-http-flv-module` 直接依赖 `cheetah-rtmp-core` 复用 FLV 封装逻辑 | 中 | 待处理 |
| F-06 | 风险 | `cheetah-mp4-module` 生产函数签名暴露 `tokio::sync::mpsc::Receiver` | 中 | 待处理 |
| F-07 | 文档 | `SystemArchitecture.md` 缺 hls/ts/mp4/srt/webrtc 的 Reference Mapping | 中 | 待处理 |
| F-08 | 测试 | `ts` 协议缺 `testing/property-tests`（其他协议均有） | 中 | 待处理 |
| F-09 | 文档/实现 | SystemArchitecture §4 观测性基线指标在代码中完全缺失 | 中 | 待处理 |

---

## P0 架构正确性

### 通过项
- **分层依赖方向**：`cheetah-codec`（Foundation）依赖仅 `base64/bitflags/bytes/smallvec/thiserror`，
  无 tokio/axum/engine/http（`AGENTS.md` §2 ✓）。`cheetah-sdk` 仅依赖 codec/runtime-api/macros，
  不反向依赖协议 module，源码无 axum/tide/actix（§2 ✓）。
- **core Sans-I/O**：11 个协议 core 全部无 `tokio` 依赖；生产代码无 `async fn`、无 `Instant::now()`/
  `SystemTime::now()`（webrtc core 的 `Instant::now()`、mp4 core 的相关引用均位于 `#[cfg(test)]` 或文档注释）
  （`AGENTS.md` §4 ✓）。
- **命名**：全仓无自有 `plugin` 命名残留；仅 webrtc 互操作测试中出现对外部 Janus `janus.plugin.*` 的引用
  （合理）（§1 ✓）。
- **三段式完整性**：rtmp/rtsp/http-flv/hls/ts/fmp4/mp4/rtp/gb28181/srt/webrtc 均具备
  `core + driver-tokio + module`（§3 ✓）。
- **模块跨协议依赖**：rtsp module 对 rtmp 的依赖仅在 `[dev-dependencies]`（互操作测试，合理）。

### F-01【已修复｜高】runtime 边界 CI 守卫失效
- 证据：`dev-scripts/check_runtime_boundaries.sh`（原文件）引用 `crates/cheetah-runtime-api/src` 等
  **重组前的扁平路径**（现实为 `crates/runtime/cheetah-runtime-api/src` 等），且使用 `rg --pcre2`；
  本机 ripgrep 无 PCRE2 → 每个检查 `rg` 报错被 `if` 视为 false，最终输出
  `all runtime boundary checks passed` 却**什么都没扫**。
- 依据：`SystemArchitecture.md` §5（此脚本是 runtime 中立性 CI guard）。
- 修复：更新为重组后的真实路径；将模式改写为默认 Rust 正则（不再依赖 PCRE2）；新增“路径不存在即
  报错退出”的自检，避免以后 crate 移动再次静默空跑。修复后对 rtmp 实测通过，并验证能检出 hls 的
  `tokio::time` 违规（守卫恢复有效）。

### F-07【待处理｜中】架构文档缺协议映射
- 证据：`SystemArchitecture.md` §3–§3.4 仅覆盖 RTMP/RTSP/HTTP-FLV/fMP4/RTP/GB28181；workspace 与 README
  已含 `hls`、`ts`、`mp4`、`srt`、`webrtc`，但无对应 Reference Mapping。
- 依据：`AGENTS.md` §13（feature/crate 变更须同步文档）。
- 建议：补 hls/ts/mp4/srt/webrtc 的 crate 映射、capability snapshot、boundary clarification。

---

## P1 基础模块（`cheetah-codec` + runtime）

### 通过项
- codec 依赖零 runtime/框架泄漏；公共接口未见 FFmpeg 类型（§7 ✓）。
- 上界防护存在且集中：`ts_demux`/`ps`/`rtp`/`fmp4` 等有 `max_*` 上限（README 各模块 `max_reassembly_bytes`
  4MB 等）。
- runtime 抽象：`cheetah-runtime-api` 公共接口 runtime 中立，未见 `pub` 暴露 `tokio::*`；hls 修复后统一走
  `RuntimeApi::{now,sleep_until}` + `futures::select_biased!`（§5 ✓）。

### F-03【已修复(工具链)+待固化｜高】工具链版本要求未被环境满足
- 证据：`crates/foundation/cheetah-codec/src/egress.rs:123,127`、`rtp.rs:290` 使用
  `u*::is_multiple_of(..)`，该 API 在 **Rust 1.87** 稳定；环境默认 `rustc 1.83.0` 编译报
  `E0658 unstable library feature 'unsigned_is_multiple_of'`，整个 codec 及依赖它的一切无法构建。
- 处理：本会话 `rustup update stable` 升级至 `rustc 1.96.1` 后全量构建通过（含
  `cargo build -p cheetah-server --features "rtsp,http-flv,hls,fmp4,ts,rtp,gb28181"`）。
- 建议（待固化）：仓库未提供 `rust-toolchain.toml`。建议新增 `rust-toolchain.toml` 固定最低 stable
  版本（≥1.87），并在环境 blueprint 中安装对应工具链，避免新会话/CI 因默认工具链过旧而失败。

### F-09【待处理｜中】观测性基线指标缺失
- 证据：`SystemArchitecture.md` §4 要求运行报告暴露 `startup_latency_ms`、
  `first_second_avg_frame_interval_ms`、`average_playback_rate_x`、`first_keyframe_delay_ms`，
  并按层输出 `source_repair_events`/`canonical_repair_events`/`egress_repair_events` 及
  `REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD`。全仓 grep 这些标识符命中数均为 **0**。
- 现状：`cheetah-codec` 存在时间戳/参数集修复逻辑（如 `egress.rs` 的 `repair_count`、
  `repair_h26x_keyframe_frame`），但没有文档所述的三层分类与命名指标。
- 依据：`SystemArchitecture.md` §4 + `AGENTS.md` §13。
- 建议：要么实现该观测性基线，要么把该章节标记为路线图/未实现，保持文档与实现一致。

---

## P2 系统层（sdk / config / control / engine / record）

### 通过项
- `cheetah-sdk` 提供 `RuntimeApi`、`CancellationToken`、`OneShotReceiver`、`EngineContext`、`StreamKey`
  等 runtime-neutral 抽象；HTTP module 契约未绑定具体 Web 框架（§2/§5 ✓）。
- `cheetah-record-module` 未见直接 `tokio::{net,time,sync}` 使用（0 命中）。

### 待深入项（本轮未逐行确认，列为下一轮重点）
- `cheetah-engine/src/stream.rs`(1770 行)：单发布者租约独占语义、Dispatcher/RingBuffer/subscriber queue
  的“慢订阅者隔离”与上界（`AGENTS.md` §6/§9），建议逐行核对。
- `module_manager.rs`：`restart_module/restart_modules` 仅接受 `Running`、否则 `Conflict`（§6）。
- `cheetah-config`：加载顺序（默认→`CHEETAH_CONFIG`→`M7S_`）与 `config.example.yaml` 字段一致性。

---

## P3 各协议

### 通过/观察
- 各 core 保持 Sans-I/O（见 P0）。
- rtmp/rtsp/ts/fmp4/rtp/gb28181/srt module 生产代码无禁用 tokio 原语直用（0 命中）。

### F-02【已修复｜中】hls module 生产路径直用 `tokio::time`
- 证据：`crates/protocols/hls/module/src/pull.rs:169`（修复前）
  `tokio::time::timeout(read_timeout, stream.read(..))`。
- 依据：`AGENTS.md` §6（module 不得直接依赖 `tokio::time`，多路等待走 runtime-neutral 原语）。
- 修复：改用 `RuntimeApi::{now,sleep_until}` + `futures::{pin_mut,select_biased,FutureExt}`（与同文件
  playlist 轮询处一致的模式）；同时移除 `cheetah-hls-module` 已不再需要的直接 `tokio` 依赖。
  `cargo clippy -p cheetah-hls-module` 干净、`cargo test -p cheetah-hls-module` 35 passed。

### F-04【待处理｜高】webrtc module 大量直用 tokio 原语
- 证据：`cheetah-webrtc-module` 生产代码 ~81 处 `tokio::{net,time,sync}::` 与 `tokio::select!`，
  例如 `src/module.rs:236,262,1006,1009`、`src/http_client.rs:36,192,205`、`src/p2p/*.rs`（server/hub/
  bridge/transport/ws/supervisor）。
- 依据：`AGENTS.md` §6 + `SystemArchitecture.md` §5（module 必须 runtime 中立，多路等待禁止
  `tokio::select!`）。
- 影响：这是当前最集中的架构违规，且 WebRTC 亦未在 SystemArchitecture 记录（F-07）。
- 建议：不宜在本轮一次性重写。建议单独立项：将网络/定时/信令 I/O 下沉到
  `cheetah-webrtc-driver-tokio`，module 侧改走 `RuntimeApi`/SDK 抽象；把 `tokio::select!` 替换为
  `CancellationToken` + futures 组合子。

### F-05【待处理｜中】http-flv module 依赖 rtmp-core 复用 FLV 封装
- 证据：`crates/protocols/http-flv/module/Cargo.toml` 有 `cheetah-rtmp-core` 生产依赖；
  `src/module.rs:14` 导入 `build_track_bootstrap_payloads / map_frame_to_rtmp_flv_payload /
  track_list_has_audio / RtmpFlvPayloadKind / RtmpFlvPlayMode`。
- 依据：`AGENTS.md` §2（feature module 只应经 `cheetah-sdk`+`cheetah-codec` 交互）、§7（FLV 封装属
  `cheetah-codec`，已有 `flv.rs`，不应各协议自持）。
- 建议：把 FLV 帧↔payload 映射与 bootstrap 逻辑收敛到 `cheetah-codec`，rtmp 与 http-flv 共同复用，
  解除 http-flv→rtmp-core 的跨协议依赖。

### F-06【待处理｜中】mp4 module 暴露 tokio 通道类型
- 证据：`crates/protocols/mp4/module/src/api.rs:293`
  `async fn bridge_events(mut events: tokio::sync::mpsc::Receiver<VodDriverEvent>, ..)`。
- 依据：`AGENTS.md` §6。
- 建议：driver 侧以 runtime-neutral 接收端（SDK 抽象 / trait 对象）暴露事件流，module 不直接持有
  `tokio::sync::mpsc::Receiver`。

### F-08【待处理｜中】ts 协议缺属性测试
- 证据：`crates/protocols/ts/` 下无 `testing/property-tests`（其余协议均有；workspace members 也无
  `cheetah-ts-property-tests`）。
- 依据：`AGENTS.md` §11 + `SystemArchitecture.md` §6（core 应有属性测试）。
- 建议：补 `crates/protocols/ts/testing/property-tests`，覆盖 TS 包解析/PAT-PMT/重组上界。

---

## P4 跨协议与非功能性（本轮抽样）

- **观测性**：见 F-09（基线指标缺失）。
- **跨协议桥接**：`dev-scripts/cross_protocol_matrix_*` 与 `mp4` 的 `bridge_events`（→RTSP/RTMP/HTTP-FLV）
  显示桥接路径存在；完整接受矩阵回归留待下一轮实跑。
- **构建健康**：升级工具链后 `cheetah-server` 全协议 feature 组合构建通过。

---

## P5 结论与后续

**本轮已落地修复：**
1. F-01 修复 `check_runtime_boundaries.sh`（恢复守卫有效性）。
2. F-02 修复 hls module runtime 中立性违规 + 清理无用 tokio 依赖。
3. F-03 工具链升级至满足 `is_multiple_of` 的 stable（1.96），全量构建通过。

**建议后续立项（按优先级）：**
- F-04 webrtc module runtime 中立化（工作量最大，单独 PR）。
- F-03 固化：新增 `rust-toolchain.toml` + blueprint 安装新 stable。
- F-05 FLV 封装收敛到 codec，解除 http-flv→rtmp-core 依赖。
- F-07 / F-09 文档与实现对齐（补协议映射 / 观测性章节标注）。
- F-06 mp4 桥接通道去 tokio 化；F-08 补 ts 属性测试。
- P2 engine 租约/队列上界逐行核对（下一轮重点）。

**复现命令：**
```bash
bash dev-scripts/check_runtime_boundaries.sh
cargo test -p cheetah-hls-module
cargo build -p cheetah-server --features "rtsp,http-flv,hls,fmp4,ts,rtp,gb28181"
```
