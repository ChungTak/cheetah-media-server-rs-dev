# Phase 01 — 共享 MP4 / FLV / HLS / PS 容器能力

- **状态**: 已完成
- **范围**: 在 `cheetah-codec` 中补齐 classic MP4 读写、统一 record writer 事件、PS 文件 writer 和 HLS 录制导出视图
- **完成标准**: `cheetah-codec` 可独立完成 MP4 文件 mux/demux/seek 和 `FLV/HLS/MP4/PS` 录制导出，为 record module 与 mp4 module 提供稳定接口

## 实现概览

- `cheetah-codec::mp4` 拆分为 `box_parser`、`sample_table`、`sample_entry`、`writer`、`reader`、`compat` 子模块；`Mp4Reader` 为 Sans-I/O 风格、通过 `Mp4ReadRequest` 让驱动层填充字节，支持 `moov` 在头/尾两种布局。
- `cheetah-codec::record` 新增统一 `RecordContainerWriter` trait 与 `RecordFormat`/`RecordWriteEvent`/`RecordDiagnostic` 模型，包含 `flv/hls/mp4/ps` 四个文件 writer 实现。
- `Mp4Writer` 输出 `ftyp + mdat + moov` 的 classic VOD MP4，自动写 `stss/ctts/stco-or-co64`，覆盖 H264/H265/AAC/G711/Opus/MP3/MJPEG/VP8/VP9/AV1。
- HLS record writer 默认输出 fMP4 segment 与 VOD playlist；FLV writer 输出连续 FLV 文件流；PS writer 复用 `PsMuxer`。
- `cargo test -p cheetah-codec --lib` 全部通过（212 用例）。

## 1.1 升级 classic MP4 file 能力

新增或扩展：

- `mp4.rs` 拆成 box parser、sample table、writer、reader、compat 小模块
- 支持 `ftyp/moov/mvhd/trak/mdia/minf/stbl/stsd/stts/ctts/stss/stsc/stsz/stco/co64`
- 支持 track 建模、duration、first dts、seek table、sample flags
- 支持多轨音视频和 `H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1`

要求：

- `reader` 允许 `moov` 在前或在后
- seek 必须 bounded，不允许整文件线性扫描作为正常路径
- 缺失 `stss` 时对视频采用 sample flags 或最近随机访问点回退

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
- 补齐 sequence header、metadata、end marker 策略

PS：

- 在现有 `ps.rs` 基础上补文件 writer
- 复用现有 demux/mux 和 RTP 场景时间戳逻辑
- 面向 GB28181 主路径先覆盖 `H264/H265/AAC/G711/MP3`

HLS：

- 提供 record writer 级 segment/playlist 视图
- 默认 fMP4 segment，兼容 TS legacy 模式
- finalize 时输出完整 VOD playlist

## 1.4 Phase 01 测试要求

- MP4 reader/muxer roundtrip
- MP4 多轨 seek 回归
- `moov` 在尾部、缺失 `stss`、异常 `ctts`、超大 `co64` 回归
- FLV enhanced codec writer 回归
- PS writer/demux roundtrip
- HLS VOD playlist、segment 边界和 finalize 回归
- property tests 覆盖 sample table 与 seek 单调性
- fuzz 覆盖 MP4 box parser、sample table builder、PS writer/demux
