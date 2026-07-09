# 双时间线媒体架构与播放顺畅度修复计划总索引

- 状态：已完成
- 目标：在保持 GOP 秒开和跨协议时间戳正确性的同时，修复 RTSP 入站过早修改时间线导致播放不如旧版本顺畅的问题。
- 方法：从单一“入站强归一化”升级为三层时间模型：源协议时间线保真、引擎 canonical 时间线、协议出站导出时间线。
- 完成判定：RTSP/RTMP 同协议推拉流、RTSP/RTMP 双向转协议、RTSP TCP/UDP 推拉流在 B 帧和非 B 帧素材下均满足 GOP 秒开、首 1 秒无快放、无 `Invalid timestamps` / `Non-increasing DTS` / `Negative cts`，且 RTSP 同协议播放顺畅度不低于旧版本。

## 总体约束

- 协议入口必须尽快收敛为 `AVFrame + TrackInfo`，但不得把源协议时间语义错误地当成目标封装时间语义。
- RTSP RTP timestamp 只代表源 RTP/展示时间语义，不得直接作为 canonical DTS。
- 引擎 RingBuffer 只存储 canonical `AVFrame`，并通过 side data 或显式字段保留 source timeline 元数据。
- RTMP/RTSP/SRT/WebRTC 出站只能消费 canonical timeline 和 source metadata，由 `cheetah-codec` 导出目标封装视图。
- `NonMonotonicDtsRepaired` 只是时间修复告警，不等价于 `DISCONTINUITY`；只有 reset、publisher restart、超大跳变等真实切段事件才是 discontinuity。
- 兼容逻辑集中在 `cheetah-codec` 或明确 compat 层，不在协议 module 热路径临时分叉。
- 后续 SRT/WebRTC 必须接入相同 ingress/egress adapter 契约，不复制 RTSP/RTMP 私有时间戳修复逻辑。

## 计划文件清单

| 文件 | 状态 | 范围 |
| --- | --- | --- |
| `media-timeline-architecture.md` | 已完成 | 双时间线/三层时间模型总体设计 |
| `media-timeline-phase-01-codec-model.md` | 已完成 | `cheetah-codec` 源时间元数据、PTS-only 输入、DTS 生成策略 |
| `media-timeline-phase-02-rtsp-ingress-source-preservation.md` | 已完成 | RTSP 入站源 RTP 时间保真与 canonical 时间生成 |
| `media-timeline-phase-03-egress-export-views.md` | 已完成 | RTMP/RTSP 出站导出视图与 pacing 边界 |
| `media-timeline-phase-04-regression-and-observability.md` | 已完成 | 跨协议回归、抓包验收、观测指标 |
| `media-timeline-troubleshooting-manual.md` | 已完成 | 双时间线故障定位与修复路径 |

## 任务完成状态总表

| 阶段 | 任务 | 状态 | 计划文件 |
| --- | --- | --- | --- |
| Architecture | A.1 明确三层时间模型 | 已完成 | `media-timeline-architecture.md` |
| Architecture | A.2 明确 ingress/canonical/egress 边界 | 已完成 | `media-timeline-architecture.md` |
| Architecture | A.3 明确兼容策略与未来协议约束 | 已完成 | `media-timeline-architecture.md` |
| Phase 01 | 1.1 源时间元数据模型 | 已完成 | `media-timeline-phase-01-codec-model.md` |
| Phase 01 | 1.2 PTS-only normalizer 输入模式 | 已完成 | `media-timeline-phase-01-codec-model.md` |
| Phase 01 | 1.3 平滑 DTS 生成策略 | 已完成 | `media-timeline-phase-01-codec-model.md` |
| Phase 01 | 1.4 codec 时间线测试矩阵 | 已完成 | `media-timeline-phase-01-codec-model.md` |
| Phase 02 | 2.1 RTSP RTP source timestamp 保留 | 已完成 | `media-timeline-phase-02-rtsp-ingress-source-preservation.md` |
| Phase 02 | 2.2 H26x AU 顺序驱动 canonical DTS | 已完成 | `media-timeline-phase-02-rtsp-ingress-source-preservation.md` |
| Phase 02 | 2.3 RTSP TCP/UDP 入站一致性 | 已完成 | `media-timeline-phase-02-rtsp-ingress-source-preservation.md` |
| Phase 02 | 2.4 RTSP 入站告警降噪与语义校正 | 已完成 | `media-timeline-phase-02-rtsp-ingress-source-preservation.md` |
| Phase 03 | 3.1 RTMP egress 时间导出视图 | 已完成 | `media-timeline-phase-03-egress-export-views.md` |
| Phase 03 | 3.2 RTSP egress RTP timestamp 导出视图 | 已完成 | `media-timeline-phase-03-egress-export-views.md` |
| Phase 03 | 3.3 bootstrap pacing 与 source/canonical 分离 | 已完成 | `media-timeline-phase-03-egress-export-views.md` |
| Phase 03 | 3.4 SRT/WebRTC egress 契约预留 | 已完成 | `media-timeline-phase-03-egress-export-views.md` |
| Phase 04 | 4.1 B 帧/非 B 帧跨协议矩阵 | 已完成 | `media-timeline-phase-04-regression-and-observability.md` |
| Phase 04 | 4.2 抓包与 ffprobe/ffplay 验收 | 已完成 | `media-timeline-phase-04-regression-and-observability.md` |
| Phase 04 | 4.3 观测指标与告警标准 | 已完成 | `media-timeline-phase-04-regression-and-observability.md` |
| Phase 04 | 4.4 文档与架构收口 | 已完成 | `media-timeline-phase-04-regression-and-observability.md` |

## 最新进展

- 2026-04-29：完成 Phase 04 / 4.4。完成文档与架构收口：`SystemArchitecture.md` 媒体时间章节新增观测与告警基线（四项起播指标、repair 分层、高频阈值、上下文完整性）；`media-timeline-troubleshooting-manual.md` 新增双时间线快速定位流程；`plans-4` 各文件与任务状态统一更新为完成。并确认本轮未引入新的 `AVFrame` / `TrackInfo` / 时间戳模型变更，因此 `AGENTS.md` 约束无需追加条目。
- 2026-04-29：完成 Phase 04 / 4.3。跨协议回归指标升级：`cross_protocol_matrix_regression.sh` 新增 `first_keyframe_delay_ms`、`source_repair_events`、`canonical_repair_events`、`egress_repair_events`、`repair_warn_high_frequency`、`repair_context_complete`，并把指标写入 `summary.txt/anomaly_summary.txt`。其中 `repair_warn_high_frequency` 通过 `REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD`（默认 32）只对 canonical/egress 高频修复触发异常，避免正常 B 帧重排被误报；`repair_context_complete` 校验 repair 日志是否同时包含 source timestamp 与 canonical `pts/dts` 上下文。acceptance matrix 与 doctor 校验同步补齐上述项，确保后续回归强约束。验证通过：`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/cross_protocol_matrix_command_templates.sh doctor-acceptance`、`bash dev-scripts/cross_protocol_matrix_regression.sh doctor`。
- 2026-04-29：完成 Phase 04 / 4.2。将 ffprobe/抓包验收纳入自动回归脚本：`cross_protocol_matrix_regression.sh` 新增 ffprobe 验证（`ffprobe_first_video_keyframe`、`ffprobe_first_video_pts_near_zero`、`ffprobe_video_dts_monotonic`）并写入 `summary.txt/anomaly_summary.txt`；acceptance matrix 新增对应阈值检查；新增可选 `ENABLE_TCPDUMP_CAPTURE=1` 抓包开关与 `pcap_file` 产物路径写入 summary（默认关闭，避免无 root 环境失败）。同时保留 ffplay/ffmpeg 日志异常检查（`Invalid timestamps`、`Non-increasing DTS`、`Negative cts`）并在 4.1 的全 profile 场景遍历中生效。验证通过：`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/cross_protocol_matrix_command_templates.sh doctor-acceptance`、`bash dev-scripts/cross_protocol_matrix_regression.sh doctor`。
- 2026-04-29：完成 Phase 04 / 4.1。扩展跨协议矩阵脚本场景与素材覆盖：新增 `rtsp-tcp-to-rtsp-udp`、`rtsp-udp-to-rtsp-tcp` 两个 RTSP 混合传输场景；`cross_protocol_matrix_regression.sh run-all` 默认按 `MATRIX_PROFILE_MODE=all` 遍历 matrix 中全部输入 profile（含 `b_frames=yes/no`），从而对每个场景自动覆盖 B 帧与非 B 帧素材。为兼容现有测试桩，若 matrix script 不支持 `list-inputs` 则回退到 `INPUT_PROFILE` 单 profile。同步补充脚本测试断言。验证通过：`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/cross_protocol_matrix_regression.sh doctor`。
- 2026-04-29：完成 Phase 03 / 3.4。在 `cheetah-codec` 增加 future protocol egress 显式契约导出 `build_future_protocol_egress_contract_view`：`SrtEgressContractView` 仅暴露 canonical 派生的 `dts_ms/composition_time_ms` 与 `codec_config/parameter_set_replay`；`WebRtcEgressContractView` 暴露 canonical 派生 `rtp_timestamp_ticks`、AU 边界、`codec_config/parameter_set_replay`，并复用 `enforce_future_protocol_egress` 的 AU 边界校验。同时将 `random_access/discontinuity` 纳入 `EgressAdapterView`，避免未来协议出站直接读取 RTMP/RTSP module 私有 session 状态。新增回归测试：`srt_egress_contract_view_uses_canonical_timeline_and_codec_config`、`webrtc_egress_contract_view_uses_exported_rtp_timestamp_only`。验证通过：`cargo fmt`、`cargo clippy -p cheetah-codec`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module --test bridge_rtsp_rtmp`、`cargo test -p cheetah-rtsp-module --test bridge_rtmp_rtsp`。
- 2026-04-29：完成 Phase 03 / 3.3。RTMP/RTSP play 启动 pacing 明确与 source epoch 分离：首个媒体帧（包括大 epoch 场景）延时固定为 0，后续按 canonical 时间线推进；并新增回归测试 `play_start_pacing_first_frame_is_immediate_even_with_large_epoch_timestamp`（RTMP/RTSP 各一条）。RTMP egress 增补 `should_reset_rtmp_egress_timeline_for_discontinuity`，仅在 `DISCONTINUITY + 大幅向后回退` 时重置 rebase/clamp/mute 状态，避免跨段续接旧时间轴，同时保留长时程正向跳变（30min 级别）不被误重置；新增单测 `discontinuity_reset_applies_only_to_large_backward_timestamp_jump`。验证通过：`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module --test bridge_rtsp_rtmp`。
- 2026-04-29：完成 Phase 03 / 3.2。RTSP play egress 时间导出统一走共享 view：视频优先 canonical PTS、音频优先 canonical DTS（由 `select_egress_timestamps` + `media_ts_to_rtp_ticks`）；新增 `source_rtp_timestamp_for_egress`，在同协议安全 codec（H265/H266/AV1/VP8/VP9/Opus/G711/MP3）且存在 source RTP 元数据时可作为保真参考；并受 canonical 约束：遇到 `DISCONTINUITY` 或首包时重置 egress 单调修复基线，其余包才做 monotonic repair。TCP interleaved 与 UDP 路径继续共享同一 timestamp view 逻辑。新增单测 `source_rtp_timestamp_for_egress_uses_supported_codec_only`。验证通过：`cargo fmt`、`cargo clippy -p cheetah-codec`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 03 / 3.1。RTMP egress 去除 `FastPts` 模式下 `timestamp_ms = dts_ms * 0.95` 的私有缩放，统一保证 RTMP timestamp 仅来自 canonical DTS 毫秒；CTS 继续由 canonical `PTS-DTS` 导出并按 FLV/RTMP 兼容策略处理负值（clamp 到 0）；late join rebase 与 monotonic repair 继续仅作用于播放出站命令时间戳，不回写 engine frame。新增回归测试 `h264_egress_fast_pts_mode_keeps_canonical_dts_timestamp`。验证通过：`cargo fmt`、`cargo clippy -p cheetah-codec`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module --test bridge_rtsp_rtmp`、`cargo test -p cheetah-rtsp-module --test bridge_rtmp_rtsp`。
- 2026-04-29：完成 Phase 02 / 2.4。RTSP publish timestamp 告警从单一“repaired”改为分级语义：`source_disorder`（PTS 重排，降级为 debug 采样）、`canonical_repair`（真实时间修复，warn）、`discontinuity`（切段/大跳变，warn）；并在日志中统一输出 `stream_key/track_id/codec/source_pts/source_dts/pts/dts/protocol_ingress/alert_class`。新增单测验证分级优先级与“正常 B 帧重排仅计入 source_disorder，不计入 canonical_repair”。验证通过：`cargo fmt`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 02 / 2.3。新增 `bridge_rtsp_rtmp` 集成用例 `rtsp_tcp_and_udp_publish_produce_consistent_rtmp_timestamps`：同一 RTP 时间序列分别走 RTSP TCP/UDP publish，RTMP 侧输出时间戳序列一致；新增 `rtsp_udp_publish_loss_and_reorder_keeps_canonical_timeline_monotonic`：UDP 丢包与乱序场景下 canonical 时间线保持单调且不回退。结合已有 `build_frame_from_rtp_records_rtcp_sender_report_mapping_when_available` 验证 RTCP SR 仅作为 source mapping 元数据，不覆盖 canonical DTS。验证通过：`cargo fmt`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 02 / 2.2。RTSP publish 入站新增 H265/H266 AU depacketize 路径：不再按 RTP 单包直通视频帧，而是按 AU 边界（含 FU/AP 组帧、marker/timestamp 边界处理）输出 `CanonicalH26x`，再由 normalizer 用 `PtsOnly` 生成平滑 canonical DTS；同时保留 source RTP timestamp 元数据。`SETUP` 预分配与 TCP interleaved/UDP unicast 收包路径统一接入同一 depacketizer 状态机。新增 H265 单元测试覆盖 source metadata 与 FU 组帧边界行为。验证通过：`cargo fmt`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`（含 bridge_rtsp_rtmp、bridge_rtmp_rtsp）。
- 2026-04-29：完成 Phase 02 / 2.1。RTSP publish 入站补齐 source/canonical 映射回归：新增 H264 `build_frame_from_rtp` source timestamp 单测；`normalize_publish_frame_timestamps` 扩展为全视频 codec（H264/H265/H266/AV1/VP8/VP9）验证“忽略 raw video dts，仅按 source PTS（PtsOnly）驱动 canonical”；扩展全音频 codec（AAC/Opus/G711A/G711U/MP3）验证“保留 dts 语义并稳定归一化”。并通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`（含 bridge_rtsp_rtmp、bridge_rtmp_rtsp TCP/UDP）回归。
- 2026-04-29：完成 Phase 01 / 1.4。`cheetah-codec` 新增 `media_kernel_matrix` 时间线矩阵测试：覆盖 PTS-only B 帧重排与非 B 帧稳定 cadence；覆盖 RTMP DTS+CTS 到 canonical 映射；覆盖 32-bit wrap、随机 epoch、重复 timestamp、小幅回退修复与大幅 reset/discontinuity；覆盖视频 H264/H265/H266/AV1/VP8/VP9 与音频 AAC/Opus/G711A/G711U/MP3。并通过 `cargo fmt`、`cargo clippy -p cheetah-codec`、`cargo test -p cheetah-codec` 及受影响 `cheetah-rtsp-module`、`cheetah-rtmp-module` clippy/test 回归。
- 2026-04-29：完成 Phase 01 / 1.3。`TimestampNormalizer` 的 `PtsOnly` 路径引入平滑 DTS 生成策略：优先使用 `frame_duration`，其次使用相邻 PTS 差值，再回退到估计步进；新增 `PtsReorderObserved` 结构化告警用于小幅重排（不标记 discontinuity）；对大跳变保留正向大步进并标记 `TimelineDiscontinuityDetected`，同时重建生成状态，避免“压缩为 +1 tick”或“长时程被压扁”。RTSP publish 引入 `fallback_step_for_publish_frame()`（duration > fps > clock-rate 提示）以提升 PTS-only 入站稳定性，并保持 RTSP->RTMP 长时程回归通过。
- 2026-04-29：完成 Phase 01 / 1.2。`TimestampNormalizeInput` 从隐式 `Option` 组合升级为显式 `TimestampNormalizeMode`：`DtsPts`、`DtsWithCompositionOffset`、`PtsOnly`（并保留 `NoTimestamp` 用于 fallback-only 通用场景）；RTSP publish ingress 视频改为 `PtsOnly`，音频改为 `DtsPts`；RTMP ingest 视频改为 `DtsWithCompositionOffset`（DTS+CTS），音频改为 `DtsWithCompositionOffset`（offset=None，保持 `pts==dts`）。新增 `PtsOnly` 单测并修复 RTMP 音频回退场景回归，确保跨协议播放时间线稳定。
- 2026-04-29：完成 Phase 01 / 1.1。`AVFrame` side data 新增结构化 source timeline 模型，支持 `RtpTimestamp` 与 `RtmpTimestamp`（含 raw/unwrapped/epoch、RTP sequence、可选 RTCP mapping）；RTSP 入站在所有 codec 路径写入 RTP source timestamp，RTMP 音视频入站写入 RTMP source timestamp；新增 codec/rtsp/rtmp 单测覆盖 source timeline 与 canonical `pts/dts` 并存观测，验证 source 元数据不影响 canonical 播放时间线与排序语义。
- 2026-04-29：完成 Architecture 任务 A.3。`cheetah-codec` 补齐 future protocol ingress 契约：WebRTC ingress 也必须经过 normalizer（`TimelineSource::TimestampNormalizer`），否则返回结构化错误；新增契约测试覆盖 WebRTC passthrough 拒绝与 normalized 接受。`cheetah-rtsp-module` publish 时间修复日志补齐 `source_pts/source_dts`，排障可同时观测 source timeline 与 canonical `pts/dts` 及 alert 原因。并保持 `NonMonotonicDtsRepaired` 不触发 discontinuity 切段语义。
- 2026-04-29：完成 Architecture 任务 A.2。RTSP publish 入站新增 `source_dts_for_rtsp_ingress()`：视频轨按 `PTS-only` 进入 normalizer（不再把 RTP timestamp 直接当 canonical DTS），音频轨继续保留 DTS 输入；新增单测覆盖“视频忽略 raw dts 输入、音频保留 dts 输入”。同时复核 RTMP ingress、Engine canonical 存储、RTMP/RTSP egress 导出边界符合三层时间模型。
- 2026-04-29：完成 Architecture 任务 A.1。已在 `SystemArchitecture.md` 补充 source/canonical/egress 三层时间模型定义与边界规则，并在 `cheetah-codec` crate 文档中明确 `AVFrame.pts/dts` 只表示 canonical timeline，协议原始 timestamp 作为 source metadata 保留，egress 时间修复不得回写 canonical timeline。
- 2026-04-29：创建 `plans-4` 计划文档结构，所有任务初始化为“未开始”。
- 2026-04-29：根据当前问题确认架构方向：采用“双时间线”方案，避免 RTSP 入站把 RTP timestamp 直接当 DTS 并压缩 B 帧时间线。

## 渐进式执行顺序

1. 先完成 Architecture，统一术语和边界，避免再次把源协议时间、canonical 时间、封装时间混用。
2. 再完成 Phase 01，在 `cheetah-codec` 建立可复用的源时间元数据和 PTS-only canonical 生成能力。
3. 再完成 Phase 02，把 RTSP 入站改为源时间保真 + canonical 时间生成。
4. 再完成 Phase 03，把 RTMP/RTSP 出站改为只消费导出视图，不倒逼入站做目标协议修正。
5. 最后完成 Phase 04，用抓包、日志和跨协议矩阵锁定播放顺畅度和未来协议扩展边界。

## 阶段完成后的统一检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo clippy -p cheetah-engine
cargo test -p cheetah-engine
cargo clippy -p cheetah-rtmp-module
cargo test -p cheetah-rtmp-module
cargo clippy -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-module --test bridge_rtsp_rtmp
cargo test -p cheetah-rtsp-module --test bridge_rtmp_rtsp
```

涉及实际播放修复时，还必须执行 RTSP 推流、RTMP/RTSP late join 拉流、tcpdump 抓包和 ffprobe 首包检查。
