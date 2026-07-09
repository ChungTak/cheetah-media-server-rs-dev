# RTMP Phase 01 — 编解码补全

- **状态**: 未开始
- **范围**: G.711 A-law/μ-law、VP8、VP9 编解码支持；国内扩展 codec ID 兼容层；未知编码透传
- **完成标准**: 所有新增编解码可正常 ingest/egress，国内扩展 ID 可正确识别并转换，未知编码可透传转发

---

## 目标

补齐 RTMP/FLV 生态中常见但本地尚未支持的编解码，使服务器能够：

1. 接收和转发 G.711 A-law/μ-law 音频流
2. 接收和转发 VP8/VP9 视频流
3. 兼容国内厂商使用的非标准 codec ID（H.265=12, AV1=13, VP8=14, VP9=15, Opus=13）
4. 对未知编码进行透传转发（不转协议）

---

## 设计约束

- 新增编解码参数解析统一放在 `cheetah-codec`
- RTMP 层的 codec ID 映射放在 `cheetah-rtmp-core` 的 `media.rs`
- 国内扩展作为入口兼容层，内部统一使用 `CodecId` 枚举
- 透传编码不经过 codec 处理管线，直接以原始字节转发
- 所有新增必须补单元测试和属性测试

---

## 任务分解

### 1.1 G.711 A-law/μ-law 支持

**目标**: 支持 G.711 音频的 ingest 和 egress。

**实现**:

1. `cheetah-codec` 扩展 `CodecId`：

```rust
pub enum AudioCodecId {
    // ... 已有
    G711Alaw,   // RTMP audio codec ID = 7
    G711Ulaw,   // RTMP audio codec ID = 8
}
```

2. `cheetah-codec` 新增 `audio/g711.rs`：

```rust
/// G.711 不需要配置头，固定参数：
/// - 采样率: 8000 Hz
/// - 位深: 16 bit (解码后)
/// - 通道: 单声道
pub struct G711Params {
    pub law: G711Law,
    pub sample_rate: u32,   // 固定 8000
    pub channels: u8,       // 固定 1
}

pub enum G711Law {
    ALaw,
    ULaw,
}
```

3. `cheetah-rtmp-core` media 解析扩展：

```rust
// FLV audio flags: codec_id = 7 (A-law) 或 8 (μ-law)
// G.711 没有 config header，所有包都是 raw audio data
fn parse_audio_tag(flags: u8, data: &[u8]) -> AudioFrame {
    match codec_id {
        7 => AudioFrame::raw(AudioCodecId::G711Alaw, data),
        8 => AudioFrame::raw(AudioCodecId::G711Ulaw, data),
        // ...
    }
}
```

4. `cheetah-rtmp-module` ingest/egress 管线扩展：
   - Ingest: 识别 G.711 帧，生成 `TrackInfo` 并发布
   - Egress: 将 G.711 `AVFrame` 封装为 FLV audio tag

**测试**:
- 单元测试：G.711 FLV tag 解析/生成往返
- 属性测试：任意 G.711 payload 解析不 panic
- 集成测试：G.711 推流 → 拉流验证

---

### 1.2 VP8 编解码支持

**目标**: 支持 VP8 视频的 ingest 和 egress。

**实现**:

1. `cheetah-codec` 扩展 `CodecId`：

```rust
pub enum VideoCodecId {
    // ... 已有
    VP8,
}
```

2. `cheetah-codec` 新增 `video/vp8.rs`：

```rust
/// VP8 关键帧检测（通过 frame header 第一个字节的 bit 0）
pub fn is_keyframe(data: &[u8]) -> bool {
    !data.is_empty() && (data[0] & 0x01) == 0
}

/// VP8 序列头（从关键帧中提取宽高）
pub struct Vp8Params {
    pub width: u16,
    pub height: u16,
}

pub fn parse_keyframe_header(data: &[u8]) -> Option<Vp8Params> {
    // VP8 keyframe: 3-byte frame tag + 7-byte key header
    // ...
}
```

3. `cheetah-rtmp-core` Enhanced RTMP 支持：
   - FourCC `vp08` 已在 Enhanced RTMP 解析中，确认路径畅通
   - 国内扩展 ID 14 映射到 `VideoCodecId::VP8`

4. FLV egress：VP8 使用 Enhanced RTMP FourCC 封装

**测试**:
- 单元测试：VP8 关键帧检测、参数解析
- 属性测试：任意字节序列解析不 panic
- 集成测试：VP8 推流 → 拉流

---

### 1.3 VP9 编解码支持

**目标**: 支持 VP9 视频的 ingest 和 egress。

**实现**:

1. `cheetah-codec` 扩展：

```rust
pub enum VideoCodecId {
    // ... 已有
    VP9,
}
```

2. `cheetah-codec` 新增 `video/vp9.rs`：

```rust
/// VP9 帧类型检测（通过 uncompressed header）
pub fn is_keyframe(data: &[u8]) -> bool {
    // VP9 frame marker (2 bits) + profile (1-2 bits) + ...
    // frame_type bit: 0 = key frame
    !data.is_empty() && parse_frame_type(data) == FrameType::Key
}

/// VP9 序列参数（从关键帧 uncompressed header 提取）
pub struct Vp9Params {
    pub profile: u8,
    pub width: u16,
    pub height: u16,
    pub bit_depth: u8,
}
```

3. `cheetah-rtmp-core`：FourCC `vp09` + 国内扩展 ID 15

**测试**:
- 单元测试：VP9 帧类型检测、参数解析
- 属性测试：任意字节序列解析不 panic

---

### 1.4 国内扩展 Codec ID 兼容层

**目标**: 兼容国内厂商使用的非标准 codec ID，实现双向转换。

**实现**:

1. `cheetah-rtmp-core` 新增 `compat.rs`：

```rust
/// 国内扩展视频 codec ID → 内部 VideoCodecId
pub fn domestic_video_id_to_codec(id: u8) -> Option<VideoCodecId> {
    match id {
        12 => Some(VideoCodecId::H265),
        13 => Some(VideoCodecId::AV1),
        14 => Some(VideoCodecId::VP8),
        15 => Some(VideoCodecId::VP9),
        _ => None,
    }
}

/// 国内扩展音频 codec ID → 内部 AudioCodecId
pub fn domestic_audio_id_to_codec(id: u8) -> Option<AudioCodecId> {
    match id {
        13 => Some(AudioCodecId::Opus),
        _ => None,
    }
}

/// 内部 VideoCodecId → 国内扩展 ID（用于出口兼容模式）
pub fn codec_to_domestic_video_id(codec: VideoCodecId) -> Option<u8> {
    match codec {
        VideoCodecId::H265 => Some(12),
        VideoCodecId::AV1 => Some(13),
        VideoCodecId::VP8 => Some(14),
        VideoCodecId::VP9 => Some(15),
        _ => None,
    }
}
```

2. `cheetah-rtmp-core` media 解析扩展：

```rust
fn parse_video_tag(data: &[u8]) -> VideoFrame {
    let codec_id = data[0] & 0x0F;
    let frame_type = (data[0] >> 4) & 0x0F;

    // Enhanced RTMP 检测 (bit 7 of first byte)
    if is_enhanced_header(data[0]) {
        return parse_enhanced_video(data);
    }

    // 标准 codec ID
    match codec_id {
        7 => parse_avc(data),
        // 国内扩展
        12 => parse_domestic_h265(data),
        13 => parse_domestic_av1(data),
        14 => parse_domestic_vp8(data),
        15 => parse_domestic_vp9(data),
        _ => parse_unknown_video(codec_id, data),
    }
}
```

3. 配置项控制出口模式：

```yaml
modules:
  rtmp:
    codec_mode: enhanced  # enhanced | domestic | auto
```

**测试**:
- 单元测试：所有国内扩展 ID 的双向映射
- 集成测试：使用国内扩展 ID 推流 → 服务器正确识别
- 集成测试：配置 domestic 模式 → 出口使用国内扩展 ID

---

### 1.5 未知编码透传

**目标**: 对不支持的编码进行原始字节透传，仅限同协议转发。

**实现**:

1. `cheetah-codec` 扩展：

```rust
pub enum VideoCodecId {
    // ... 已有
    Unknown(u8),            // 未知经典 ID
    UnknownFourCC([u8; 4]), // 未知 Enhanced RTMP FourCC
}

pub enum AudioCodecId {
    // ... 已有
    Unknown(u8),
    UnknownFourCC([u8; 4]),
}
```

2. `cheetah-rtmp-core`：未知 codec 生成 `RawFrame`（不解析 payload）

3. `cheetah-rtmp-module`：
   - Unknown 帧标记为 `passthrough`
   - 仅允许 RTMP→RTMP 转发
   - 跨协议请求时返回 `UnsupportedCodec` 错误
   - 录制时跳过 unknown track

**测试**:
- 单元测试：未知 codec ID 不 panic，正确生成 Unknown 变体
- 集成测试：未知编码推流 → RTMP 拉流可正常播放
- 集成测试：未知编码推流 → HTTP-FLV 拉流返回错误或降级
