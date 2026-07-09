# 跨协议 GOP 秒开与时间戳统一计划总索引

- 状态：已完成（Phase 01-04 已完成：任务 1.1-1.3、2.1-2.3、3.1-3.3、4.1-4.2 已完成）
- 目标：解决 RTSP/RTMP 跨协议推拉流中的首帧延迟、画面卡住与 `DTS out of order`，并统一所有音视频编码路径的时间戳策略。
- 方法：按“单链路先闭环、再扩展到全协议矩阵”的渐进式开发方式推进；每个阶段都要求可复现、可回归、可观测。
- 完成判定：所有阶段状态为“已完成”，并通过跨协议端到端回归矩阵（RTSP TCP/UDP、RTMP、同协议与跨协议双向）。

## 总体约束

- 严格遵守 `core + driver + module` 分层，不在 module 复制 codec 公共能力。
- 时间戳与 DTS 单调修正优先复用 `cheetah-codec`，避免协议侧私有分叉。
- 兼容优先：入口容错、内部归一化、出口稳定可预测。
- 变更先小后大：先修复单路径阻塞问题，再扩展全协议与全编码覆盖。

## 计划文件清单

| 计划文件 | 状态 | 主要范围 |
| --- | --- | --- |
| `cross-protocol-phase-01-rtsp-ingest-and-play-timestamp.md` | 已完成（任务 1-4 已完成） | RTSP 发布入口视频 DTS 单调修正、播放端 RTP 时间戳优先级、首帧连续性回归 |
| `cross-protocol-phase-02-rtmp-ingest-normalization.md` | 已完成（任务 1-4 已完成） | RTMP 发布入口时间戳归一化与 GOP 快速起播对齐 |
| `cross-protocol-phase-03-bridge-rtsp-rtmp-bidirectional.md` | 已完成（任务 1-4 已完成） | RTSP->RTMP 与 RTMP->RTSP 双向桥接链路时间戳与缓冲策略 |
| `cross-protocol-phase-04-matrix-regression-and-observability.md` | 已完成（任务 1.1-1.3、2.1-2.3、3.1-3.3、4.1-4.2 已完成） | 全矩阵回归、日志观测与运维排障基线 |

## 最新进展

- 2026-04-28：常规回归复验已完成。复核 `cross-protocol-gop-index` 后确认“未完成任务=0、下一步=无”，本轮未新增实现任务；按基线重新执行 `bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`，全部通过。
- 2026-04-28：Phase 04 任务 4.2 已完成。`dev-docs/plans-2/cross-protocol-gop-index.md` 与 `dev-docs/plans-2/cross-protocol-phase-04-matrix-regression-and-observability.md` 已同步收口为“Phase 01-04 全部已完成”；遗留风险结论为“无阻塞发布的遗留风险”，并固定“通用修复优先 + 统一错误处理（参数校验/依赖检查/命令解析失败显式退出/验收失败显式退出）”基线。`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-28：Phase 04 任务 4.1 已完成。`dev-docs/plans-2/cross-protocol-gop-index.md` 与 `dev-docs/plans-2/cross-protocol-phase-04-matrix-regression-and-observability.md` 已同步推进为“任务 1.1-1.3、2.1-2.3、3.1-3.3、4.1 已完成，4.2 进行中”；“下一步”已更新为任务 4.2（遗留风险汇总），避免执行状态漂移。`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-28：Phase 04 任务 3.3 已完成。新增 `dev-docs/plans-2/cross-protocol-matrix-troubleshooting-manual.md`，沉淀跨协议排障手册：固定排查输入条件（场景、素材、环境参数、run_id）、五类核心故障（`startup_latency`、`dts_out_of_order`、`freeze_events`、`repair_count`、`queue_drop_count`）的“现象 -> 定位路径 -> 修复策略”闭环，以及先 `doctor` 后 `run` 的标准排查流程与命令清单；修复策略明确“通用修复优先”，避免场景特判与阈值掩盖。`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-28：Phase 04 任务 3.2 已完成。`dev-docs/plans-2/cross-protocol-gop-index.md` 与 `dev-docs/plans-2/cross-protocol-phase-04-matrix-regression-and-observability.md` 已同步更新最终完成状态与结果摘要：明确 Phase 04 当前完成范围为“1.1-1.3、2.1-2.3、3.1-3.2”，并固定回归命令集与验收口径，确保后续执行与文档状态一致。`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-28：Phase 04 任务 3.1 已完成。新增一键回归入口脚本 `dev-scripts/cross_protocol_matrix_regression.sh`，提供 `run <scenario>/run-all/list/doctor` 子命令，复用 `cross_protocol_matrix_command_templates.sh` 的场景模板与 doctor 校验；回归执行按场景落盘 `push.log/pull.log/summary.txt`，并基于 `cross_protocol_matrix_acceptance_matrix.tsv` 自动判定 `startup_latency/continuous_play/freeze_events/dts_out_of_order` 四类固定验收项。脚本新增统一错误处理（模板不可执行、命令解析失败、拉流异常退出、验收不达标均显式失败），并补充 `dev-scripts/tests/cross_protocol_matrix_regression_test.sh` 覆盖成功与失败路径。`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-28：Phase 04 任务 2.3 已完成。`cheetah-rtmp-module` 与 `cheetah-rtsp-module` 新增统一 `alert_thresholds` 配置（`startup_timeout_ms`、`timestamp_repair_count`、`queue_drop_count`），并在三类关键分支接入阈值告警：RTMP pending play 起播等待超阈值预警、RTMP/RTSP 时间戳逆序修正计数阈值预警、RTMP/RTSP 回压丢帧队列堆积阈值预警。RTMP ingest 与 RTSP publish/play 的告警日志均统一带出 `stream_key/track_id/codec/pts/dts` 及阈值字段，便于线上排障与告警路由。新增阈值判定与配置校验单测。`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-28：Phase 04 任务 2.2 已完成。`cheetah-rtmp-module` 在 RTMP ingest 视频/音频时间戳单调修正分支新增采样日志，`cheetah-rtsp-module` 在 RTSP publish 的 DTS 修正分支与 RTSP play 的 RTP 时间戳单调修正分支新增采样日志；三处日志统一输出 `stream_key/track_id/codec/pts/dts`，并附带 `source_dts/raw_timestamp/repaired_timestamp/repair_count` 等排障字段，采样策略统一为“前 3 次 + 2 的幂次 + 每 1024 次”。新增采样策略单测覆盖。`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-28：Phase 04 任务 2.1 已完成。`cheetah-rtmp-module` 与 `cheetah-rtsp-module` 在关键媒体路径日志统一补齐 `stream_key/track_id/codec/pts/dts` 五个字段：覆盖 RTMP push/play 的帧映射失败、媒体发送失败与静音补帧发送失败，以及 RTSP publish 的回压丢帧/发送失败、RTSP play 的 RTP 发送失败与 RTCP 组包失败；并新增 `frame_observability_fields` 单测约束字段提取一致性。`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-28：Phase 04 任务 1.3 已完成。新增 `dev-scripts/cross_protocol_matrix_acceptance_matrix.tsv`，固化“起播时延、连续播放、无冻结、无 DTS out of order”四项跨协议验收标准；`dev-scripts/cross_protocol_matrix_command_templates.sh` 增加 `list-acceptance/show-acceptance/doctor-acceptance` 子命令，并将 `doctor` 主流程升级为“输入矩阵 + 验收矩阵”双重强校验（字段、阈值、单位、必选检查项完整性）；同时扩展脚本回归测试覆盖验收矩阵主流程与错误路径（缺失关键检查项时应失败）。
- 2026-04-28：Phase 04 任务 1.2 已完成。新增 `dev-scripts/cross_protocol_matrix_input_matrix.tsv`，固化跨协议回归输入素材矩阵（覆盖 `b_frames=yes/no`）；`dev-scripts/cross_protocol_matrix_command_templates.sh` 增加 `list-inputs/show-input/doctor-inputs` 子命令与 `INPUT_PROFILE/MATRIX_INPUT_FILE` 选择能力，并在 `doctor` 路径强制执行素材矩阵完整性检查（字段校验、文件存在性、B 帧/非 B 帧覆盖）；新增脚本回归测试 `dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh` 覆盖输入矩阵主流程。
- 2026-04-28：Phase 04 任务 1.1 已完成。新增 `dev-scripts/cross_protocol_matrix_command_templates.sh`，固化 RTSP TCP/UDP、RTMP、RTSP->RTMP、RTMP->RTSP 的推流/拉流命令模板，并提供 `list/show/show-all/doctor` 子命令与统一错误处理（参数校验、依赖检查、输入文件检查）；同步更新 `cross-protocol-phase-04-matrix-regression-and-observability.md` 进入进行中状态。
- 2026-04-28：Phase 01 任务 1-4 已完成。`cheetah-rtsp-module` 已落地“所有视频编码统一 DTS 单调修正（无固定 lookahead 缓冲）”，并将播放端 RTP 时间戳策略调整为“视频优先 PTS，音频优先 DTS”；`cargo fmt`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module` 全通过，`tcp_interleaved_play_pause_play_rtp_rtcp_continuity` 回归通过。
- 2026-04-28：Phase 02 任务 1 已完成。`cheetah-rtmp-module` 在 RTMP 发布入口接入了基于 `cheetah-codec::WrapUnwrapper` 的 32-bit 时间戳回绕处理与音视频分轨单调补偿，视频保持 `DTS=tag timestamp`、`PTS=DTS+CTS`，音频保持 `PTS=DTS`；新增入口回退时间戳回归测试。`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module` 全通过。
- 2026-04-28：Phase 02 任务 2 已完成。`cheetah-rtmp-module` 为 H265 增加了 `hvcc` 缺失时的参数集回退配置构建（由 `vps/sps/pps` 生成并用于 bootstrap），并补充了 VP8/VP9/AV1 bootstrap 配置一致性测试及 AAC/Opus/G711/MP3 音频时间轴连续性回归测试。`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module` 全通过。
- 2026-04-28：Phase 02 任务 3 已完成。`cheetah-rtmp-module` 为 RTMP 主线补齐了长时与高负载回归基础：统一 `play/push` 订阅窗口策略（视频 bootstrap floor 与队列容量下限绑定，覆盖 H264/H265/H266/VP8/VP9/AV1），并在 push 主线补充订阅/建连失败的显式告警日志；新增“10 分钟 H264 时间轴单调性”与“未知轨道阶段 push GOP 窗口下限”回归测试。`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module` 全通过。
- 2026-04-28：Phase 02 任务 4 已完成。新增 H265 缺失配置下的 bootstrap 安全跳过单测与 `rtmp_module_push_job_resilience` 集成测试，覆盖 push 源流不存在时的错误路径；并将播放/推流 bootstrap 及媒体发送失败从静默吞错改为显式告警与受控退出。`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module` 全通过。
- 2026-04-28：Phase 02 收口复验通过。`cheetah-rtmp-module` 当前测试面为 `57` 个单测 + `1` 个集成测试，均通过。
- 2026-04-28：Phase 03 任务 1 已完成。`cheetah-rtmp-module` 在 RTSP->RTMP 桥接出口统一使用 `AVFrame.timebase` 转毫秒映射（覆盖视频/音频 `timestamp_ms` 与视频 `composition_time`），并将播放/推流静音注入时间戳改为同源换算，避免混用 RTP tick 导致跨协议 FLV `DTS out of order`；新增 `rtsp_timebase_is_normalized_to_rtmp_milliseconds_on_egress` 回归测试。`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-28：Phase 03 任务 2 已完成。`cheetah-rtsp-module` 修正 RTP 时间戳映射中“主时间戳为 `0` 被误判缺失”的问题，确保 RTMP->RTSP 桥接保持视频 `PTS` 与音频 `DTS` 优先级；新增 `bridge_rtmp_rtsp` 集成测试，覆盖 RTSP `TCP interleaved` 与 `UDP unicast` 两种播放模式下的视频/音频 RTP 时间戳断言。`cargo fmt`、`cargo clippy -p cheetah-rtsp-module`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module` 全通过。
- 2026-04-28：Phase 03 任务 3 已完成。补齐跨协议多编码一致性覆盖：`cheetah-rtmp-module` 新增 RTSP->RTMP 视频/音频编码矩阵时间戳归一化回归（H264/H265/AV1/VP8/VP9 + AAC/Opus/G711/MP3），并修复 11.025kHz MP3 在毫秒换算中的向下截断漂移（`99ms -> 100ms`，统一为就近舍入）；`cheetah-rtsp-module` 新增 RTMP->RTSP 播放起播门控与原始 RTP 时间戳策略矩阵回归，确保多编码路径首帧门控与时间轴策略一致；同时补充 RTMP 桥接帧映射失败的显式告警，避免静默丢帧。`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-28：Phase 03 任务 4 已完成。`cheetah-rtsp-module` 新增 `bridge_rtsp_rtmp` 集成测试，补齐 `RTSP(TCP)->RTMP` 与 `RTSP(UDP)->RTMP` 双链路回归，并在同一用例覆盖“连续多次 RTMP 拉流”与“>=30 分钟时间轴跨度”的长时单调性断言；结合已存在 `bridge_rtmp_rtsp` 用例，形成双向桥接 TCP/UDP 回归矩阵。`cargo fmt`、`cargo clippy -p cheetah-rtsp-module`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module` 全通过。
- 下一步：无。当前计划项已全部完成，进入常规回归维护阶段。

## 渐进式执行顺序

1. 先收敛 RTSP 链路（发布入口与播放出口）时间戳策略，确保“秒开不牺牲稳定性”。
2. 再收敛 RTMP 入口，统一 AVFrame 时间轴与参数集行为。
3. 然后打通 RTSP<->RTMP 双向桥接，做跨协议时间戳与缓冲的一致性修正。
4. 最后完善全矩阵回归与观测指标，形成长期可维护基线。

## 阶段完成后的统一检查

- `cargo fmt`
- `cargo clippy -p <changed-crate>`
- `cargo test -p <changed-crate>`
- 若影响 `cheetah-codec` / 协议公共层，再执行：
  - `cargo test -p cheetah-rtsp-module`
  - `cargo test -p cheetah-rtmp-module`
  - 相关跨协议集成测试
