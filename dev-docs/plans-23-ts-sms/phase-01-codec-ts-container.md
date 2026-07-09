# Phase 01 — 共享 MPEG-TS 容器能力

- **状态**: 规划中
- **范围**: 在 `cheetah-codec` 中建立标准 MPEG-TS mux/demux，补齐 MP2、多编码、多轨、鲁棒性，并让 HLS 复用共享实现
- **完成标准**: `cheetah-codec` 可独立完成 TS mux/demux roundtrip；HLS TS 路径不再维护私有容器分叉；core 测试、property test、fuzz build 通过

---

## 1.1 媒体模型补 MP2

**问题**: 用户要求支持 MP2，但当前 `CodecId` 只有 `MP3`，无法区分 MPEG-1 Layer II 与 Layer III。

**实现方案**:

- `cheetah-codec::CodecId` 新增 `MP2`
- `FrameFormat` 新增 `Mp2Frame`
- `CodecExtradata` 新增或复用无配置路径；MP2 不要求额外 config
- ingress/egress、fMP4/HLS codec string、测试构造工具同步处理 `MP2`

**stream_type 映射**:

| CodecId | TS stream_type |
|---------|----------------|
| MP2 | `0x03` |
| MP3 | `0x04` |

输入兼容：`0x03` 优先识别为 MP2；历史样例若标成 MP3，可通过诊断或后续 probe 修正。

---

## 1.2 抽取 TS Muxer 到 `cheetah-codec`

**现状**: HLS core 中已有 `TsMuxer` 和 `TsMuxerMulti`，但属于 HLS 私有实现。

**目标 API**:

```rust
pub struct MpegTsMuxer;

impl MpegTsMuxer {
    pub fn new(config: MpegTsMuxerConfig, tracks: &[TrackInfo]) -> Result<Self, MpegTsError>;
    pub fn write_tables(&mut self) -> Vec<MpegTsMuxEvent>;
    pub fn push_frame(&mut self, frame: &AVFrame) -> Vec<MpegTsMuxEvent>;
    pub fn flush(&mut self) -> Vec<MpegTsMuxEvent>;
}
```

**实现要求**:

- PAT/PMT 可随时输出
- 多轨 PID 动态分配
- PCR 写入 adaptation field
- PTS/DTS 使用 90kHz，来源为 canonical `AVFrame.pts_us/dts_us`
- AAC raw 自动加 ADTS
- H264/H265/H266 自动导出 Annex-B
- H264/H265 写 AUD；H266 后续按 codec helper 扩展
- 关键帧前补参数集
- `FrameFlags::NON_PICTURE` 默认跳过，可配置保留
- 不支持的 codec 返回 diagnostic，不 panic

---

## 1.3 抽取 TS Demuxer 到 `cheetah-codec`

**现状**: HLS core 中已有 `TsDemuxer`，但事件只输出 PES-like frame，尚未完整进入 `AVFrame + TrackInfo`。

**目标 API**:

```rust
pub struct MpegTsDemuxer;

impl MpegTsDemuxer {
    pub fn new(config: MpegTsDemuxerConfig) -> Self;
    pub fn push(&mut self, bytes: &[u8]) -> Vec<MpegTsDemuxEvent>;
    pub fn flush(&mut self) -> Vec<MpegTsDemuxEvent>;
}
```

**实现要求**:

- 支持任意切片输入和 188 字节重同步
- 解析 PAT/PMT，发现 track 后输出 `TrackInfo`
- 重组 PES，输出 `AVFrame`
- AAC ADTS 输入剥离为 `AacRaw`，同步生成/更新 ASC
- H264/H265 Annex-B 输入转 canonical H26x frame
- PTS/DTS 33-bit unwrap，输出 canonical timeline
- continuity counter 缺口发 diagnostic，相关 frame 标记 `DISCONTINUITY`
- PMT version change 后更新 track 列表
- null packet、未知 PID、未知 stream_type 跳过
- PES length 0 用下一 PES start 或 flush 完成

---

## 1.4 stream_type 兼容矩阵

对齐标准与 SMS 工程实践：

| 编码 | 输出 stream_type | 输入兼容 | 说明 |
|------|------------------|----------|------|
| H264 | `0x1B` | `0x1B` | 标准 |
| H265 | `0x24` | `0x24` | HEVC |
| H266 | `0x33` | `0x33` | VVC 预留/兼容 |
| AAC | `0x0F` | `0x0F` | ADTS in TS |
| MP2 | `0x03` | `0x03` | MPEG-1 Audio Layer II |
| MP3 | `0x04` | `0x03/0x04` | 历史输入容忍 |
| G711A | `0x90` | `0x90` | 非标准，国标/行业实践 |
| G711U | `0x91` | `0x91` | 非标准 |
| Opus | `0x06 + Opus descriptor` | `0x06 + descriptor` / `0x9C` | 兼容 SMS |
| VP8 | `0x9D` | `0x9D` | 非标准 |
| VP9 | `0x9E` | `0x9E` / private descriptor | 非标准 |
| AV1 | `0x9F` | `0x9F` / private descriptor | 非标准 |

---

## 1.5 HLS 复用共享 TS API

**目标**: 避免 HLS 与 TS module 两套 TS 容器逻辑分叉。

**改动点**:

- `cheetah-hls-core` 停止导出私有 `TsMuxer/TsDemuxer`，或保留兼容 wrapper 指向 `cheetah-codec`
- `cheetah-hls-module::StreamMuxer` 使用 `cheetah-codec::MpegTsMuxer`
- HLS 现有 AUD、ADTS、参数集补发行为保持不退化
- HLS property/fuzz 测试迁移到共享 TS API 或补充 wrapper 兼容测试

---

## 1.6 测试与 Fuzz

单元测试：

- PAT/PMT CRC 正确
- PAT/PMT 解析出 PID 与 stream_type
- H264/H265/AAC/MP2/MP3/G711/Opus/VP8/VP9/AV1 stream_type 映射
- PTS/DTS encode/decode roundtrip
- PCR 写入和解析
- continuity counter wrap
- PES length 0 flush
- unaligned input resync

Property tests：

- 任意 payload 长度 mux 后都保持 188 字节对齐
- 任意 chunk 切分 demux 结果一致
- 多轨 PID 不冲突
- PAT/PMT 周期补发不破坏 demux

Fuzz targets：

- `mpeg_ts_demux_unaligned`
- `mpeg_ts_pat_pmt`
- `mpeg_ts_pes`
- `mpeg_ts_mux_roundtrip`

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo clippy -p cheetah-hls-core
cargo test -p cheetah-hls-core
cargo test -p cheetah-hls-property-tests
(cd crates/protocols/ts/fuzz && cargo +nightly fuzz build)
```
