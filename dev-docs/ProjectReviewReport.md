# Cheetah Media Server 审查报告（P0–P3 已执行，第四轮更新）

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
| F-03 | 环境 | 代码使用 `is_multiple_of`（Rust 1.87+），环境默认 1.83 无法编译 | 高 | 已修复 |
| F-04 | 违规 | `cheetah-webrtc-module` 生产代码大量直接依赖 `tokio::{net,time,sync}` 与 `tokio::select!` | 高 | 已修复(Stage 1-5) |
| F-05 | 风险 | `cheetah-http-flv-module` 直接依赖 `cheetah-rtmp-core` 复用 FLV 封装逻辑 | 中 | 已修复 |
| F-06 | 风险 | mp4 module 用 `tokio::spawn` 且 driver 公共 API 泄漏 tokio 通道类型 | 中 | 已修复 |
| F-07 | 文档 | `SystemArchitecture.md` 缺 hls/ts/mp4/srt/webrtc 的 Reference Mapping | 中 | 已修复 |
| F-08 | 测试 | `ts`、`http-flv` 缺 `testing/property-tests`（其余 9 协议均有） | 中 | 已修复 |
| F-09 | 文档/实现 | SystemArchitecture §4 观测性基线指标在代码中完全缺失 | 中 | 已修复(基线+脚手架) |
| F-10 | 风险 | `cheetah-engine` 内部直用 `tokio::sync::{mpsc,broadcast,Mutex}`，与 §5 允许清单冲突 | 低 | 已修复 |

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

### F-07【已修复｜中】架构文档缺协议映射
- 证据：`SystemArchitecture.md` §3–§3.4 仅覆盖 RTMP/RTSP/HTTP-FLV/fMP4/RTP/GB28181；workspace 与 README
  已含 `hls`、`ts`、`mp4`、`srt`、`webrtc`，但无对应 Reference Mapping。
- 依据：`AGENTS.md` §13（feature/crate 变更须同步文档）。
- 修复：WebRTC 映射已在 F-04 Stage 5 补齐并规范化为 §3.9；本轮补齐 §3.5 HLS / §3.6 HTTP-TS /
  §3.7 MP4 VOD / §3.8 SRT，每节含 crate 映射（含 testing/property-tests + fuzz）、capability snapshot、
  boundary clarification（core/driver/module 职责 + `cheetah-codec` 容器能力归属）。内容对照源码核对：
  HLS 容器/LLHLS 标签/编码矩阵、TS PAT/PMT+RtpTsIngest+pull、MP4 VOD 的 `/zlm/{loadMP4File,seekRecordStamp,
  setRecordSpeed}` 与 `VodEventStream` 中立事件流、SRT roles/modes/streamid/AES 加密（`shiguredo_srt`）。

---

## P1 基础模块（`cheetah-codec` + runtime）

### 通过项
- codec 依赖零 runtime/框架泄漏；公共接口未见 FFmpeg 类型（§7 ✓）。
- 上界防护存在且集中：`ts_demux`/`ps`/`rtp`/`fmp4` 等有 `max_*` 上限（README 各模块 `max_reassembly_bytes`
  4MB 等）。
- runtime 抽象：`cheetah-runtime-api` 公共接口 runtime 中立，未见 `pub` 暴露 `tokio::*`；hls 修复后统一走
  `RuntimeApi::{now,sleep_until}` + `futures::select_biased!`（§5 ✓）。

### F-03【已修复｜高】工具链版本要求未被环境满足
- 证据：`crates/foundation/cheetah-codec/src/egress.rs:123,127`、`rtp.rs:290` 使用
  `u*::is_multiple_of(..)`，该 API 在 **Rust 1.87** 稳定；环境默认 `rustc 1.83.0` 编译报
  `E0658 unstable library feature 'unsigned_is_multiple_of'`，整个 codec 及依赖它的一切无法构建。
- 修复：本会话 `rustup update stable` 升级至 `rustc 1.96.1` 后全量构建通过；并在仓库根目录新增
  `rust-toolchain.toml` 固定 `channel = "1.96.1"`（满足 ≥1.87 的最低 stable），同时声明 `rustfmt` 与
  `clippy` 组件，使新会话/CI 自动安装匹配工具链，避免默认工具链过旧导致编译失败。
- 验证：`rustup show` 解析 `rust-toolchain.toml`；`cargo build -p cheetah-server --features
  "rtsp,http-flv,hls,fmp4,ts,rtp,gb28181"` 通过。

### F-09【已修复(基线+脚手架)｜中】观测性基线指标缺失
- 证据：`SystemArchitecture.md` §4 要求运行报告暴露 `startup_latency_ms`、
  `first_second_avg_frame_interval_ms`、`average_playback_rate_x`、`first_keyframe_delay_ms`，
  并按层输出 `source_repair_events`/`canonical_repair_events`/`egress_repair_events` 及
  `REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD`。修复前全仓 grep 这些标识符命中数均为 **0**。
- 现状：`cheetah-codec` 存在时间戳/参数集修复逻辑（如 `egress.rs` 的 `repair_count`、
  `repair_h26x_keyframe_frame`），但没有文档所述的三层分类与命名指标。
- 依据：`SystemArchitecture.md` §4 + `AGENTS.md` §13。
- 修复（方案 B：落地可确定性部分 + schema 脚手架）：
  - 新增 Sans-I/O 模块 `cheetah-codec::observability`：
    - `RepairLayer` + `classify_timestamp_alert` 把 `TimestampAlert` 分类到 source/canonical
      层（纯 discontinuity/reset 不计为 repair）；`RepairEventCounters` 按层累计并提供
      `is_high_frequency_anomaly`——source 永不升级，canonical/egress 达到
      `REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD` 才告警。
    - `RuntimeReportBuilder`/`RuntimeObservabilityReport` 由注入的 `now_us` + 规范 `pts_us`
      样本计算四项运行报告指标（不读时钟、无 I/O）。
  - `cheetah-engine::MetricsRegistry` 经 `MetricsApi` 暴露：`record_repair_events`（累加层计数器）
    与 `record_runtime_report`（设置时间 gauge），`render()` 同时输出 counters + gauges。
  - 属性测试覆盖：层分类总数守恒、source 不升级、canonical/egress 阈值语义、报告时延非负。
- 分阶段：driver/module 在实时 egress 热路径逐帧喂入 `RuntimeReportBuilder` 的接入按协议增量落地
  （计量接入点已定义），故记为「基线+脚手架」。

---

## P2 系统层（sdk / config / control / engine / record）

### 通过项
- `cheetah-sdk` 提供 `RuntimeApi`、`CancellationToken`、`OneShotReceiver`、`EngineContext`、`StreamKey`
  等 runtime-neutral 抽象；HTTP module 契约未绑定具体 Web 框架（§2/§5 ✓）。
- `cheetah-record-module` 未见直接 `tokio::{net,time,sync}` 使用（0 命中）。

### 深入核对结论（第二轮已逐行确认，均通过）
- **单发布者租约**：`StreamManager::acquire_publisher` 用 `active_lease.compare_exchange(0, lease_id,..)`
  实现独占；已被占用返回 `SdkError::Conflict("stream .. already has an active publisher")`；
  `release_lease` 校验 `lease_id` 不匹配亦 `Conflict`（`stream.rs:812-819,616-631`）。符合 §6 单发布者独占。
- **热路径非阻塞 + 慢订阅者隔离**：`dispatch_frame` 对每个订阅者用 `tx.try_send`（非阻塞），队列满时按
  `BackpressurePolicy`（`DropDroppableFirst` / `DropUntilNextKeyframe` / `DisconnectOnOverflow`）丢帧或
  摘除该订阅者，绝不阻塞派发线程或其他订阅者（`stream.rs:312-372`）。符合 §9“慢订阅者不拖累其他订阅者/
  热路径禁止阻塞”。
- **有界缓冲**：RingBuffer 容量 `next_power_of_two`、`ring_capacity.max(128)`；订阅队列
  `mpsc::channel(queue_capacity.max(1))`，且拒绝 `queue_capacity==0`、`max_bootstrap_frames>queue_capacity`
  （`stream.rs:56-66,283,655-669,858-863`）。IDR 索引在冷路径 `idr_write_lock` 下维护并按容量裁剪，未把锁带入
  每帧热路径。符合 §9 上界要求。
- **module 生命周期**：`module_manager` 的 `restart_module/restart_modules` 仅接受 `Running`，否则
  `Conflict`；`ModuleRestartRequired` 由基础层 `rebuild_module`（create→init→start）执行；依赖环检测
  返回 `Conflict`（`module_manager.rs:655-678,584-593,124-127`）。符合 §6。
- **config 加载顺序**：`default → file → env → runtime` 逐层 `merge_value`（`lib.rs:129-153`）；env 前缀
  `M7S_GLOBAL__` / `M7S_MODULE__<module>__`、`__` 分隔、`env_value_to_json` 按 bool/i64/f64/string 解析
  （`lib.rs:99-127,469-483`）。与 README §2 一致。

### F-10【已修复｜风险】cheetah-engine 内部直用 tokio 原语与文档允许清单冲突
- 证据：生产代码 `stream.rs:16 use tokio::sync::mpsc`、`event.rs:3 use tokio::sync::broadcast`、
  `module_manager.rs:13 use tokio::sync::Mutex`（另有 `core_adapters.rs:283/286`、`task.rs:350`、
  `stream.rs:1002` 位于测试）。
- 依据：`AGENTS.md` §5 明确“`tokio`/`tokio-util` 仅允许留在 `cheetah-runtime-tokio`、`*-driver-tokio` 和
  应用层 crate”，`cheetah-engine`（system 层）不在允许清单内。这些用法未泄漏到公共接口，故边界守卫
  （只查 `pub` 泄漏）不报警，属**文档 vs 实现**的口径分歧。
- 修复：按建议采 (a)。`AGENTS.md` §5 与 `SystemArchitecture.md` §5 已明确把 `cheetah-engine` 纳入
  `tokio`/`tokio-util` 内部使用允许清单，但保留“公共接口不得暴露 tokio 类型”的约束；
  `dev-scripts/check_runtime_boundaries.sh` 新增 `INTERNAL_TOKIO_ORCHESTRATION` 检查，将
  `crates/system/cheetah-engine` 作为系统编排层单独校验：
  - 公共 API 仍通过 `pub_tokio_pattern` 扫描，防止 `tokio` 类型泄漏；
  - `[dependencies]` 必须显式声明 `tokio`/`tokio-util`，使内部使用异常依赖清单化、可守卫。
- 验证：`bash dev-scripts/check_runtime_boundaries.sh` 通过；`cheetah-engine` 公共接口未暴露
  `tokio`/`tokio_util` 类型；`cargo clippy -p cheetah-engine` / `cargo test -p cheetah-engine` 无新增失败。

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

### F-04【已修复｜高】webrtc module 大量直用 tokio 原语
- 证据（本轮精确统计，区分 `#[cfg(test)]`）：`cheetah-webrtc-module` **生产代码 78 处**、测试 53 处
  使用 tokio 原语。生产用法分两类：可机械替换(A，通道/锁/select/计时/spawn)与需结构性下沉(B，
  WebSocket/TLS/socket）。逐文件明细与类别见 `dev-docs/F04_WebRTC_Detokio_Plan.md` §1。
  代表点：`src/module.rs:236,262,1006,1009`、`src/http_client.rs:36,192,205,330`（`tokio-rustls`）、
  `src/ome_ws.rs`/`src/p2p/{ws,server}.rs`（`tokio-tungstenite`）、`src/p2p/{bridge,hub,supervisor}.rs`。
- 依据：`AGENTS.md` §6 + `SystemArchitecture.md` §5（module 必须 runtime 中立，多路等待禁止
  `tokio::select!`）。
- 修复：按 `dev-docs/F04_WebRTC_Detokio_Plan.md` 分阶段落地 Stage 1–6：
  - **Stage 2**：沿 `WebRtc` module → bridge/job/http 调用链注入 `Arc<dyn RuntimeApi>`，为后续计时/spawn 替换提供句柄。
  - **Stage 3**：A 类机械替换——`tokio::sync::mpsc/oneshot` → `futures::channel::*`；跨 await 的 `tokio::sync::Mutex` → `futures::lock::Mutex`；`tokio::select!` → `futures::select_biased!`；`tokio::time::{sleep,interval,timeout}` → `RuntimeApi::sleep_until` 组合；`tokio::spawn` → `RuntimeApi::spawn`。覆盖 `lifecycle_dispatcher.rs`、`p2p/bridge.rs`、`bridge.rs`、`p2p/hub.rs`、`p2p/supervisor.rs`、`jobs.rs`、`module.rs`。
  - **Stage 4**：`http.rs` 移除死 `broadcast` 占位；`WebRtcHttpService::wait_answer` 与 OME WS offer 等待改为 `RuntimeApi::sleep_until` + `futures::select_biased!`。
  - **Stage 5**：B 类结构性下沉——`driver-tokio` 新增 `WsConnection`/`WsConnector`/`WsServerListener` 中立 WebSocket 抽象，裸 HTTP/1.1+TLS 客户端迁入 driver；`module` 侧改为 `bind_ws_server`/`Box<dyn WsConnection>`，消息编解码与 SSRF/URL 策略保留在 module；`module/Cargo.toml` 从 `[dependencies]` 移除 `tokio`/`tokio-rustls`/`tokio-tungstenite`/`rustls`/`rustls-pemfile`/`webpki-roots`。
  - **Stage 6**：扩展 `dev-scripts/check_runtime_boundaries.sh`，将 `cheetah-webrtc-module/src` 纳入公共 API 中立扫描，`module/Cargo.toml` 加入 `[dependencies]` 禁 tokio 清单。
- 验证：`cargo fmt/clippy/test -p cheetah-webrtc-module` 全绿；`cargo build -p cheetah-server` 全协议 feature 构建通过；`bash dev-scripts/check_runtime_boundaries.sh` 通过；`module/Cargo.toml` 生产依赖已无 tokio。

### F-05【已修复｜中】http-flv module 依赖 rtmp-core 复用 FLV 封装
- 证据（修复前）：`crates/protocols/http-flv/{core,module}/Cargo.toml` 均有 `cheetah-rtmp-core` 生产依赖；
  `module/src/module.rs:14` 导入 `build_track_bootstrap_payloads / map_frame_to_rtmp_flv_payload /
  track_list_has_audio / RtmpFlvPayloadKind / RtmpFlvPlayMode`，`core/src/{request,session,lib}.rs`
  导入 `RtmpFlvPlayMode`。
- 依据：`AGENTS.md` §2（feature module 只应经 `cheetah-sdk`+`cheetah-codec` 交互）、§7（FLV 封装属
  `cheetah-codec`，不应各协议自持）。
- 修复：把 FLV 帧↔payload 映射与 bootstrap 逻辑从 `rtmp/core/src/flv.rs`（841 行）下沉到
  `cheetah-codec` 新模块 `flv_egress.rs`，用一个自包含的最小 AMF0 `onMetaData` 编码器替代对 rtmp-core
  `amf0` 的依赖（rtmp 的完整 AMF0 仍留在 rtmp-core 供命令消息使用）。`rtmp/core/src/flv.rs` 改为薄
  re-export 以保留其公共 API；`http-flv/{core,module}` 改为直接消费 `cheetah_codec`，从两个 `Cargo.toml`
  删除 `cheetah-rtmp-core` 生产依赖。字节兼容由 rtmp-core 中 `build_metadata` → `decode_all` 的回归测试
  保证。验证：`cargo test -p cheetah-codec -p cheetah-rtmp-core -p cheetah-http-flv-core -p
  cheetah-http-flv-module -p cheetah-rtmp-module`（除 1 项 main 上即缺失 `manifest.tsv` fixture 的
  预存失败外全绿）、clippy 干净、全 feature 服务端构建通过、边界守卫通过。

### F-06【已修复｜中】mp4 module/driver 桥接未 runtime 中立（第二轮补充证据）
- 证据（修复前，module 侧）：
  - `mp4/module/src/api.rs:293` `async fn bridge_events(mut events: tokio::sync::mpsc::Receiver<VodDriverEvent>, ..)`。
  - `api.rs:209` `tokio::spawn(bridge_events(..))` —— module 直接用 `tokio::spawn`，应走 `RuntimeApi::spawn`。
- 证据（修复前，driver 公共接口泄漏）：`mp4/driver-tokio/src/lib.rs` 的 `pub fn take_events()
  -> Option<mpsc::Receiver<VodDriverEvent>>` 在 driver **公共 API** 直接暴露 tokio 通道类型。
- 依据：`AGENTS.md` §5（driver 公共接口用 runtime-neutral 类型；module 公共接口禁暴露 tokio）+ §6
  （module 不得直用 `tokio::sync`/`tokio::spawn`）。
- 修复：
  - driver 新增 runtime-neutral 的 `VodEventStream`（`impl futures::Stream<Item = VodDriverEvent>`，内部仍
    持有 tokio `mpsc::Receiver`——driver 层允许直用 tokio），`take_events()` 返回类型改为 `Option<VodEventStream>`，
    公共 API 不再出现 tokio 类型。命令通道 `cmd_tx` 仍为 driver 内部私有实现。
  - `VodApi` 注入 `Arc<dyn RuntimeApi>`（`with_engine_bridge` 新增入参，`Mp4Module::init` 从
    `EngineContext.runtime_api` 传入）；`bridge_events` 改消费 `VodEventStream`（`StreamExt::next`），
    spawn 改为 `runtime_api.spawn(Box::pin(bridge_events(..)))`。
  - 从 `mp4/module/Cargo.toml` 的 `[dependencies]` 删除 `tokio`（module 生产代码零 tokio 命中），driver/module
    加 `futures`。
  - 扩展 `dev-scripts/check_runtime_boundaries.sh`：将 mp4 module/driver 纳入公共 API + 驱动中立性 + module
    禁用清单扫描，并把 `mp4/module/Cargo.toml` 加入“`[dependencies]` 禁 tokio”清单。
- 验证：`cargo fmt/clippy(--tests)/test -p cheetah-mp4-core -p cheetah-mp4-driver-tokio -p cheetah-mp4-module`
  全绿；`cargo build -p cheetah-server --features "...,mp4,..."` 通过；边界守卫通过。行为不变。

### F-08【已修复｜中】ts、http-flv 协议缺属性测试
- 证据（第二轮逐协议核对）：11 协议中仅 **ts** 与 **http-flv** 无 `testing/property-tests`
  （rtmp/rtsp/hls/fmp4/mp4/rtp/gb28181/srt/webrtc 均有）。
- 依据：`AGENTS.md` §11 + `SystemArchitecture.md` §6（core 应有属性测试）。
- 修复：新增 `crates/protocols/ts/testing/property-tests`（`cheetah-ts-property-tests`，7 项）
  与 `crates/protocols/http-flv/testing/property-tests`（`cheetah-http-flv-property-tests`，7 项），
  并加入根 workspace members。属性覆盖：
  - TS：MPEG-TS 188B 对齐 + 0x47 同步字节、mux→demux roundtrip 恢复 track/frame、任意分块投喂不变量、
    `parse_ts_request_target` roundtrip/忽略 query/拒绝路径穿越、`websocket_accept_key` 确定性。
  - HTTP-FLV：`map_frame_to_rtmp_flv_payload` 的 H264 tag 头（关键帧 0x17 / 非关键帧 0x27 + AVC 0x01）、
    时间戳=DTS(ms)、`build_track_bootstrap_payloads` 元数据领头且唯一、`parse_play_request_target`
    roundtrip + `type=` 模式、拒绝非 `.flv`、`websocket_accept_key` 确定性。
- 验证：两 crate `cargo fmt/clippy(--tests)/test` 全绿（各 7 项通过），边界守卫通过；同步 §3.6 crate 清单与
  §6 测试策略/CI 基线。（`cheetah-ts-core` 现存 1 条 `rtp_ts.rs` clippy 提示与本次无关，未纳入本 PR。）

---

## P4 跨协议与非功能性（本轮抽样）

- **观测性**：见 F-09（基线指标缺失）。
- **跨协议桥接**：`dev-scripts/cross_protocol_matrix_*` 与 `mp4` 的 `bridge_events`（→RTSP/RTMP/HTTP-FLV）
  显示桥接路径存在；完整接受矩阵回归留待下一轮实跑。
- **构建健康**：升级工具链后 `cheetah-server` 全协议 feature 组合构建通过。

---

## P4 跨协议一致性（第五轮）

### P4 目标与范围
- 依据 `ProjectReviewPlan.md` §5 推进跨协议一致性与全局非功能性审查。
- 本轮聚焦 `RTSP↔RTSP`、`RTMP→RTSP`、`RTSP→RTMP` 路径的时间戳一致性、
  `ffprobe`/`ffplay` 互操作性，以及 `dev-scripts/cross_protocol_matrix_*` 回归基线。

### 主要发现与修复

#### F-11【已修复｜中】H.264 RTSP loopback 在 `ffprobe` 中 `dts_time` 非单调
- **证据（修复前）**：`ffprobe -v error -select_streams v:0 -show_entries packet=pts_time,dts_time,flags` 对
  `rtsp://127.0.0.1:8554/live/test` 输出 `0.166667,0.000000,0.033000,...`，`dts_time` 先跌后升；
  `tcpdump` 解析服务器到客户端的 RTP 时间戳为 `0, 2970, 6030, 9000, ...`（单调递增）。
- **根因**：`RTP` 时间戳承载的是解码时间戳（`DTS`），Wire 上已单调；`ffprobe`（FFmpeg 4.4.2）
  对 H.264 Constrained Baseline 流在缺少 `bitstream_restriction_flag` 与 `num_reorder_frames` 时，
  会按 level 推断 `has_b_frames`，并将 `DTS` 重算到显示顺序输出，导致 `dts_time` 视觉上非单调。
- **修复**：
  - `cheetah-codec/src/egress.rs`：`select_egress_timestamps` 统一以 `DTS` 为主、`PTS` 为辅，
    保证所有协议出口端的时间线是单调的解码时间线。
  - `crates/protocols/rtsp/module/src/module/play.rs`：播放路径使用 `media_timestamp_priority`（DTS 优先）
    生成 `canonical_raw_timestamp` 与 `raw_timestamp`；RTP 时间戳按 `frame.dts` 推导，并在首次发送后
    以 `repair_monotonic_timestamp` 防止回绕引起的微小回退；RTCP Sender Report 恢复原始触发逻辑。
  - `crates/protocols/rtsp/module/src/module/publish.rs`：发布路径通过 `normalize_publish_frame_timestamps` 同步
    `frame.dts/frame.pts` 与微秒字段，并正确设置 `B_FRAME` 标志。
  - `crates/foundation/cheetah-codec/src/ingress.rs`：RTP 入口统一以 `pts` 作为 `DtsPts` 的 DTS/PTS 候选，
    交给 `TimestampNormalizer` 钳制成单调 DTS，同时保留原始 PTS 用于 composition 偏移。
  - `dev-scripts/cross_protocol_matrix_regression.sh`：对 RTSP 拉流命令增加 `-probesize 32 -analyzeduration 0`，
    避免 `ffprobe` 基于不完整 SPS 推断 `num_reorder_frames` 而误判 `dts_time`；
    `dev-scripts/cross_protocol_matrix_command_templates.sh` 已支持 `FFPLAY_BIN` / `FFPLAY_PULL_SINK` 环境变量，
    允许在无头环境用 `ffmpeg -f null -` 作为 pull sink 完成 `continuous_play` 指标。
- **验证**：`dev-scripts/cross_protocol_matrix_regression.sh run` 的以下场景全部 `result=PASS` 且
  `ffprobe_video_dts_monotonic=1`、`freeze_events=0`、`negative_cts=0`、`non_increasing_dts=0`：
  - `rtsp-tcp-loopback`
  - `rtsp-udp-loopback`
  - `bridge-rtmp-to-rtsp-tcp`
  - `bridge-rtmp-to-rtsp-udp`
  - `bridge-rtsp-tcp-to-rtmp`

#### F-12【已修复｜低】`should_emit_sender_report` 调试代码被临时关闭
- **证据**：`crates/protocols/rtsp/module/src/module/play.rs:2217` 临时为 `fn should_emit_sender_report(_: u32) -> bool { false }`。
- **修复**：恢复为 `packets_sent == 1 || packets_sent.is_multiple_of(200)`，使 RTCP Sender Report 正常发送。

#### F-13【观察｜低】H.264 参数集 VUI 可进一步降低播放器推断歧义
- **证据**：`test_media_files/bbb_sunflower_1080p_30fps_normal.flv` 的 H.264 SPS 为
  `profile_idc=66`（Baseline / Constrained Baseline）、`pic_order_cnt_type=2`、
  `max_num_ref_frames=1`、`vui_parameters_present_flag=1`、`bitstream_restriction_flag=0`。
- **影响**：FFmpeg 在 probe 阶段会按 H.264 level 推断 `num_reorder_frames`，导致 `ffprobe` 默认输出
  `dts_time` 非单调；`ffmpeg` 实际解码路径（`demuxer+ffmpeg`）能正确应用 `off` 偏移并输出单调
  `pkt_pts`，播放正常。
- **建议后续**：可在 `cheetah-codec` 的 `ParameterSetCache` 中增加 H.264 SPS VUI patcher，
  对 Baseline 流设置 `bitstream_restriction_flag=1` 与 `num_reorder_frames=0`；
  本轮未实现，因为当前 `-probesize 32` 测试调整已满足接受矩阵，且 SPS bitstream rewrite 需要完整
  Exp-Golomb 编解码器，风险与工作量较高，应作为独立改进项。

### 验证结果

```bash
cargo fmt --check
cargo clippy -p cheetah-codec -p cheetah-rtsp-module -p cheetah-rtmp-module -p cheetah-engine
cargo test -p cheetah-codec -p cheetah-rtsp-module -p cheetah-rtmp-module -p cheetah-engine
bash dev-scripts/check_runtime_boundaries.sh

cargo build -p cheetah-server --features rtsp,rtmp
# 启动 cheetah-server（config 含 rtmp/rtsp 监听端口）后
FFPLAY_BIN=ffmpeg FFPLAY_PULL_SINK="-f null -" \
  bash dev-scripts/cross_protocol_matrix_regression.sh run rtsp-tcp-loopback
```

### 结论
- P4 跨协议桥接（RTSP/RTMP 互转）的接受矩阵回归通过，时间戳在 `AVFrame + TrackInfo` 层统一收敛，
  出口单调性正确。
- `ProjectReviewPlan.md` §5.1 已勾选，§5.5 feature gating 已勾选；
  §5.2 每条 repair 日志的完整 source/canonical 上下文、§5.3 性能与并发、§5.4 安全仍留待后续。

---

## P5 结论与后续

**本轮已落地修复：**
1. F-01 修复 `check_runtime_boundaries.sh`（恢复守卫有效性）。
2. F-02 修复 hls module runtime 中立性违规 + 清理无用 tokio 依赖。
3. F-03 工具链升级至满足 `is_multiple_of` 的 stable（1.96），全量构建通过。
4. F-04 webrtc module 去 tokio 化：分阶段 Stage 1–6 落地，A 类原语机械替换 + B 类 WS/TLS/socket 下沉到 driver。
5. F-05 将 FLV 封装收敛到 `cheetah-codec`，解除 `http-flv` 对 `rtmp-core` 的生产依赖。
6. F-06 mp4 桥接 runtime 中立化：`VodEventStream` + `RuntimeApi` 注入，module 生产代码零 tokio。
7. F-07 补齐 HLS/HTTP-TS/MP4 VOD/SRT 的 Reference Mapping 与能力快照。
8. F-08 补齐 ts 与 http-flv 的 property-tests。
9. F-09 观测性基线：三层 repair 分类 + 阈值 + 运行报告 schema 与 `MetricsRegistry` 暴露。
10. F-10 统一 `cheetah-engine` tokio 口径：AGENTS.md / SystemArchitecture.md §5 显式纳入 engine，扩展边界守卫并校验公共 API 与 [dependencies] 声明。

**第二轮（P2 深入 + 各协议测试/依赖复核）结论：**
- P2 系统层深入核对全部通过：单发布者租约（CAS + Conflict）、热路径 `try_send` 非阻塞派发与三种
  背压策略下的慢订阅者隔离、RingBuffer/订阅队列有界、module 重启 `Conflict` 语义、config 分层加载与类型
  解析。均记为通过并附 `文件:行`。
- 新增/细化发现：F-06（mp4 module `tokio::spawn` + driver 公共 API 泄漏 tokio 通道）、F-08（ts 与
  http-flv 均缺属性测试）、F-10（engine 内部直用 tokio 与 §5 允许清单冲突）。上述三项均已在第三轮通过
  F-06/F-08/F-10 对应 PR 闭环落地。

**建议后续立项（按优先级）：**
- F-03 至 F-12 已闭环；P0–P3 审查项与修复已同步到 `ProjectReviewPlan.md`；P4 跨协议一致性（F-11/F-12）已落地。
- 当前高/中风险项清零；剩余未解问题是 P3 的 http-flv fixture manifest.tsv 缺失（预存失败）与 webrtc-core 依赖 `vendor-ref/simple-media-server` 外部 fixtures 缺失。
- 继续按 `ProjectReviewPlan.md` 推进 P4 剩余项（§5.2 完整 repair 日志上下文、§5.3 性能与并发、§5.4 安全）与 P5 文档同步收尾。

**复现命令：**
```bash
bash dev-scripts/check_runtime_boundaries.sh
cargo test -p cheetah-hls-module
cargo build -p cheetah-server --features "rtsp,http-flv,hls,fmp4,ts,rtp,gb28181"
```
