# Phase 04: 全矩阵回归与可观测性收口

- 状态：已完成（任务 1.1-1.3、2.1-2.3、3.1-3.3、4.1-4.2 已完成）
- 范围：建立跨协议时间戳问题的标准回归矩阵与观测基线，确保后续改动可快速定位退化。
- 完成标准：形成固定执行脚本/命令、固定日志字段与固定验收标准，计划状态全部闭环。

## 具体任务

### 1. 端到端回归矩阵固化（已完成）

- [x] 固化推流/拉流命令模板（RTSP TCP/UDP、RTMP、跨协议双向）。
- [x] 固化输入素材矩阵（含 B 帧与非 B 帧视频样本）。
- [x] 固化验收点：起播时延、连续播放、无冻结、无 `DTS out of order`。

#### 已固化命令模板（2026-04-28）

- 脚本入口：`dev-scripts/cross_protocol_matrix_command_templates.sh`
- 场景列表：`list`
- 单场景模板：`show <scenario>`
- 全场景模板：`show-all`
- 环境自检（依赖和输入文件）：`doctor`

示例：

```bash
./dev-scripts/cross_protocol_matrix_command_templates.sh list
./dev-scripts/cross_protocol_matrix_command_templates.sh show bridge-rtsp-udp-to-rtmp
./dev-scripts/cross_protocol_matrix_command_templates.sh doctor
```

#### 已固化输入素材矩阵（2026-04-28）

- 素材矩阵文件：`dev-scripts/cross_protocol_matrix_input_matrix.tsv`
- 覆盖要求：至少包含一个 `b_frames=yes` 样本和一个 `b_frames=no` 样本；`doctor-inputs` 强制校验。
- 新增命令：
  - `list-inputs`：列出素材 profile、B 帧标记、文件路径与用途说明。
  - `show-input <profile>`：查看单个素材 profile 详情。
  - `doctor-inputs`：校验矩阵结构、字段合法性与素材文件存在性。
- 选择方式：
  - 默认通过 `INPUT_PROFILE` 选择素材（默认 `b-frame-h264`）。
  - 兼容保留 `INPUT_FILE` 直接覆盖模式（用于临时自定义素材）。

示例：

```bash
./dev-scripts/cross_protocol_matrix_command_templates.sh list-inputs
./dev-scripts/cross_protocol_matrix_command_templates.sh show-input non-b-hevc
./dev-scripts/cross_protocol_matrix_command_templates.sh doctor-inputs
INPUT_PROFILE=non-b-hevc ./dev-scripts/cross_protocol_matrix_command_templates.sh show rtmp-loopback
```

#### 已固化验收点（2026-04-28）

- 验收矩阵文件：`dev-scripts/cross_protocol_matrix_acceptance_matrix.tsv`
- 覆盖要求：必须包含并通过校验的四个关键验收点：
  - `startup_latency`（阈值 `<= 3000ms`）
  - `continuous_play`（阈值 `>= 300 seconds`，即持续观测窗口）
  - `freeze_events`（阈值 `== 0`）
  - `dts_out_of_order`（阈值 `== 0`）
- 新增命令：
  - `list-acceptance`：按固定格式列出所有验收项和阈值。
  - `show-acceptance`：输出可直接执行回归时使用的验收清单。
  - `doctor-acceptance`：校验验收矩阵字段、阈值格式、单位和必选检查项完整性。
- `doctor` 主入口已强制包含 `doctor-acceptance`，确保输入矩阵与验收矩阵同时有效。

示例：

```bash
./dev-scripts/cross_protocol_matrix_command_templates.sh list-acceptance
./dev-scripts/cross_protocol_matrix_command_templates.sh show-acceptance
./dev-scripts/cross_protocol_matrix_command_templates.sh doctor-acceptance
./dev-scripts/cross_protocol_matrix_command_templates.sh doctor
```

### 2. 日志与指标可观测性补齐（已完成）

- [x] 为关键路径增加统一日志键：`stream_key/track_id/codec/pts/dts`。
- [x] 对时间戳修正分支增加采样日志，支持在线排障。
- [x] 明确告警阈值：起播超时、时间戳逆序、队列堆积。

#### 2.1 已完成：关键路径统一日志键（2026-04-28）

- `cheetah-rtmp-module`：
  - 为 push/play 关键失败路径日志补齐统一字段 `stream_key/track_id/codec/pts/dts`（含帧映射失败、媒体发送失败、静音补帧发送失败）。
  - 增加 `frame_observability_fields` 辅助字段提取，避免日志字段散落和命名漂移。
- `cheetah-rtsp-module`：
  - 为 publish 回压丢帧/发送失败日志补齐统一字段 `stream_key/track_id/codec/pts/dts`。
  - 为 play RTP 发送失败与 RTCP 组包失败日志补齐统一字段 `stream_key/track_id/codec/pts/dts`。
- 回归验证：
  - `cargo fmt`
  - `cargo clippy -p cheetah-rtmp-module`
  - `cargo clippy -p cheetah-rtsp-module`
  - `cargo test -p cheetah-rtmp-module`
  - `cargo test -p cheetah-rtsp-module`

#### 2.2 已完成：时间戳修正分支采样日志（2026-04-28）

- `cheetah-rtmp-module`：
  - 在 RTMP ingest 视频/音频单调修正分支新增采样日志，覆盖 `WrapUnwrapper` 后回退修正路径。
  - 日志统一带出 `stream_key/track_id/codec/pts/dts`，并追加 `source_dts/raw_timestamp_ms/repair_count`，用于在线定位修正规模与输入偏差。
  - 采样策略采用渐进采样（前 3 次、2 的幂次、每 1024 次）避免高频日志放大。
- `cheetah-rtsp-module`：
  - 在 RTSP publish 侧 DTS 重排修正分支新增采样日志，覆盖 `generated_dts <= last_dts` 修正路径。
  - 在 RTSP play 侧 RTP 时间戳单调修正分支新增采样日志，覆盖 `raw_timestamp <= last_rtp_timestamp` 修正路径。
  - 两条分支日志统一带出 `stream_key/track_id/codec/pts/dts`，并追加修正前后时间戳与 `repair_count`。
- 回归验证：
  - `cargo fmt`
  - `cargo clippy -p cheetah-rtmp-module`
  - `cargo clippy -p cheetah-rtsp-module`
  - `cargo test -p cheetah-rtmp-module`
  - `cargo test -p cheetah-rtsp-module`

#### 2.3 已完成：告警阈值固化（2026-04-28）

- 统一阈值配置：
  - `cheetah-rtmp-module` / `cheetah-rtsp-module` 增加 `alert_thresholds`，统一暴露：
    - `startup_timeout_ms`
    - `timestamp_repair_count`
    - `queue_drop_count`
  - 默认值统一为 `3000ms / 32 / 64`，并增加配置校验（阈值必须 `> 0`；RTMP 在启用 `play_wait_source_timeout_ms` 时要求 `startup_timeout_ms <= play_wait_source_timeout_ms`）。
- 起播超时告警：
  - `cheetah-rtmp-module` 在 pending play 等待源流分支增加“超阈值预警”，在真正拒绝前输出等待时长和阈值。
  - `cheetah-rtsp-module` 在 PLAY 起播阶段增加“首帧等待超阈值预警”。
- 时间戳逆序告警：
  - `cheetah-rtmp-module` ingest 视频/音频单调修正路径在 `repair_count` 达阈值时额外告警。
  - `cheetah-rtsp-module` publish/play 时间戳修正路径在 `repair_count` 达阈值时额外告警。
- 队列堆积告警：
  - `cheetah-rtmp-module` 与 `cheetah-rtsp-module` 对 `DispatchResult::DroppedByPolicy` 做计数，达到阈值（含阈值倍数）触发告警；成功推送时重置计数，避免历史值误报。
- 告警字段：
  - 告警日志均保持 `stream_key/track_id/codec/pts/dts` 基础字段，并补充阈值字段（如 `*_alert_threshold`、`queue_drop_count`、`repair_count`）。
- 回归验证：
  - `cargo fmt`
  - `cargo clippy -p cheetah-rtmp-module`
  - `cargo clippy -p cheetah-rtsp-module`
  - `cargo test -p cheetah-rtmp-module`
  - `cargo test -p cheetah-rtsp-module`

### 3. 回归自动化与文档沉淀（已完成）

- [x] 增加脚本化回归入口（开发环境一键执行）。
- [x] 在 `dev-docs/plans-2` 中更新最终完成状态与结果摘要。
- [x] 沉淀故障排查手册（输入条件、现象、定位路径、修复策略）。

#### 3.1 已完成：脚本化回归入口（2026-04-28）

- 新增脚本：`dev-scripts/cross_protocol_matrix_regression.sh`
  - 一键入口：`run <scenario>`、`run-all`，并保留 `list/doctor`。
  - 复用 `cross_protocol_matrix_command_templates.sh` 的场景命令与 `doctor` 校验，避免脚本能力分叉。
  - 新增统一错误处理：模板脚本不可执行、场景命令解析失败、拉流进程异常退出、验收阈值不达标均显式失败并返回非零退出码。
  - 自动产出回归记录：按 `REPORT_ROOT/<run_id>/<scenario>/` 落盘 `push.log`、`pull.log`、`summary.txt`，便于复现与排障。
  - 自动验收覆盖四类固定检查项：`startup_latency`、`continuous_play`、`freeze_events`、`dts_out_of_order`（从验收矩阵读取阈值并统一判定）。
- 新增测试：`dev-scripts/tests/cross_protocol_matrix_regression_test.sh`
  - 覆盖成功路径（`run-all` 全通过并生成 summary）与错误路径（命令模板缺失 pull 命令、`DTS out of order` 触发验收失败）。
- 回归验证：
  - `bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`
  - `bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`
  - `cargo fmt`
  - `cargo clippy -p cheetah-rtmp-module`
  - `cargo clippy -p cheetah-rtsp-module`
  - `cargo test -p cheetah-rtmp-module`
  - `cargo test -p cheetah-rtsp-module`

#### 3.2 已完成：计划文档最终状态与结果摘要同步（2026-04-28）

- 更新文件：
  - `dev-docs/plans-2/cross-protocol-gop-index.md`
  - `dev-docs/plans-2/cross-protocol-phase-04-matrix-regression-and-observability.md`
- 变更内容：
  - 同步 Phase 04 当前完成状态为“任务 1.1-1.3、2.1-2.3、3.1-3.2 已完成”。
  - 在总索引“最新进展”中补充 Phase 04 任务 3.2 结果摘要，固定已完成范围和已验证命令集合。
  - 将总索引“下一步”更新为任务 3.3（故障排查手册沉淀），避免与当前完成状态不一致。

#### 3.3 已完成：故障排查手册沉淀（2026-04-28）

- 新增文档：
  - `dev-docs/plans-2/cross-protocol-matrix-troubleshooting-manual.md`
- 覆盖内容：
  - 固化排查输入条件（场景、输入素材、环境参数、回归参数、验收矩阵版本、run_id）。
  - 固化“现象 -> 定位路径 -> 修复策略”闭环，覆盖 `startup_latency`、`dts_out_of_order`、`freeze_events`、`repair_count`、`queue_drop_count` 五类核心故障。
  - 固化推荐排查流程与命令清单，统一要求先 `doctor` 再 `run`，并基于 `summary/push.log/pull.log` 定位。
  - 固化“通用修复优先”原则：优先修复共享时间轴与背压策略，禁止场景特判和阈值掩盖。
- 回归验证：
  - `bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`
  - `bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`
  - `cargo fmt`
  - `cargo clippy -p cheetah-rtmp-module`
  - `cargo clippy -p cheetah-rtsp-module`
  - `cargo test -p cheetah-rtmp-module`
  - `cargo test -p cheetah-rtsp-module`

### 4. 最终收口（已完成）

- [x] 所有 Phase 状态更新为“已完成”。
- [x] 汇总遗留风险（若无则明确“无遗留风险”）。

#### 4.1 已完成：Phase 状态收口同步（2026-04-28）

- 更新文件：
  - `dev-docs/plans-2/cross-protocol-gop-index.md`
  - `dev-docs/plans-2/cross-protocol-phase-04-matrix-regression-and-observability.md`
- 变更内容：
  - 将总索引与 Phase 04 文档状态同步推进为“任务 4.1 已完成、4.2 进行中”。
  - 将总索引“下一步”推进到任务 4.2（遗留风险汇总），避免执行状态漂移。
- 回归验证：
  - `bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`
  - `bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`
  - `cargo fmt`
  - `cargo clippy -p cheetah-rtmp-module`
  - `cargo clippy -p cheetah-rtsp-module`
  - `cargo test -p cheetah-rtmp-module`
  - `cargo test -p cheetah-rtsp-module`

#### 4.2 已完成：遗留风险汇总（2026-04-28）

- 更新文件：
  - `dev-docs/plans-2/cross-protocol-gop-index.md`
  - `dev-docs/plans-2/cross-protocol-phase-04-matrix-regression-and-observability.md`
- 遗留风险结论：
  - 无阻塞发布的遗留风险；Phase 01-04 的范围内已固化命令模板、输入矩阵、验收矩阵、告警阈值与排障手册，且回归入口脚本具备参数校验、依赖检查、命令解析失败显式退出、验收失败显式退出等统一错误处理。
  - 后续仅保留常规演进项（新增协议/编码或运行时能力时同步扩展矩阵与验收阈值），不属于本轮收口遗留缺陷。
- 回归验证：
  - `bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`
  - `bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`
  - `cargo fmt`
  - `cargo clippy -p cheetah-rtmp-module`
  - `cargo clippy -p cheetah-rtsp-module`
  - `cargo test -p cheetah-rtmp-module`
  - `cargo test -p cheetah-rtsp-module`

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-rtsp-module`
- `cargo clippy -p cheetah-rtmp-module`
- `cargo test -p cheetah-rtsp-module`
- `cargo test -p cheetah-rtmp-module`
- 全矩阵端到端回归命令执行并记录结果
