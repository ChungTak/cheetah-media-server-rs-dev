# Phase 01 — 共享 MP4 / FLV / HLS / PS 容器能力

- **状态**: 已完成
- **范围**: 在 `cheetah-codec` 中补齐 classic MP4 读写、统一 record writer 事件、FLV/PS 文件 writer 和 HLS/HLS-FMP4 录制导出视图
- **完成标准**: `cheetah-codec` 可独立完成 MP4 文件 mux/demux/seek 和 `FLV/HLS/MP4/PS` 录制导出，为 record module 与 mp4 module 提供稳定接口

## 实现概览

- 与 `plans-26-mp4-sms` Phase 01 共享同一套 `cheetah-codec::mp4` / `cheetah-codec::record` 实现：classic MP4 reader + writer、`RecordContainerWriter` 接口与 `flv/hls/mp4/ps` 文件 writer。
- 新增 ZLM 兼容专用扩展：
  - `Mp4FileWriterConfig::drop_below_bytes` 对应 ZLM `MP4Recorder::asyncClose()` 的 1024B 弃文件阈值；finalize 时低于阈值返回 `RecordDiagnostic::DropTinyFile`。
  - `RecordDiagnostic::DropTinyFile { size_bytes, threshold_bytes }` 让 runtime 把 `.part` 删除而不是 rename。
- HLS record writer 默认 fMP4，TS legacy 模式仍兼容；FLV writer 输出连续 FLV 文件流；PS writer 复用 `PsMuxer`。
- `cargo test -p cheetah-codec --lib` 全部通过（213 用例）。

## 1.1 升级 classic MP4 file 能力

新增或扩展：

- `mp4.rs` 拆成 box parser、sample table、writer、reader、compat 小模块
- 支持 `ftyp/moov/mvhd/trak/mdia/minf/stbl/stsd/stts/ctts/stss/stsc/stsz/stco/co64`
- 支持 track 建模、duration、first dts、seek table、sample flags
- 支持多轨音视频和 `H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1`

要求：

- reader 允许 `moov` 在前或在后
- seek 必须 bounded，不允许整文件线性扫描作为正常路径
- 缺失 `stss` 时对视频采用 sample flags 或最近随机访问点回退
- MP4 录制时间戳从 0 起算，保留 source timestamp 作为 side data

## 1.2 建立统一 record writer 接口

新增共享抽象：

- `RecordFormat`
- `RecordWriteEvent`
- `RecordDiagnostic`
- `RecordContainerWriter`

职责：

- writer 只负责容器导出，不负责磁盘 I/O
- HLS writer 输出 `segment/init/playlist` 事件
- MP4/FLV/PS writer 输出 `bytes` 或 `segment` 事件
- 所有 writer 都使用 canonical timeline，不回写 source timestamp

## 1.3 补 FLV / PS / HLS 录制导出能力

FLV：

- 在现有 `flv.rs` 基础上补文件 writer
- 支持 classic 和 enhanced codec 映射
- 补齐 sequence header、metadata、previous tag size、end marker 策略

PS：

- 在现有 `ps.rs` 基础上补文件 writer
- 复用现有 demux/mux 和 RTP 场景时间戳逻辑
- 面向 GB28181 主路径先覆盖 `H264/H265/AAC/G711/MP3`

HLS：

- 提供 record writer 级 segment/playlist 视图
- 默认 fMP4 segment，兼容 TS legacy 模式
- finalize 时输出完整 VOD playlist

TS / FMP4：

- 作为 registry 扩展格式保留
- 复用现有 `ts_mux`、`fmp4_mux`
- 不作为首批验收阻塞项

## 1.4 ZLM 兼容点

- 对齐 `MP4Muxer` 的 faststart / fMP4 双模式能力
- 对齐 `MP4Recorder` 的关键帧切片策略
- 对齐 `HlsRecorder` 的 TS HLS 与 HLS-FMP4 双路径
- 对齐 `FlvRecorder` 的文件缓存和完整 FLV tag 写入语义

## 1.5 Phase 01 测试要求

- MP4 reader/muxer roundtrip
- MP4 多轨 seek 回归
- `moov` 在尾部、缺失 `stss`、异常 `ctts`、超大 `co64` 回归
- FLV enhanced codec writer 回归
- PS writer/demux roundtrip
- HLS VOD playlist、segment 边界和 finalize 回归
- property tests 覆盖 sample table 与 seek 单调性
- fuzz 覆盖 MP4 box parser、sample table builder、PS writer/demux、FLV tag writer
