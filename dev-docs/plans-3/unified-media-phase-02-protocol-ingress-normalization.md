# Phase 02: 协议入口与出口统一适配

- 状态：已完成
- 范围：RTSP/RTMP module 的 ingress normalizer 接入、egress export view 接入、私有媒体修正逻辑清理。
- 完成标准：RTSP/RTMP 同协议推拉流和双向转协议都通过 `cheetah-codec` 统一媒体内核，不再在 module 内复制时间戳修正、NALU 处理或参数集缓存逻辑。

## 具体任务

### 2.1 RTSP 入站改为 codec normalizer

- [x] RTSP RTP packet 输入只在 driver/module 完成收包、排序边界和 transport 适配。
- [x] RTP timestamp、marker、payload header 进入 `cheetah-codec` 后拼装为 `AVFrame`。
- [x] TCP interleaved 和 UDP transport 只影响网络收发，不影响进入引擎后的时间模型。
- [x] RTSP publish 对 H264、H265、AAC、Opus 等格式统一走 codec adapter。

### 2.2 RTMP 入站改为 codec normalizer

- [x] RTMP/FLV tag 输入通过 `cheetah-codec` 处理 DTS、CTS、PTS 和 codec config。
- [x] 支持 timestamp reset、timestamp extended、回绕、重复 DTS、负 CTS 的兼容归一化。
- [x] 音频格式按采样率和 samples 推导 duration，不把启动时间线写死为视频逻辑。
- [x] RTMP publish 进入引擎前统一输出 `AVFrame + TrackInfo`。

### 2.3 RTSP/RTMP 出站导出视图统一

- [x] RTMP play 通过 `cheetah-codec` 导出 FLV tag 所需的 timestamp、CTS、codec config。
- [x] RTSP play 通过 `cheetah-codec` 导出 RTP packet 所需的 RTP timestamp、marker、payload 分片。
- [x] RTSP->RTMP、RTMP->RTSP 转协议不再绕过统一媒体内核。
- [x] 出站逻辑只负责协议封包、session 状态和发送 backpressure。

### 2.4 入站/出站兼容与告警清理

- [x] 清理协议模块中的重复 timestamp repair、param cache、NALU prepend 逻辑。
- [x] 将兼容策略集中到 `cheetah-codec` 或明确 compat 层。
- [x] 对脏数据保留结构化日志，日志中标注 stream key、track id、codec、协议入口。
- [x] 降低正常推拉流中的 `Invalid timestamps`、`Non-increasing DTS`、`Negative cts` 风险。

## 最新进展

- 2026-04-29：完成任务 2.4。`cheetah-rtsp-module` 将 publish 侧私有 `h264_parameter_sets` 统一为 `video_parameter_sets`，关键帧参数集补齐改为 `H264/H265/H266` 通用路径（`ParameterSetCache::update_from_annexb + prepend_to_annexb_access_unit`）；`cheetah-codec::egress` 新增统一时间戳修复与告警策略函数（`repair_monotonic_timestamp`、`should_sample_timestamp_repair`、`should_emit_alert_threshold`），并在 RTSP play、RTSP publish、RTMP ingest/egress 复用，清理模块侧重复策略实现；RTSP/RTMP 的时间戳修复告警补齐 `protocol_ingress` 结构化字段，统一脏数据观测维度。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo test -p cheetah-rtsp-module`（含 RTSP<->RTMP 双向桥接）、`cargo test -p cheetah-rtmp-module`。
- 2026-04-29：完成任务 2.3。`cheetah-codec` 新增统一出站时间导出模块 `egress`，集中提供 RTMP 毫秒时间戳与 CTS 导出（含负值钳制）以及 RTSP RTP 出站时间戳选择/换算（音视频主副时间戳优先级、fallback、wrap 语义）；`cheetah-rtmp-module` egress 删除本地 `timebase->ms` 与 `composition_time` 私有实现，改为直接调用 `cheetah-codec` 导出函数；`cheetah-rtsp-module` play 的 RTP 时间戳选择/换算改为委托 `cheetah-codec`，保持现有行为并统一跨协议时间导出来源。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`（含 RTSP/RTMP 双向桥接回归）。
- 2026-04-29：完成任务 2.2。`cheetah-rtmp-module` 移除私有 `IngestTimestampState`，发布入站按音视频分轨接入 `cheetah-codec::TimestampNormalizer`，统一处理 RTMP tag DTS/CTS/PTS（含 32-bit wrap、重复/回退 DTS 修复、large backward reset 触发 normalizer reset、负 CTS 兼容）；音频入站新增 codec 样本数与采样率驱动的 `AVFrame::duration` 推导（AAC/Opus/MP3/G711/ADPCM）；补充 RTMP 入站回归测试覆盖 wrap/reset/duration。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成任务 2.1。RTSP publish 入站按 track 引入 `cheetah-codec::TimestampNormalizer`（RTP 32-bit wrap 展开、单调 DTS 修复、告警采样与阈值告警、断流标记透传），并将 `build_frame_from_rtp` 入口时间改为原始 RTP timestamp 后统一归一化；`PublishSession` 由私有 `video_reorder` 切换为 `timestamp_normalizers`，保持 TCP interleaved 与 UDP 仅在 transport 层分流，不影响进入引擎后的时间语义。完成 `cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`，并通过 RTSP/RTMP 双向桥接测试。
- 2026-04-29：计划已创建，任务未开始。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-rtsp-module`
- `cargo test -p cheetah-rtsp-module`
- `cargo clippy -p cheetah-rtmp-module`
- `cargo test -p cheetah-rtmp-module`
- 运行 RTSP->RTMP、RTMP->RTSP 桥接测试。
