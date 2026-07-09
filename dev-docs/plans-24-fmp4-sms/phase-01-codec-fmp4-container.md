# Phase 01 — 共享 fMP4 容器能力

- **状态**: 规划中
- **范围**: 在 `cheetah-codec` 中建立标准 ISO BMFF/fMP4 mux/demux，补齐 MJPEG、MP2、多编码、多轨、鲁棒性，并让 HLS 复用共享实现
- **完成标准**: `cheetah-codec` 可独立完成 fMP4 mux/demux roundtrip；HLS fMP4 路径不再维护私有容器分叉；core 测试、property test、fuzz build 通过

---

## 1.1 媒体模型补 MJPEG

**问题**: 用户要求支持 MJPEG，但当前 `CodecId` 无 MJPEG，无法表达 JPEG access unit。

**实现方案**:

- `cheetah-codec::CodecId` 新增 `MJPEG`
- `FrameFormat` 新增 `MjpegFrame`
- `CodecExtradata` 新增 `MJPEG { config: Option<Bytes> }` 或复用 `Raw(Bytes)`；首版默认无额外 config
- `compat::codec_from_name()` 接受 `mjpeg/jpeg/mjpg`
- `is_video_codec()`、测试矩阵、frame view、adapter 合约同步处理 `MJPEG`
- `Mp4SampleEntry::from_track()` 支持 MJPEG

**fMP4 映射**:

| CodecId | sample entry | object type |
|---------|--------------|-------------|
| MJPEG | `mp4v` | `0x6C` |

输入兼容 `jpeg/mjpa/mjpb`，输出默认 `mp4v + esds`，并用 diagnostic 标记该路径播放器支持有限。

---

## 1.2 抽取 fMP4 Muxer 到 `cheetah-codec`

**现状**: HLS core 中已有 `Fmp4Muxer`，但属于 HLS 私有实现，缺少 MJPEG/MP2、完整 box 鲁棒性和通用事件 API。

**目标 API**:

```rust
pub struct Fmp4Muxer;

impl Fmp4Muxer {
    pub fn new(config: Fmp4MuxerConfig, tracks: &[TrackInfo]) -> Result<Self, Fmp4Error>;
    pub fn init_segment(&mut self) -> Vec<Fmp4MuxEvent>;
    pub fn push_frame(&mut self, frame: &AVFrame) -> Vec<Fmp4MuxEvent>;
    pub fn flush(&mut self) -> Vec<Fmp4MuxEvent>;
}
```

**实现要求**:

- 写 `ftyp + moov` init segment
- `moov` 写 `mvhd/trak/mdia/minf/stbl/mvex/trex`
- 写 media segment：`styp` 可选、`sidx` 可选、`moof + mdat` 必选
- 每个有样本的 track 写一个 `traf`
- `tfhd` 设置 `default-base-is-moof`
- `tfdt` 使用首样本 dts 相对 track 起点的 decode time
- `trun` 对 video 使用 version 1，支持 signed composition time offset
- B-frame 样本保留 `pts - dts`
- H26x payload 自动导出 length-prefixed NALU
- 关键帧、时间窗口或显式 flush 可触发 fragment
- unsupported frame 返回 diagnostic，不 panic
- 所有缓存和单 fragment 样本数量有上界

---

## 1.3 抽取 fMP4 Demuxer 到 `cheetah-codec`

**现状**: HLS core 中已有 `Fmp4Demuxer`，但只支持基础 parse，错误模型、streaming input、重复 init、partial box、large size 不完整。

**目标 API**:

```rust
pub struct Fmp4Demuxer;

impl Fmp4Demuxer {
    pub fn new(config: Fmp4DemuxerConfig) -> Self;
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Fmp4DemuxEvent>;
    pub fn flush(&mut self) -> Vec<Fmp4DemuxEvent>;
}
```

**实现要求**:

- 支持任意切片输入和 box reassembly
- 支持 32-bit size、64-bit largesize、size 0 extends-to-end
- 跳过 unknown box，不影响后续 box
- 解析 `ftyp/moov`，发现 track 后输出 `TrackInfo`
- 解析 `styp/sidx/moof/mdat`，按 `traf/trun` 输出 frame
- 输入兼容 `moof+mdat`、`styp+moof+mdat`、`sidx+moof+mdat`
- H26x length-prefixed 输入转 canonical H26x frame
- AAC sample 输入转 `AacRaw`
- G711/Opus/MJPEG/MP2/MP3/VP8/VP9/AV1 原样进入对应 frame format
- 重复 init segment 更新 track 列表，并发出 discontinuity diagnostic
- `mdat` 越界、sample size 越界、track id 缺失发 diagnostic 并丢弃当前 fragment

---

## 1.4 sample entry 兼容矩阵

| 编码 | 输出 entry | 输入 entry | config box | 说明 |
|------|------------|------------|------------|------|
| H264 | `avc1` | `avc1/avc2/avc3/avc4` | `avcC` | 输出 Apple/SMS 兼容首选 |
| H265 | `hvc1` | `hvc1/hev1/dvh1/dvhe` | `hvcC` | 输入兼容 Dolby Vision HEVC tag |
| H266 | `vvc1` | `vvc1` | `vvcC` | 若已有 extradata helper 则输出，否则 diagnostic |
| AAC | `mp4a` | `mp4a` | `esds` | ObjectType `0x40` |
| G711A | `alaw` | `alaw` | none | SMS 兼容 |
| G711U | `ulaw` | `ulaw` | none | SMS 兼容 |
| Opus | `Opus` | `Opus` | `dOps` | Opus in ISOBMFF |
| MJPEG | `mp4v` | `mp4v/jpeg/mjpa/mjpb` | `esds` | ObjectType `0x6C` |
| MP2 | `mp4a` | `mp4a` | `esds` | ObjectType `0x6B` |
| MP3 | `mp4a` | `mp4a` | `esds` | ObjectType `0x69`，输入兼容 `0x6B` |
| VP8 | `vp08` | `vp08` | `vpcC` | 非所有播放器支持 |
| VP9 | `vp09` | `vp09` | `vpcC` | WebM/MP4 兼容 |
| AV1 | `av01` | `av01` | `av1C` | AV1 ISOBMFF |

---

## 1.5 HLS 复用共享 fMP4 API

**目标**: 避免 HLS 与 fMP4 module 两套 fMP4 容器逻辑分叉。

**改动点**:

- `cheetah-hls-core` 停止维护私有 `Fmp4Muxer/Fmp4Demuxer`，或保留兼容 wrapper 指向 `cheetah-codec`
- `cheetah-hls-module::StreamMuxer` 使用 `cheetah-codec::Fmp4Muxer`
- HLS 现有 init segment、part、segment、demuxed AV 行为保持不退化
- HLS LL-HLS part 可通过 `Fmp4MuxerConfig { include_styp: false, include_sidx: false }` 生成
- HLS property/fuzz 测试迁移到共享 fMP4 API 或补充 wrapper 兼容测试

---

## 1.6 测试与 Fuzz

单元测试：

- `ftyp/moov/mvex` init segment 正确
- `styp/sidx/moof/mdat` media segment 正确
- `tfhd default-base-is-moof`、`tfdt`、`trun.data_offset` 正确
- B-frame signed composition time offset roundtrip
- H264/H265/H266 length-prefixed sample roundtrip
- AAC/G711/Opus/MJPEG/MP2/MP3/VP8/VP9/AV1 sample entry roundtrip
- multi-track `traf` 与 `mdat` offset 正确
- large box / unknown box / partial box bounded

Property tests：

- 任意 chunk 切分 demux 结果一致
- 多轨 track id 稳定且不冲突
- mux 后 demux 保持 frame count、track kind、codec、keyframe flag
- fragment flush 策略不生成空 `mdat`
- repeated init 不造成 track 泄漏

Fuzz targets：

- `fuzz_fmp4_box_parser`
- `fuzz_fmp4_init_segment`
- `fuzz_fmp4_media_fragment`
- `fuzz_fmp4_mux_roundtrip`

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo clippy -p cheetah-hls-core
cargo test -p cheetah-hls-core
cargo test -p cheetah-hls-property-tests
(cd crates/protocols/fmp4/fuzz && cargo +nightly fuzz build)
```
