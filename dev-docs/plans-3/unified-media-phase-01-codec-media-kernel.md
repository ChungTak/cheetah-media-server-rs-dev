# Phase 01: cheetah-codec 统一媒体内核

- 状态：已完成
- 范围：`cheetah-codec` 内的媒体时间模型、时间戳归一化、Access Unit 拼装、参数集缓存/补发、codec 级测试。
- 完成标准：RTSP、RTMP 以及后续协议都能通过同一套 `AVFrame + TrackInfo` 语义表达音视频时间线和随机访问点，协议模块不再维护私有时间戳修正器或参数集缓存。

## 具体任务

### 1.1 统一媒体时间模型

- [x] 梳理 `AVFrame` / `TrackInfo` 中 PTS、DTS、duration、timebase、随机访问点、断流标记的语义。
- [x] 明确所有音视频格式进入引擎后的统一时间单位和转换规则。
- [x] 明确 B 帧、有 CTS、无 DTS、重复时间戳、时间戳回绕、断流重连的处理边界。
- [x] 确认 `cheetah-codec` 公共接口不泄漏 FFmpeg 类型，也不包含 RTMP/RTSP 协议状态机。

### 1.2 通用时间戳归一化器

- [x] 在 `cheetah-codec` 中提供协议无关的 timestamp normalizer。
- [x] 支持 timebase 转换、DTS 单调生成、PTS/DTS 合法化、回绕展开、重连 reset、断流标记。
- [x] 支持视频帧和音频帧，不把逻辑写死到 H264。
- [x] 为异常输入保留兼容策略和可观测告警，而不是在协议热路径临时修补。

### 1.3 通用 Access Unit 与参数集能力

- [x] 在 `cheetah-codec` 中抽象 Access Unit 拼装结果，包含完整帧、媒体时间、随机访问点、参数集需求。
- [x] 支持 H264、H265、H266 的参数集缓存/补发。
- [x] 支持 AV1、VP8、VP9、AAC、Opus、G711A、G711U、MP3 等格式的配置帧或 codec config 语义。
- [x] 参数集补发只由统一媒体内核决定，RTSP/RTMP module 只消费导出视图。

### 1.4 codec 内核测试矩阵

- [x] 覆盖无 B 帧、有 B 帧、重复 DTS、负 CTS、timestamp 回绕、断流重连、乱序片段。
- [x] 覆盖多 RTP 包组成一个 AU、marker 缺失、marker 重复、参数集晚到、参数集变化。
- [x] 覆盖视频格式 H264、H265、H266、AV1、VP8、VP9。
- [x] 覆盖音频格式 AAC、Opus、G711A、G711U、MP3。

## 最新进展

- 2026-04-29：完成任务 1.4。`cheetah-codec` 新增 `tests/media_kernel_matrix.rs`，覆盖时间戳回绕/重复 DTS/负 CTS 与 reset、无 B 帧与 B 帧边界、RTP 分包乱序与 marker 噪声、参数集晚到与旋转、视频（H264/H265/H266/AV1/VP8/VP9）和音频（AAC/Opus/G711A/G711U/MP3）codec config 矩阵；`video` 新增 `LengthPrefixedParseError`、`AccessUnitAssembler::push_length_prefixed_checked()`、`ParameterSetCache::update_from_length_prefixed_checked()`，对零长度 NAL、截断 NAL、不完整长度前缀提供结构化错误并补充单测；`cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module -- -D warnings`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-29：完成任务 1.3。`cheetah-codec` 新增 `CodecConfigRequirement/CodecConfigPayload/CodecConfigView/CodecConfigError` 与 `TrackInfo::codec_config_view()`，统一表达 H264/H265/H266、AV1/VP8/VP9、AAC/Opus/MP3/G711 配置语义；`video` 新增 `AccessUnitTiming`、`ParameterSetRequirement`、`AccessUnitBuildError` 与 `AccessUnit::from_frame_units()`；`ParameterSetCache` 新增随机访问帧参数集需求判定并修正 H266 参数集 NAL 类型识别；`cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module -- -D warnings`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-29：计划已创建，任务未开始。
- 2026-04-29：完成任务 1.1。`cheetah-codec::AVFrame` 增加 `duration/duration_us` 与 `FrameTimingError` 校验接口，`TrackInfo` 增加 `media_timebase()` 与 `TrackInfoError`；补充统一时间语义单测；`cargo fmt`、`cargo clippy -p cheetah-codec`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module` 全通过。
- 2026-04-29：完成任务 1.2。`cheetah-codec::time` 新增 `TimestampNormalizer`、`TimestampNormalizerConfig`、`TimestampNormalizeInput/Output`、`TimestampAlert` 与配置/运行时错误类型；覆盖 wrap 展开、非单调修复、负 CTS 策略、fallback 推导、reset 断流标记单测；`cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module -- -D warnings`、`cargo test -p cheetah-rtsp-module` 全通过。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-codec`
- `cargo test -p cheetah-codec`
- 若公共模型变更，继续运行 RTSP/RTMP module 的相关测试。
