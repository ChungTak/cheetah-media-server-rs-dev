# Phase 01: cheetah-codec 双时间线模型

- 状态：进行中
- 范围：`cheetah-codec` 中的 source timeline 元数据、PTS-only 输入模式、canonical DTS 生成、时间线测试矩阵。
- 完成标准：RTSP 这类只有 RTP/PTS 语义的输入可以生成平滑 canonical DTS，RTMP 这类已有 DTS/CTS 的输入仍保持封装语义正确，所有逻辑可被 SRT/WebRTC 复用。

## 具体任务

### 1.1 源时间元数据模型

- [x] 为 `AVFrame` 增加可扩展 source timeline 表达，优先使用 side data 或新类型承载 source protocol、raw timestamp、unwrapped timestamp、epoch offset、sequence number、rtcp mapping。
- [x] 支持至少两类源：`RtpTimestamp` 与 `RtmpTimestamp`。
- [x] source timeline 不参与 RingBuffer 排序，不作为 bootstrap 随机访问点判断依据。
- [x] 日志和测试能同时输出 source timestamp 与 canonical pts/dts。

### 1.2 PTS-only normalizer 输入模式

- [x] 将 `TimestampNormalizeInput` 从隐式 `Option` 组合升级为显式输入模式，至少包含 `DtsPts`、`DtsWithCompositionOffset`、`PtsOnly`。
- [x] `PtsOnly` 用于 RTSP/WebRTC RTP 视频输入，表示源只提供展示时间或 RTP timestamp。
- [x] `DtsPts` 用于已明确提供 DTS/PTS 的封装输入。
- [x] `DtsWithCompositionOffset` 用于 RTMP/FLV 等 DTS + CTS 输入。

### 1.3 平滑 DTS 生成策略

- [x] 对 PTS-only 视频输入按 AU 到达顺序生成 canonical DTS。
- [x] DTS 步进优先使用帧 duration；没有 duration 时按相邻 PTS 差值、track clock rate、已观测平均帧间隔推导。
- [x] 不允许把正常 B 帧重排修成连续 `+1 tick` 的解码时间线。
- [x] 小幅重排只产生结构化 alert，不标记 discontinuity。
- [x] 大跳变、reset、publisher restart 才标记 discontinuity 并重建 DTS 生成状态。

### 1.4 codec 时间线测试矩阵

- [x] 覆盖 RTSP/RTP PTS-only H264 B 帧：canonical DTS 平滑递增，PTS 可重排。
- [x] 覆盖 RTSP/RTP PTS-only 无 B 帧：PTS 与 DTS 基本同向、帧间隔稳定。
- [x] 覆盖 RTMP DTS+CTS：RTMP timestamp 映射到 canonical DTS，CTS 映射到 PTS-DTS。
- [x] 覆盖 32-bit wrap、随机 RTP epoch、重复 timestamp、小幅回退、大幅 reset。
- [x] 覆盖 H264/H265/H266/AV1/VP8/VP9 和 AAC/Opus/G711A/G711U/MP3。

## 最新进展

- 2026-04-29：完成 1.4。新增 `crates/cheetah-codec/tests/media_kernel_matrix.rs` 三组矩阵测试：`PtsOnly` B 帧重排单调 DTS、`PtsOnly` 非 B 帧稳定步进、RTMP DTS+CTS 与 wrap/reset 边界行为；覆盖视频 H264/H265/H266/AV1/VP8/VP9 与音频 AAC/Opus/G711A/G711U/MP3。验证命令通过：`cargo fmt`、`cargo clippy -p cheetah-codec`、`cargo test -p cheetah-codec`，并额外通过 `cheetah-rtsp-module` 与 `cheetah-rtmp-module` clippy/test 回归。
- 2026-04-29：完成 1.3。`DtsGenerator` 重构为 PTS-only 平滑策略：新增 `frame_duration` 输入提示、相邻 PTS 差值步进、历史步进平滑；新增 `PtsReorderObserved` 告警用于小幅重排且不触发 discontinuity；对大跳变保留正向大步进并标记 `TimelineDiscontinuityDetected`，同时重建内部步进状态。RTSP publish 增加 `fallback_step_for_publish_frame()`（duration > fps > clock rate）作为 PTS-only cadence 提示，跨协议桥接长时程回归通过。
- 2026-04-29：完成 1.2。`cheetah-codec` 引入显式 `TimestampNormalizeMode`（`DtsPts`、`DtsWithCompositionOffset`、`PtsOnly`，并保留 `NoTimestamp` 以兼容 fallback-only 通用场景）；RTSP publish ingress 视频切换 `PtsOnly`、音频切换 `DtsPts`；RTMP ingest 视频切换 `DtsWithCompositionOffset(cts)`、音频切换 `DtsWithCompositionOffset(None)` 保持 `pts==dts`。新增 `PtsOnly` 正向测试并修复 RTMP 音频时间回退回归测试，确保无回归。
- 2026-04-29：完成 1.1。`cheetah-codec` 新增 `SourceTimestamp::{Rtp,Rtmp}`、`RtpTimestamp`、`RtmpTimestamp`、`RtpRtcpMapping`，并在 `AVFrame` 增加 `set_source_timestamp()/source_timestamp()` 统一接口；RTSP `build_frame_from_rtp` 对所有 codec 写入 RTP source timeline（含 sequence 与可选 RTCP SR mapping），RTMP 音视频 ingest 写入 RTMP source timeline。新增 `cheetah-codec`、`cheetah-rtsp-module`、`cheetah-rtmp-module` 单测覆盖 source/canonical 并行观测与 side data 替换语义。
- 2026-04-29：计划已创建，任务未开始。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
```

若 `AVFrame`、`TrackInfo` 或 timestamp API 改变，继续运行受影响的 RTSP/RTMP module 测试并同步更新 `SystemArchitecture.md`。
