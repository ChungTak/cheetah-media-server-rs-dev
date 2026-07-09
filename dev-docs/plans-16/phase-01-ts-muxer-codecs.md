# Phase 01 — TS Muxer 多编码支持

- **状态**: 未开始
- **范围**: 扩展 TS stream type 映射、AUD 注入、ADTS 封装、参数集补发、SEI 过滤
- **完成标准**: ffplay 可播放包含 H264/H265/AAC/G711/OPUS/MP3 的 HLS 流

---

## 1.1 扩展 TS stream type 映射

**问题**: 当前 `ts_mux.rs` 仅支持 H264(0x1B)、H265(0x24)、AAC(0x0F)。

**simple-media-server 参考** (`Mpeg.h`):

| 编码 | stream_type | 说明 |
|------|-------------|------|
| H264 | 0x1B | 标准 |
| H265 | 0x24 | 标准 |
| VP8 | 0x9D | 非标准，业界约定 |
| VP9 | 0x9E | 非标准，业界约定 |
| AV1 | 0x9F | 非标准，业界约定 |
| AAC | 0x0F | 标准 |
| MP3 | 0x04 | MPEG-2 Audio Layer III |
| G711A | 0x90 | 非标准，国标/业界约定 |
| G711U | 0x91 | 非标准，业界约定 |
| OPUS | 0x9C | 非标准，业界约定 |
| MP2 | 0x04 | MPEG-1 Audio Layer II（与 MP3 共用） |

**实现方案**:

```rust
// crates/protocols/hls/core/src/ts_mux.rs
fn stream_type_for_codec(codec: CodecId) -> u8 {
    match codec {
        CodecId::H264 => 0x1B,
        CodecId::H265 => 0x24,
        CodecId::VP8 => 0x9D,
        CodecId::VP9 => 0x9E,
        CodecId::AV1 => 0x9F,
        CodecId::AAC => 0x0F,
        CodecId::MP3 => 0x04,
        CodecId::G711A => 0x90,
        CodecId::G711U => 0x91,
        CodecId::Opus => 0x9C,
        _ => 0x06, // private data fallback
    }
}
```

**改动点**:
- `TsMuxer::new()` 接受 `CodecId` 用于视频，新增 `audio_codec: CodecId` 参数
- `write_pmt()` 使用 `stream_type_for_codec()` 替代硬编码常量
- 支持多音频轨（预留 PID 分配）

---

## 1.2 H264/H265 Access Unit Delimiter 注入

**问题**: 部分播放器要求每个 PES 以 AUD 开头才能正确解码。

**simple-media-server 参考** (`TsMuxer.cpp`):
- H264: 在 keyframe PES 前注入 `00 00 00 01 09 F0`
- H265: 在 keyframe PES 前注入 `00 00 00 01 46 01 50`

**实现方案**:

```rust
// crates/protocols/hls/core/src/ts_mux.rs
fn prepend_aud(codec: CodecId, data: &[u8]) -> Vec<u8> {
    let aud = match codec {
        CodecId::H264 => &[0x00, 0x00, 0x00, 0x01, 0x09, 0xF0][..],
        CodecId::H265 => &[0x00, 0x00, 0x00, 0x01, 0x46, 0x01, 0x50][..],
        _ => return data.to_vec(),
    };
    let mut out = Vec::with_capacity(aud.len() + data.len());
    out.extend_from_slice(aud);
    out.extend_from_slice(data);
    out
}
```

**改动点**: `write_video()` 在构建 PES 前调用 `prepend_aud()`

---

## 1.3 AAC ADTS 头封装

**问题**: 引擎内部 AAC 帧是裸 AU（无 ADTS 头），但 TS 容器中 AAC 需要 ADTS 头。

**实现方案**: 使用 `cheetah-codec` 已有的 `adts_wrap()` 函数：

```rust
// crates/protocols/hls/module/src/muxer.rs — push_frame 中
if frame.media_kind == MediaKind::Audio && self.audio_codec == CodecId::AAC {
    let adts_frame = cheetah_codec::adts_wrap(&frame.payload, &self.aac_config);
    muxer.write_audio(&adts_frame, pts_90k);
} else {
    muxer.write_audio(&frame.payload, pts_90k);
}
```

**改动点**:
- `StreamMuxer` 新增 `aac_config: Option<AacAudioSpecificConfig>` 字段
- `set_tracks()` 从 TrackInfo extradata 解析 ASC
- `push_frame()` 对 AAC 帧自动加 ADTS 头

---

## 1.4 参数集每段首帧前显式补发

**问题**: 每个 TS segment 必须独立可解码，需要在每段第一个 keyframe 前补发 SPS/PPS/VPS。

**simple-media-server 参考**: `findVpsSpsPps` + keyframe 前 prepend。

**实现方案**:

```rust
// crates/protocols/hls/module/src/muxer.rs
// 在 segment 开始时，如果是 keyframe，prepend 参数集
if is_video && is_keyframe && self.segment_start_dts.is_none() {
    if let Some(ps) = &self.parameter_sets {
        let mut combined = ps.clone();
        combined.extend_from_slice(&frame.payload);
        muxer.write_video(&combined, pts_90k, dts_90k, true);
        return;
    }
}
```

**改动点**:
- `StreamMuxer` 新增 `parameter_sets: Option<Bytes>` 字段
- `set_tracks()` / 首帧检测时从 extradata 或 CONFIG 帧提取参数集
- 每段首个 keyframe 前 prepend

---

## 1.5 SEI/Metadata NAL 过滤

**问题**: SEI NAL 单元不影响解码但增大 segment 体积，部分播放器处理异常。

**simple-media-server 参考**: `metaFrame()` 和 NAL type 6 显式跳过。

**实现方案**:

```rust
// crates/protocols/hls/module/src/muxer.rs — push_frame 中
if is_video && frame.flags.contains(FrameFlags::NON_PICTURE) {
    return false; // skip SEI/metadata
}
```

**改动点**: `push_frame()` 入口处检查 `NON_PICTURE` flag，直接跳过。

---

## 验证方法

1. 单元测试：每种编码的 PMT stream_type 正确性
2. 集成测试：ffmpeg 推 H265+AAC → ffplay 拉 HLS 播放
3. 手动验证：VLC / hls.js 播放 G711/OPUS 流（需转码或直通验证）
