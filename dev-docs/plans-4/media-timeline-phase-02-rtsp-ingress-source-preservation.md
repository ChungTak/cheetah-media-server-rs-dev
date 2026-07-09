# Phase 02: RTSP 入站源时间保真

- 状态：已完成
- 范围：RTSP publish 入站从“RTP timestamp 直接归一化为 DTS/PTS”改为“保留源 RTP timestamp，生成 canonical DTS/PTS”。
- 完成标准：RTSP 入站 B 帧素材不再产生持续 `NonMonotonicDtsRepaired`，RTSP 同协议播放顺畅，RTSP->RTMP 仍 GOP 秒开且时间戳合法。

## 具体任务

### 2.1 RTSP RTP source timestamp 保留

- [x] `build_frame_from_rtp` 生成帧时保留 RTP timestamp source metadata。
- [x] RTP timestamp 作为 source PTS 或 presentation timestamp 输入，不再直接填入 canonical DTS。
- [x] 继续保留 sequence、marker、AU start/end、keyframe、参数集等源协议信息。
- [x] TCP interleaved 与 UDP unicast 只影响收包路径，不影响 source/canonical 映射规则。

### 2.2 H26x AU 顺序驱动 canonical DTS

- [x] H264/H265/H266 depacketize 输出完整 AU 后再生成 canonical 时间。
- [x] DTS 按 AU 到达顺序和平滑步进生成，PTS 来自 source RTP timestamp 展开和 rebasing。
- [x] 如果 B 帧导致 PTS 与 DTS 有 composition offset，保留 `B_FRAME` 语义，但不压缩 DTS 到 `+1 tick`。
- [x] 参数集补发仍由 `cheetah-codec` 管理，不在 RTSP module 写 H264 专用补丁。

### 2.3 RTSP TCP/UDP 入站一致性

- [x] 对同一输入素材，RTSP TCP 和 UDP publish 进入 engine 后 canonical `AVFrame` 序列一致。
- [x] UDP 丢包或乱序只通过 packet loss/corruption/discontinuity 表达，不改变正常路径时间模型。
- [x] RTCP SR 只用于 source RTP 与 wallclock 的映射和排障，不直接覆盖 canonical DTS。

### 2.4 RTSP 入站告警降噪与语义校正

- [x] 正常 B 帧重排不应持续输出 `rtsp publish timestamp repaired by codec normalizer` 告警。
- [x] 告警区分 source disorder、canonical repair、discontinuity、egress repair。
- [x] 所有告警带 `stream_key/track_id/codec/source_ts/pts/dts/protocol_ingress`。
- [x] 修复后更新排障手册，避免把正常 B 帧误判为入站异常。

## 最新进展

- 2026-04-29：完成 2.4。`normalize_publish_frame_timestamps` 引入入站告警分级：`RtspPublishAlertClass::{SourceDisorder, CanonicalRepair, Discontinuity}`，并为每类分别计数与采样阈值告警。正常 B 帧重排（`PtsReorderObserved`）仅归类为 `source_disorder`（debug 采样），不再计入 `canonical_repair`；真实修复（如 `NonMonotonicDtsRepaired`）与 discontinuity 继续 warn。日志统一补齐 `protocol_ingress`、`alert_class`、`source_pts/source_dts/pts/dts` 等排障字段。新增单测 `classify_rtsp_publish_alert_class_prioritizes_discontinuity_then_repair_then_disorder` 与 `normalize_publish_frame_timestamps_tracks_bframe_reorder_as_source_disorder_only`。验证通过：`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`（含 bridge 测试）。
- 2026-04-29：完成 2.3。新增 `crates/cheetah-rtsp-module/tests/bridge_rtsp_rtmp.rs` 集成回归：`rtsp_tcp_and_udp_publish_produce_consistent_rtmp_timestamps` 验证同一 RTP 时间序列在 RTSP TCP/UDP publish 下导出的 RTMP 时间戳序列一致；`rtsp_udp_publish_loss_and_reorder_keeps_canonical_timeline_monotonic` 验证 UDP 丢包与乱序场景下 canonical 时间线保持单调不回退。并结合 `build_frame_from_rtp_records_rtcp_sender_report_mapping_when_available` 单测确认 RTCP SR 仅写入 source mapping 元数据，不覆盖 canonical DTS。验证通过：`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`（含 bridge_rtsp_rtmp）。
- 2026-04-29：完成 2.2。RTSP publish 入站为 H265/H266 新增 AU depacketizer 状态机（FU/AP/单 NAL 支持、大小上界保护、timestamp/marker 边界驱动），组帧输出 `FrameFormat::CanonicalH26x` 后统一进入 `normalize_publish_frame_timestamps` 的 `PtsOnly` 路径生成 canonical DTS。`SETUP` 阶段将 H265/H266 与 H264 同样预分配 depacketizer，保证 TCP interleaved 与 UDP unicast 收包路径共享同一组帧规则。新增 H265 回归测试覆盖 source timestamp 保留与 FU 跨包 AU 边界。验证通过：`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`（含 bridge_rtsp_rtmp、bridge_rtmp_rtsp）。
- 2026-04-29：完成 2.1。RTSP publish 入站新增/扩展回归测试：`build_frame_from_rtp_h264_emits_source_timestamp_metadata` 覆盖 H264 AU 输出时保留 RTP source timestamp（含 sequence）；`normalize_publish_frame_timestamps_ignores_raw_video_dts_input_for_all_video_codecs` 覆盖 H264/H265/H266/AV1/VP8/VP9 统一 `PtsOnly` 语义；`normalize_publish_frame_timestamps_preserves_audio_dts_input_for_all_audio_codecs` 覆盖 AAC/Opus/G711A/G711U/MP3 统一 `DtsPts` 语义。并通过 `cargo clippy -p cheetah-rtsp-module` 与 `cargo test -p cheetah-rtsp-module`（含 TCP/UDP bridge）。
- 2026-04-29：计划已创建，任务未开始。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-module --test bridge_rtsp_rtmp
```

必须额外执行一次 RTSP/TCP 推流与 RTMP late join 拉流抓包，确认 GOP 秒开和首 1 秒播放速率稳定。
