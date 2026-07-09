# Phase 01 — fMP4 Muxer 实现

- **状态**: 未开始
- **范围**: fMP4 init segment 生成、media segment (moof+mdat) 生成、CMAF 低延迟切片、与现有 HLS pipeline 集成
- **完成标准**: 配置 `container: "fmp4"` 后，推流生成 init.mp4 + *.m4s，hls.js 可播放

---

## 1.1 fMP4 Init Segment 生成 (ftyp + moov)

**ZLMediaKit 参考**: `MP4MuxerMemory::getInitSegment()` 在 `addTrackCompleted()` 时生成。`HlsFMP4Recorder` 调用 `_hls->inputInitSegment(data.data(), data.size())`。

**实现方案**:

在 `cheetah-hls-core` 新增 `fmp4_mux.rs`：

```rust
// cheetah-hls-core/src/fmp4_mux.rs

/// fMP4 track description.
pub struct Fmp4TrackDesc {
    pub track_id: u32,
    pub codec: CodecId,
    pub media_kind: MediaKind,
    pub timescale: u32,
    /// Codec-specific extradata (SPS/PPS for H264, AudioSpecificConfig for AAC, etc.)
    pub extradata: Bytes,
    /// Video dimensions (0 for audio).
    pub width: u16,
    pub height: u16,
    /// Audio sample rate / channel count.
    pub sample_rate: u32,
    pub channels: u8,
}

/// fMP4 muxer — generates ISO BMFF segments.
pub struct Fmp4Muxer {
    tracks: Vec<Fmp4TrackDesc>,
    sequence_number: u32,
    init_segment: Option<Bytes>,
}

impl Fmp4Muxer {
    pub fn new(tracks: Vec<Fmp4TrackDesc>) -> Self;

    /// Generate the init segment (ftyp + moov). Cached after first call.
    pub fn init_segment(&mut self) -> Bytes;

    /// Generate a media segment (styp + moof + mdat) from accumulated samples.
    pub fn write_segment(&mut self, samples: &[Fmp4Sample]) -> Bytes;
}
```

**Box 结构 — Init Segment**:
```
ftyp (isom, iso6, msdh, msix)
moov
├── mvhd (timescale=1000, duration=0)
├── trak[] (per track)
│   ├── tkhd
│   ├── mdia
│   │   ├── mdhd (timescale per track)
│   │   ├── hdlr (vide/soun)
│   │   └── minf
│   │       ├── vmhd/smhd
│   │       └── stbl (empty: stsd + stts + stsc + stsz + stco)
│   │           └── stsd → avc1/hev1/mp4a/Opus/vp09/av01
│   └── edts (optional)
└── mvex
    └── trex[] (per track, default_sample_duration=0)
```

**改动点**:
- `cheetah-hls-core`: 新增 `fmp4_mux.rs`
- `cheetah-hls-core/src/lib.rs`: 导出 `Fmp4Muxer`, `Fmp4TrackDesc`, `Fmp4Sample`

---

## 1.2 fMP4 Media Segment 生成 (moof + mdat)

**ZLMediaKit 参考**: `MP4MuxerMemory::onSegmentData()` 输出 moof+mdat buffer。

**实现方案**:

```rust
pub struct Fmp4Sample {
    pub track_id: u32,
    pub pts_ms: u64,
    pub dts_ms: u64,
    pub is_keyframe: bool,
    pub data: Bytes,
}

impl Fmp4Muxer {
    pub fn write_segment(&mut self, samples: &[Fmp4Sample]) -> Bytes {
        // 1. styp box (msdh, msix)
        // 2. moof box:
        //    - mfhd (sequence_number++)
        //    - traf[] per track:
        //      - tfhd (track_id, default_sample_flags)
        //      - tfdt (baseMediaDecodeTime)
        //      - trun (sample_count, data_offset, per-sample: duration, size, flags, cts_offset)
        // 3. mdat box (concatenated sample data)
    }
}
```

**Box 结构 — Media Segment**:
```
styp (msdh, msix)
moof
├── mfhd (sequence_number)
└── traf[] (per track with samples in this segment)
    ├── tfhd (track_id, default_sample_flags)
    ├── tfdt (baseMediaDecodeTime in track timescale)
    └── trun (sample_count, data_offset, [duration, size, flags, cts_offset]*)
mdat (raw sample data concatenated)
```

**关键细节**:
- `data_offset` 在 trun 中指向 mdat 内的偏移，需要回填
- `composition_time_offset` = PTS - DTS（有符号，version=1 时）
- `sample_flags`: keyframe = `0x02000000`, non-key = `0x00010000`

---

## 1.3 CMAF 低延迟切片

**ZLMediaKit 参考**: `HlsFMP4Recorder` 按 GOP 边界输出 segment，配合 LL-HLS 的 `#EXT-X-PART`。

**实现方案**:

扩展现有 `LowLatencyState`，支持 fMP4 part：

```rust
// 在 StreamMuxer 中，当 container=fmp4 时：
// - 每个 GOP 生成一个完整 media segment
// - 每个 part (sub-GOP chunk) 可独立生成 moof+mdat
// - init segment 在 track info 确定后立即生成并缓存

impl StreamMuxer {
    fn finalize_fmp4_segment(&mut self) -> Bytes {
        let samples = std::mem::take(&mut self.pending_samples);
        self.fmp4_muxer.as_mut().unwrap().write_segment(&samples)
    }
}
```

**与 LL-HLS 集成**:
- `#EXT-X-MAP:URI="init.mp4"` 指向 init segment
- 每个 `.m4s` 文件是一个 media segment
- Part 级别的 `#EXT-X-PART` 指向同一 segment 的子范围（或独立 part 文件）

---

## 1.4 与现有 Pipeline 集成

**改动点**:

1. `StreamMuxer` 新增 `fmp4_muxer: Option<Fmp4Muxer>` 字段
2. `StreamMuxer::init()` 根据 `container` 配置选择 TS 或 fMP4 路径
3. `StreamMuxer::push_frame()` 在 fMP4 模式下累积 samples 而非写 TS packets
4. `StreamMuxer::finalize_segment()` 在 fMP4 模式下调用 `write_segment()`
5. `SegmentRing` 存储 fMP4 segment（与 TS 相同接口，仅 content-type 不同）
6. `PlaylistBuilder` 在 fMP4 模式下输出 `#EXT-X-MAP` 和 `.m4s` 后缀
7. HTTP server 对 init.mp4 请求返回缓存的 init segment

**配置**:
```yaml
modules:
  hls:
    container: "fmp4"  # "ts" (default) or "fmp4"
```

---

## 1.5 编码支持矩阵

| 编码 | TS stream_type | fMP4 stsd box | 状态 |
|------|---------------|---------------|------|
| H264 | 0x1B | avc1 (avcC) | ✅ TS / 待实现 fMP4 |
| H265 | 0x24 | hev1 (hvcC) | ✅ TS / 待实现 fMP4 |
| VP8 | 私有 | vp08 | ✅ TS / 待实现 fMP4 |
| VP9 | 私有 | vp09 (vpcC) | ✅ TS / 待实现 fMP4 |
| AV1 | 私有 | av01 (av1C) | ✅ TS / 待实现 fMP4 |
| AAC | 0x0F | mp4a (esds) | ✅ TS / 待实现 fMP4 |
| MP3 | 0x03 | mp4a (esds) | ✅ TS / 待实现 fMP4 |
| OPUS | 私有 | Opus (dOps) | ✅ TS / 待实现 fMP4 |
| G711A | 0x90 | alaw | ✅ TS / 待实现 fMP4 |
| G711U | 0x91 | ulaw | ✅ TS / 待实现 fMP4 |
| MP2 | 0x03 | mp4a | ❌ 待 Phase 04 |

---

## 验证方法

1. 配置 `container: "fmp4"` → 推流 → 验证 init.mp4 + *.m4s 文件生成
2. `ffprobe init.mp4` → 验证 ftyp + moov 结构正确
3. `ffprobe seg_0.m4s` → 验证 moof + mdat 结构正确
4. hls.js demo 播放 fMP4 HLS → 验证端到端可播
5. 多编码测试: H264+AAC, H265+OPUS → 验证 stsd box 正确
6. LL-HLS: 验证 `#EXT-X-MAP` 出现在 playlist 中
