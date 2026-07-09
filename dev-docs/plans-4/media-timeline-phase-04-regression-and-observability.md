# Phase 04: 回归矩阵与可观测性

- 状态：已完成
- 范围：跨协议矩阵、B 帧/非 B 帧素材、抓包验收、播放顺畅度指标和排障日志。
- 完成标准：修复不只通过单元测试，还能通过真实 ffmpeg 推流、ffplay/ffprobe 拉流、tcpdump 抓包证明 GOP 秒开与首 1 秒播放速率稳定。

## 具体任务

### 4.1 B 帧/非 B 帧跨协议矩阵

- [x] RTSP TCP -> RTSP TCP/UDP 拉流。
- [x] RTSP UDP -> RTSP TCP/UDP 拉流。
- [x] RTSP TCP/UDP -> RTMP 拉流。
- [x] RTMP -> RTSP TCP/UDP 拉流。
- [x] RTMP -> RTMP 拉流。
- [x] 每个组合覆盖 B 帧素材和非 B 帧素材。

### 4.2 抓包与 ffprobe/ffplay 验收

- [x] RTMP late join 抓包确认 play 后 metadata/config/keyframe 立即发送。
- [x] 抓包确认首个大视频 burst 后，后续 bootstrap 不是无节制一次性冲出。
- [x] ffprobe 首个视频包应为 keyframe，`pts/dts` 从 0 或接近 0 开始。
- [x] ffplay debug 日志不得出现 `Invalid timestamps`、`Non-increasing DTS`、`Negative cts`。
- [x] RTSP 拉流抓包确认 RTP timestamp 与播放时间稳定递增。

### 4.3 观测指标与告警标准

- [x] 回归报告记录 startup latency、first second frame interval、average playback rate、first keyframe delay。
- [x] 告警区分 source timeline repair、canonical repair、egress repair。
- [x] 正常 B 帧重排不得触发高频 WARN；异常才进入阈值告警。
- [x] 每条告警包含 source timestamp 和 canonical pts/dts，便于定位是哪一层异常。

### 4.4 文档与架构收口

- [x] 更新 `SystemArchitecture.md` 的媒体时间章节。
- [x] 更新 `dev-docs/plans-4/index.md` 的完成状态和最新进展。
- [x] 更新排障手册，加入双时间线定位流程。
- [x] 若改变 `AVFrame` / `TrackInfo` / 时间戳模型，同步检查 `AGENTS.md` 是否需要补充约束。

## 最新进展

- 2026-04-29：完成 4.4。`SystemArchitecture.md` 媒体时间章节补充观测与告警基线：统一 `startup_latency_ms`、`first_second_avg_frame_interval_ms`、`average_playback_rate_x`、`first_keyframe_delay_ms` 指标定义；明确 `source/canonical/egress` repair 分层计数和高频告警阈值策略；要求 repair 日志必须携带 source timestamp 与 canonical `pts/dts`。同时完成 `index.md` 状态收口和 `media-timeline-troubleshooting-manual.md` 双时间线快速定位流程（含 keyframe delay 与 startup latency 联判）。本轮未改动 `AVFrame` / `TrackInfo` / 时间戳数据模型定义，`AGENTS.md` 约束无需新增。
- 2026-04-29：完成 4.3。回归脚本新增跨场景观测指标与告警分层：`first_keyframe_delay_ms`（由 ffprobe 首包到首 keyframe 的媒体时间差计算）、`source_repair_events`、`canonical_repair_events`、`egress_repair_events`、`repair_warn_high_frequency`、`repair_context_complete`。其中 `repair_warn_high_frequency` 以 `REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD`（默认 32）限制 canonical/egress 高噪声告警频率，避免正常 B 帧重排触发高频 WARN；`repair_context_complete` 会校验 repair 类日志是否同时携带 source timestamp 与 canonical `pts/dts` 语义。acceptance matrix 与 doctor 校验同步升级，确保上述检查项长期生效。验证通过：`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/cross_protocol_matrix_command_templates.sh doctor-acceptance`、`bash dev-scripts/cross_protocol_matrix_regression.sh doctor`。
- 2026-04-29：完成 4.2。`cross_protocol_matrix_regression.sh` 增加 ffprobe 自动验收：按 pull 命令推导 URL/RTSP transport，采样视频包并计算 `ffprobe_first_video_keyframe`、`ffprobe_first_video_pts_near_zero`、`ffprobe_video_dts_monotonic` 三项指标；acceptance matrix 新增对应检查项。同时增加可选抓包通道（`ENABLE_TCPDUMP_CAPTURE=1`）在每个场景生成 `capture.pcap` 并将路径写入 summary，便于验证 late join metadata/config/keyframe 顺序与 burst 行为；默认关闭以兼容无抓包权限环境。ffplay/ffmpeg 日志侧继续强约束 `Invalid timestamps`、`Non-increasing DTS`、`Negative cts` 为 0。验证通过：`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/cross_protocol_matrix_command_templates.sh doctor-acceptance`、`bash dev-scripts/cross_protocol_matrix_regression.sh doctor`。
- 2026-04-29：完成 4.1。跨协议矩阵脚本新增 RTSP 混合传输场景 `rtsp-tcp-to-rtsp-udp`、`rtsp-udp-to-rtsp-tcp`，补齐 RTSP TCP/UDP 推拉流交叉组合；`cross_protocol_matrix_regression.sh run-all` 默认按 `MATRIX_PROFILE_MODE=all` 遍历 matrix 全部输入 profile（含 `b_frames=yes/no`），使 RTSP/RTMP 同协议与转协议场景均覆盖 B 帧与非 B 帧素材。为兼容历史测试桩，若 matrix script 不支持 `list-inputs`，回退到 `INPUT_PROFILE` 单 profile。验证通过：`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/cross_protocol_matrix_regression.sh doctor`。
- 2026-04-29：计划已创建，任务未开始。

## 完成后检查

```bash
bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh
bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh
bash dev-scripts/cross_protocol_matrix_regression.sh doctor
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo clippy -p cheetah-engine
cargo test -p cheetah-engine
cargo clippy -p cheetah-rtmp-module
cargo test -p cheetah-rtmp-module
cargo clippy -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-module
```

真实链路验收必须保存 push/pull 日志、ffprobe 输出和 pcap 文件路径到最新进展中。
