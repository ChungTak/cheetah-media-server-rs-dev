# 统一媒体内核与跨协议起播稳定性计划总索引

- 状态：已完成
- 目标：统一 RTSP、RTMP 以及后续 SRT、WebRTC 的媒体时间线、Access Unit 拼装、参数集缓存、GOP 秒开和启动 pacing，修复推拉流后首段快放、DTS/PTS 异常、GOP 未生效等问题。
- 方法：先把时间戳归一化、DTS 生成、回绕处理、断流标记、Access Unit 拼装、参数集缓存/补发下沉到 `cheetah-codec`，再让协议模块只做协议包与 `AVFrame + TrackInfo` 的适配，最后补齐跨协议矩阵回归。
- 完成判定：所有任务状态为“已完成”，RTSP/RTMP 同协议推拉流、RTSP/RTMP 双向转协议、RTSP TCP/UDP 推拉流以及多音视频编码矩阵均无 `Invalid timestamps`、`Non-increasing DTS`、`Negative cts`，并且首 1 秒无启动快放。

## 总体约束

- 协议入口进入引擎前必须统一收敛为 `AVFrame + TrackInfo`。
- 协议出口前必须优先通过 `cheetah-codec` 导出目标封装视图。
- 时间戳归一化、timebase 转换、DTS 生成、回绕处理、断流标记、Access Unit 拼装、参数集缓存/补发统一放在 `cheetah-codec`。
- RTSP/RTMP module 不再复制私有媒体时间戳修正、NALU 处理或参数集缓存逻辑。
- RTSP 的 TCP/UDP 差异只属于 transport/driver 行为，不应改变进入引擎后的媒体时间语义。
- 方案必须为 SRT、WebRTC 等后续协议保留统一的 ingress/egress adapter 契约。

## 计划文件清单

| 文件 | 状态 | 范围 |
| --- | --- | --- |
| `unified-media-phase-01-codec-media-kernel.md` | 已完成 | `cheetah-codec` 统一媒体时间、AU、参数集能力 |
| `unified-media-phase-02-protocol-ingress-normalization.md` | 已完成 | RTSP/RTMP 入站归一化与出站视图统一 |
| `unified-media-phase-03-engine-bootstrap-and-egress-pacing.md` | 已完成 | GOP 秒开、live-tail bootstrap、启动 pacing |
| `unified-media-phase-04-cross-protocol-matrix-and-future-protocols.md` | 已完成 | 全协议全编码矩阵、SRT/WebRTC 扩展契约 |
| `unified-media-troubleshooting-manual.md` | 已完成 | 常见故障定位与修复路径 |

## 任务完成状态总表

| 阶段 | 任务 | 状态 | 计划文件 |
| --- | --- | --- | --- |
| Phase 01 | 1.1 统一媒体时间模型 | 已完成 | `unified-media-phase-01-codec-media-kernel.md` |
| Phase 01 | 1.2 通用时间戳归一化器 | 已完成 | `unified-media-phase-01-codec-media-kernel.md` |
| Phase 01 | 1.3 通用 Access Unit 与参数集能力 | 已完成 | `unified-media-phase-01-codec-media-kernel.md` |
| Phase 01 | 1.4 codec 内核测试矩阵 | 已完成 | `unified-media-phase-01-codec-media-kernel.md` |
| Phase 02 | 2.1 RTSP 入站改为 codec normalizer | 已完成 | `unified-media-phase-02-protocol-ingress-normalization.md` |
| Phase 02 | 2.2 RTMP 入站改为 codec normalizer | 已完成 | `unified-media-phase-02-protocol-ingress-normalization.md` |
| Phase 02 | 2.3 RTSP/RTMP 出站导出视图统一 | 已完成 | `unified-media-phase-02-protocol-ingress-normalization.md` |
| Phase 02 | 2.4 入站/出站兼容与告警清理 | 已完成 | `unified-media-phase-02-protocol-ingress-normalization.md` |
| Phase 03 | 3.1 `SubscriberOptions` / `BootstrapPolicy` 设计 | 已完成 | `unified-media-phase-03-engine-bootstrap-and-egress-pacing.md` |
| Phase 03 | 3.2 RingBuffer live-tail bootstrap | 已完成 | `unified-media-phase-03-engine-bootstrap-and-egress-pacing.md` |
| Phase 03 | 3.3 RTMP play 启动 pacing | 已完成 | `unified-media-phase-03-engine-bootstrap-and-egress-pacing.md` |
| Phase 03 | 3.4 RTSP play 启动 pacing | 已完成 | `unified-media-phase-03-engine-bootstrap-and-egress-pacing.md` |
| Phase 03 | 3.5 慢订阅者与积压回归 | 已完成 | `unified-media-phase-03-engine-bootstrap-and-egress-pacing.md` |
| Phase 04 | 4.1 全协议全 codec 矩阵 | 已完成 | `unified-media-phase-04-cross-protocol-matrix-and-future-protocols.md` |
| Phase 04 | 4.2 SRT/WebRTC 扩展契约 | 已完成 | `unified-media-phase-04-cross-protocol-matrix-and-future-protocols.md` |
| Phase 04 | 4.3 自动化回归与报告 | 已完成 | `unified-media-phase-04-cross-protocol-matrix-and-future-protocols.md` |
| Phase 04 | 4.4 排障手册和索引收口 | 已完成 | `unified-media-phase-04-cross-protocol-matrix-and-future-protocols.md` |

## 最新进展

- 2026-04-29：完成 Phase 04 任务 4.4（排障手册和索引收口）。`dev-docs/plans-3/unified-media-troubleshooting-manual.md` 升级为可执行排障手册：补齐排查输入条件、`Invalid timestamps`/`Non-increasing DTS`/`Negative cts` 与启动快放等真实问题的“现象->定位路径->通用修复”、标准排查流程、命令清单，并新增“未覆盖组合与原因”表（SRT/WebRTC 及其桥接链路因协议三段式 crate 尚未落地而未纳入当前矩阵）。同步更新 `unified-media-phase-04-cross-protocol-matrix-and-future-protocols.md` 与本索引状态为已完成，并核对本次未改变 `AVFrame` / `TrackInfo` / 时间戳模型，无需更新 `SystemArchitecture.md`。执行并通过 `cargo fmt`、`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/cross_protocol_matrix_regression.sh doctor`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 04 任务 4.3（自动化回归与报告）。增强 `dev-scripts/cross_protocol_matrix_command_templates.sh` 与 `dev-scripts/cross_protocol_matrix_regression.sh`：场景命令统一输出 `ffmpeg/ffplay -v debug -debug_ts -stats`，固定可重复执行组合；验收矩阵新增 `invalid_timestamps/non_increasing_dts/negative_cts` 三项，回归脚本自动扫描 `Invalid timestamps`、`Non-increasing DTS`、`Negative cts` 并生成 `anomaly_summary.txt`；每个组合 `summary.txt` 新增 `startup_latency_ms`、`first_second_avg_frame_interval_ms`、`average_playback_rate_x`、`media_span_seconds` 等指标；失败场景落盘 `failure_input.txt` 用于后续补充 codec/module 回归输入。新增/更新脚本测试覆盖新验收项与失败路径。执行并通过 `bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/cross_protocol_matrix_regression.sh doctor`、`cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 04 任务 4.2（SRT/WebRTC 扩展契约）。`cheetah-codec` 新增 `adapter` 契约模块：`IngressAdapterFrame` 固定未来协议 ingress 统一字段（`track/codec/timebase/pts/dts/duration/random_access/discontinuity`）并支持 `TimestampNormalizeOutput` 对齐校验，`EgressAdapterView` 固定未来协议 egress 统一导出视图（封装时间戳、AU 分片边界、`codec_config_view`、参数集补发视图）；新增 `AdapterContractError` 统一错误处理；新增 `enforce_future_protocol_ingress/egress` 显式约束 SRT（禁止绕过归一化）与 WebRTC（视频 AU 边界必须由 codec 语义给出）。新增测试 `cheetah-codec/tests/future_protocol_adapter_contract.rs` 覆盖契约字段与错误路径。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 04 任务 4.1（全协议全 codec 矩阵）。新增 `cheetah-rtmp-module/tests/rtmp_publish_play_matrix.rs`，补齐 RTMP publish -> RTMP play 同协议回归并验证时间戳单调与 100ms 步进；结合既有 `bridge_rtsp_rtmp` / `bridge_rtmp_rtsp`（RTSP TCP/UDP <-> RTMP 双向桥接）和 RTSP 同协议回归（`play_pause`、`udp_forwarding`）形成全协议组合矩阵。补齐编码矩阵缺口：`cheetah-rtmp-module` RTSP->RTMP 时间归一化矩阵新增 H266 与 G711U，`cheetah-rtsp-module` raw RTP 时间戳保留矩阵新增 G711U。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 03 任务 3.5（慢订阅者与积压回归）。`cheetah-engine` 的 `SubscriberApi::subscribe` 新增参数校验：`queue_capacity` 必须大于 0 且不得小于 `bootstrap_policy.max_bootstrap_frames`，避免 bootstrap 历史窗口与订阅队列上界冲突导致静默截断。新增回归覆盖“慢订阅者不拖累快订阅者与发布者分发结果”“高帧率视频 + 低码率音频 + 大 GOP + 断流重连场景下 bootstrap 不跨 discontinuity 且遵守 `max_bootstrap_frames` 上界”“非法订阅窗口参数返回 `InvalidArgument`”。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-engine --all-targets -- -D warnings`、`cargo clippy -p cheetah-sdk --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-sdk`、`cargo test -p cheetah-engine`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 03 任务 3.4（RTSP play 启动 pacing）。`cheetah-rtsp-module` 在 `handle_play` 发送循环新增 runtime-neutral `PlayStartPacingState`：首个媒体帧立即发送，后续 RTP 输出按统一媒体毫秒时间线节奏发送；`FrameFlags::DISCONTINUITY`、大幅时间戳回退或异常前跳时自动重建 pacing 基准，避免 reset/restart 后继续沿用旧基准。pacing 决策在 RTP 实际发送前执行，`TCP interleaved` 与 `UDP unicast` 共用同一逻辑，避免单 track 音视频积压触发启动快放。新增单测覆盖“首帧立即+后续按 delta 延迟”“discontinuity/回退重建基准”“音视频交织共享单时间线”“媒体时间戳优先级与 timebase 转换”。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-sdk --all-targets -- -D warnings`、`cargo clippy -p cheetah-engine --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-sdk`、`cargo test -p cheetah-engine`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 03 任务 3.3（RTMP play 启动 pacing）。`cheetah-rtmp-module` 新增 runtime-neutral 启动 pacing 状态机 `PlayStartPacingState`：metadata/codec config 后首个媒体帧立即发送，后续 bootstrap 媒体帧按统一媒体毫秒时间线节奏发送；音视频共享单时间线，避免音频积压触发视频快放；在 `FrameFlags::DISCONTINUITY`、大幅 timestamp 回退或异常超前时间差场景下自动重建 pacing 基准，避免 reset/restart 后沿用旧基准。新增单测覆盖“首帧立即+后续按 delta 延迟”“discontinuity/回退重建基准”“音视频交织不触发快放”。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-sdk --all-targets -- -D warnings`、`cargo clippy -p cheetah-engine --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-sdk`、`cargo test -p cheetah-engine`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 03 任务 3.2（RingBuffer live-tail bootstrap）。`cheetah-engine` 的 RingBuffer bootstrap 选择新增 `discontinuity` 边界裁剪：在 `max_bootstrap_frames` / `max_bootstrap_age_ms` 裁剪之后、随机访问点选择之前，先将起点收敛到窗口内最近 `FrameFlags::DISCONTINUITY`；当 reset 后窗口内尚无新随机访问点且 `wait_for_next_random_access_point=true` 时，bootstrap 为空并等待下一个随机访问点；当等待关闭时，fallback 也不会跨越 discontinuity 回放旧 GOP。新增回归测试覆盖“reset 后等待 keyframe”与“fallback 不跨 discontinuity 边界”。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-sdk --all-targets -- -D warnings`、`cargo clippy -p cheetah-engine --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-sdk`、`cargo test -p cheetah-engine`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 03 任务 3.1（`SubscriberOptions` / `BootstrapPolicy` 设计）。`cheetah-sdk` 新增 `BootstrapMode`（`None`/`LiveTail`/`FullGop`）与 `BootstrapPolicy`，`SubscriberOptions` 改为策略对象并显式表达 `max_bootstrap_age_ms`、`max_bootstrap_frames`、`wait_for_next_random_access_point`；`cheetah-engine` RingBuffer bootstrap 改为策略驱动并补齐年龄窗口与随机访问点等待/回退测试；RTMP/RTSP module 仅选择并传递策略。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-sdk --all-targets -- -D warnings`、`cargo clippy -p cheetah-engine --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-sdk`、`cargo test -p cheetah-engine`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 02 任务 2.4（入站/出站兼容与告警清理）。`cheetah-rtsp-module` 将 publish 侧参数集缓存泛化为 `video_parameter_sets`，关键帧参数集补齐统一覆盖 `H264/H265/H266`，移除模块中 H264 专用补丁路径；`cheetah-codec::egress` 新增统一时间戳修复/告警策略函数（`repair_monotonic_timestamp`、`should_sample_timestamp_repair`、`should_emit_alert_threshold`），RTSP play/publish 与 RTMP ingest/egress 复用同一策略，减少跨协议重复修补；RTSP/RTMP timestamp 修复告警补齐 `protocol_ingress` 字段，统一结构化观测维度。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo test -p cheetah-rtsp-module`、`cargo test -p cheetah-rtmp-module`（含 RTSP<->RTMP 双向桥接回归）。
- 2026-04-29：完成 Phase 02 任务 2.3（RTSP/RTMP 出站导出视图统一）。`cheetah-codec` 新增统一出站时间导出模块 `egress`，集中 RTMP 毫秒时间戳/CTS 导出与 RTSP RTP 时间戳选择/换算；`cheetah-rtmp-module` egress 删除本地时间导出逻辑并改为调用 codec；`cheetah-rtsp-module` play 时间戳选择/换算改为委托 codec，保持现有行为并统一跨协议时间导出来源。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`（含 RTSP/RTMP 双向桥接回归）。
- 2026-04-29：完成 Phase 02 任务 2.2（RTMP 入站改为 codec normalizer）。`cheetah-rtmp-module` publish 入站由私有 `IngestTimestampState` 切换到 `cheetah-codec::TimestampNormalizer`（按音视频分轨状态管理、32-bit timestamp wrap 展开、非单调 DTS 修复、large backward reset 触发 normalizer reset、负 CTS 通过 `composition_offset` 归一化）；音频入站新增按 codec 样本数与采样率推导 `AVFrame::duration`（AAC/Opus/MP3/G711/ADPCM），并补充 RTMP 入站回归用例覆盖 wrap/reset/duration。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 02 任务 2.1（RTSP 入站改为 codec normalizer）。`cheetah-rtsp-module` publish 入站改为按轨道使用 `cheetah-codec::TimestampNormalizer`（32-bit RTP wrap 展开、单调 DTS 修复、结构化告警、断流标记透传），并将 RTP 入站帧时间改为原始 RTP timestamp 后统一归一化；会话状态新增 `timestamp_normalizers` 替换私有 `video_reorder`；补充/调整单测覆盖视频时间戳修复路径。附带修复 `bridge_rtsp_rtmp` / `bridge_rtmp_rtsp` 的 clippy 折叠匹配告警。`cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module` 全通过。
- 2026-04-29：完成 Phase 01 任务 1.4（codec 内核测试矩阵）。`cheetah-codec` 新增 `tests/media_kernel_matrix.rs` 覆盖时间线、AU 组装、参数集晚到/变化、RTP 乱序与 marker 噪声、视频（H264/H265/H266/AV1/VP8/VP9）与音频（AAC/Opus/G711A/G711U/MP3）矩阵；`video` 新增 `LengthPrefixedParseError` 以及 length-prefixed 严格解析接口（`AccessUnitAssembler::push_length_prefixed_checked` / `ParameterSetCache::update_from_length_prefixed_checked`）并补单测；`cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module -- -D warnings`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-29：完成 Phase 01 任务 1.3（通用 Access Unit 与参数集能力）。`cheetah-codec` 新增 `TrackInfo::codec_config_view()`（覆盖 H264/H265/H266、AV1/VP8/VP9、AAC/Opus/MP3/G711 配置语义与必需配置错误）、`AccessUnit` 媒体时间/随机访问/参数集需求元数据抽象以及 `AccessUnit::from_frame_units()`，`ParameterSetCache` 新增随机访问帧参数集需求判定并修正 H266 Annex-B 参数集识别；`cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module -- -D warnings`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-29：完成 Phase 01 任务 1.2（通用时间戳归一化器）。`cheetah-codec` 新增协议无关 `TimestampNormalizer`（支持 timebase 转换、wrap 展开、单调 DTS 修复、PTS/DTS 合法化、reset 断流标记、结构化告警），并补充对应单测；`cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module -- -D warnings`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-29：完成 Phase 01 任务 1.1（统一媒体时间模型）。`cheetah-codec` 新增 `AVFrame` duration 语义、`FrameTimingError`/`TrackInfoError`、`TrackInfo::media_timebase()` 以及时间语义回归测试；`cheetah-codec`、`cheetah-rtmp-module`、`cheetah-rtsp-module` 的 `clippy/test` 全通过。
- 2026-04-29：创建 `plans-3` 计划文档结构，所有任务初始化为“未开始”。
- 当前计划覆盖 RTSP->RTMP、RTMP->RTSP、RTSP TCP、RTSP UDP、RTMP 同协议推拉流以及后续 SRT/WebRTC 扩展。
- 当前计划明确“不只修 H264”，所有音视频编码都必须走统一媒体内核和协议适配路径。

## 渐进式执行顺序

1. 先完成 Phase 01，保证媒体时间、AU、参数集能力在 `cheetah-codec` 内可复用。
2. 再完成 Phase 02，让 RTSP/RTMP module 删除私有修正逻辑，只保留协议适配。
3. 再完成 Phase 03，解决 GOP 秒开和启动积压快放。
4. 最后完成 Phase 04，用矩阵测试锁定所有协议组合和未来协议扩展边界。

## 阶段完成后的统一检查

- `cargo fmt`
- `cargo clippy -p cheetah-codec`
- `cargo test -p cheetah-codec`
- `cargo clippy -p cheetah-rtmp-module`
- `cargo test -p cheetah-rtmp-module`
- `cargo clippy -p cheetah-rtsp-module`
- `cargo test -p cheetah-rtsp-module`
- 运行 RTSP/RTMP 双向桥接测试，确认无 `Invalid timestamps`、`Non-increasing DTS`、`Negative cts`。
