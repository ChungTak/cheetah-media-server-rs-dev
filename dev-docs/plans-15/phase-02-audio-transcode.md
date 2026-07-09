# Phase 02 — 音频转码与选择性禁用

- **状态**: 未开始
- **范围**: G711→AAC 转码实现、AAC→G711 转码、音视频选择性禁用
- **完成标准**: G.711 RTSP 源可通过 RTMP 播放 AAC 音频；可按需禁用音频或视频转发

---

## 2.1 G711→AAC 实时转码

**问题**: G.711（PCMA/PCMU）是 RTSP/SIP/国标设备常用音频编码，但 RTMP/FLV 播放器通常只支持 AAC。

**ABLMediaServer 方案**: 
- `ConvertG711ToAAC()`：G711 → `alaw_to_pcm16`/`ulaw_to_pcm16` → 累积 PCM → FAAC 编码 → AAC+ADTS
- 参数：64kbps, 8000Hz, 1 channel
- 由 `nG711ConvertAAC` 配置控制，可全局或按流设置

**本地现状**: 
- `G711ToAacTranscoder` 管线已实现（cheetah-codec transcode.rs）
- `AacEncoder` trait 已定义
- 缺少实际的 AAC 编码器实现

**实现方案**:

引入 `fdk-aac` crate 作为可选 feature：

```toml
# crates/foundation/cheetah-codec/Cargo.toml
[features]
fdk-aac = ["dep:fdk-aac"]

[dependencies]
fdk-aac = { version = "0.6", optional = true }
```

```rust
// cheetah-codec/src/transcode/fdk_aac_encoder.rs
#[cfg(feature = "fdk-aac")]
pub struct FdkAacEncoder { /* ... */ }

#[cfg(feature = "fdk-aac")]
impl AacEncoder for FdkAacEncoder {
    fn encode(&mut self, pcm: &[i16]) -> Option<Bytes> { /* fdk-aac encode */ }
    fn frame_size(&self) -> usize { 1024 }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn channels(&self) -> u8 { self.channels }
}
```

**集成位置**: RTMP module egress — 检测源 codec 为 G711 时自动插入转码器

**配置**:
```yaml
modules:
  rtmp:
    g711_to_aac: true
    aac_encode_bitrate: 64000
```

---

## 2.2 AAC→G711 实时转码

**问题**: 部分 RTSP 设备（对讲、SIP 网关）只接受 G.711 音频，需要将 AAC 源转为 G.711。

**ABLMediaServer 方案**: FFmpeg AAC 解码 → PCM → `pcm16_to_alaw()` 查表转换。

**本地现状**: 无 AAC 解码能力。

**实现方案**:

```rust
// cheetah-codec/src/transcode.rs — 新增 trait
pub trait AacDecoder: Send {
    /// Decode AAC frame to interleaved PCM i16 samples.
    fn decode(&mut self, aac_frame: &[u8]) -> Vec<i16>;
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u8;
}

/// PCM → G.711 编码（查表）
pub fn pcm16_to_g711a(pcm: &[i16]) -> Vec<u8>;
pub fn pcm16_to_g711u(pcm: &[i16]) -> Vec<u8>;

/// AAC → G.711 转码管线
pub struct AacToG711Transcoder {
    decoder: Box<dyn AacDecoder>,
    target_codec: CodecId, // G711A or G711U
    output_track_id: TrackId,
}
```

**依赖**: AAC 解码可通过 `symphonia` (纯 Rust) 或 `fdk-aac` 解码模式实现。

**配置**:
```yaml
modules:
  rtsp:
    aac_to_g711: false  # 默认禁用
    aac_to_g711_codec: g711a  # g711a 或 g711u
```

---

## 2.3 音视频选择性禁用

**问题**: 某些场景只需要视频（监控回放）或只需要音频（对讲），需要按需禁用。

**ABLMediaServer 方案**: `disableVideo`/`disableAudio` 参数，在 `openRtpServer`/`addStreamProxy` 时指定。

**本地现状**: 无选择性禁用能力。

**实现方案**:

在订阅选项中增加媒体过滤：

```rust
// cheetah-sdk — SubscriberOptions 扩展
pub struct SubscriberOptions {
    pub queue_capacity: usize,
    pub backpressure: BackpressurePolicy,
    pub bootstrap_policy: BootstrapPolicy,
    pub media_filter: MediaFilter,  // 新增
}

pub struct MediaFilter {
    pub enable_video: bool,  // 默认 true
    pub enable_audio: bool,  // 默认 true
}
```

在 Dispatcher 分发时检查 filter：

```rust
// 分发帧时跳过被禁用的媒体类型
if !subscriber.media_filter.enable_video && frame.media_kind == MediaKind::Video {
    continue;
}
```

**配置**: 通过 RTSP SETUP 参数或 RTMP play URL query 参数传递：
- RTSP: `rtsp://host/live/test?disableAudio=1`
- RTMP: `rtmp://host/live/test?disableAudio=1`
