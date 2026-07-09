# Phase 03 — 跨协议音频兼容性

- **状态**: 未开始
- **范围**: 跨协议静音音频注入验证、G.711↔AAC 实时转码、Opus↔AAC 实时转码
- **完成标准**: 视频流跨协议播放时自动补静音音频；G.711 RTSP 源可通过 RTMP 播放 AAC 音频

---

## 3.1 跨协议静音音频注入（RTSP egress 验证）

**问题**: 视频-only 的 RTMP 推流通过 RTSP 拉流时，部分播放器（VLC、ffplay）可能因缺少音频 track 而异常。需要在 RTSP egress 自动注入静音音频。

**ZLMediaKit 方案**: `MuteAudioMaker` 在 `MediaSink` 层检测到仅有视频 track 时，自动注入静音 AAC 帧。注入的音频 track 参与 SDP 生成和 RTP 发送。

**本地现状**:
- `MuteAudioMaker`（`cheetah-codec`）已实现，可生成静音 AAC 帧
- RTMP module 的 `enable_add_mute` 配置已支持在 RTMP play 时注入
- 但 RTSP module 的 play 路径未集成 `MuteAudioMaker`

**实现方案**:

```rust
// RTSP module play.rs — 静音音频注入
fn setup_play_session(tracks: &[TrackInfo], config: &RtspModuleConfig) -> PlaySession {
    let has_audio = tracks.iter().any(|t| t.media_kind == MediaKind::Audio);
    let has_video = tracks.iter().any(|t| t.media_kind == MediaKind::Video);
    
    let mute_audio = if !has_audio && has_video && config.enable_mute_audio {
        // 创建静音 AAC track 信息
        let mute_track = TrackInfo::mute_aac(48000, 2); // 48kHz stereo
        Some(MuteAudioState {
            maker: MuteAudioMaker::new(48000, 2, 1024), // 1024 samples/frame
            track_info: mute_track,
            rtp_state: PlayTrackState::new_for_mute_audio(),
        })
    } else {
        None
    };
    
    PlaySession { mute_audio, .. }
}

// SDP 生成时包含静音 audio track
fn build_describe_sdp_with_mute(tracks: &[TrackInfo], mute: Option<&TrackInfo>) -> Sdp {
    let mut all_tracks = tracks.to_vec();
    if let Some(mute_track) = mute {
        all_tracks.push(mute_track.clone());
    }
    build_describe_sdp(&all_tracks)
}

// Play 循环中注入静音帧
fn inject_mute_audio_if_needed(
    video_frame: &AVFrame,
    mute_state: &mut MuteAudioState,
) -> Vec<AVFrame> {
    // 根据视频帧时间戳生成对应时间段的静音 AAC 帧
    mute_state.maker.generate_frames_up_to(video_frame.dts_us)
}
```

**配置**:
```yaml
modules:
  rtsp:
    enable_mute_audio: true  # 默认启用
    mute_audio_sample_rate: 48000
    mute_audio_channels: 2
```

**实现位置**: `cheetah-rtsp-module` play.rs，sdp.rs

**验证**:
- 集成测试：RTMP 推流纯视频 → RTSP DESCRIBE 返回含 audio track 的 SDP → RTSP PLAY 收到静音 AAC RTP 包
- 播放器验证：VLC/ffplay 通过 RTSP 播放纯视频源不报错

---

## 3.2 G.711A/U↔AAC 跨协议实时转码

**问题**: G.711（PCMA/PCMU）是 RTSP/SIP 设备常用的音频编码，但 RTMP/FLV 播放器通常只支持 AAC。跨协议播放时需要实时转码。

**ZLMediaKit 方案**: `Factory` 插件机制 + 独立转码 muxer。当目标协议不支持源 codec 时，自动插入转码环节。

**本地现状**:
- G.711 在 RTMP 和 RTSP 中均已支持（编解码、RTP packetize/depacketize）
- 但 RTMP 播放器（如 ffplay rtmp）对 G.711 的支持取决于 FLV 容器兼容性
- 缺少 G.711→AAC 转码能力

**实现方案**:

采用独立转码模块，不在协议热路径中：

```rust
// crates/foundation/codec/src/transcode/g711_aac.rs

/// G.711 → AAC 转码器（使用 fdk-aac 或纯 Rust AAC 编码器）
pub struct G711ToAacTranscoder {
    decoder: G711Decoder,       // G.711 → PCM (trivial: μ-law/A-law lookup table)
    encoder: AacEncoder,        // PCM → AAC (LC profile)
    input_sample_rate: u32,     // 8000 (G.711 标准)
    output_sample_rate: u32,    // 44100 或 48000
    resampler: Option<Resampler>, // 8kHz → 44.1/48kHz
    frame_size: usize,          // AAC frame size (1024 samples)
    pcm_buffer: Vec<i16>,       // 累积 PCM 样本
}

impl G711ToAacTranscoder {
    pub fn new(config: G711ToAacConfig) -> Self;
    
    /// 输入 G.711 AVFrame，输出 0 或多个 AAC AVFrame
    pub fn transcode(&mut self, input: &AVFrame) -> Vec<AVFrame> {
        // 1. G.711 → PCM (lookup table, 无损)
        let pcm = self.decoder.decode(&input.payload);
        // 2. 重采样 8kHz → 目标采样率
        let resampled = self.resampler.as_mut()
            .map(|r| r.process(&pcm))
            .unwrap_or(pcm);
        // 3. 累积到 frame_size
        self.pcm_buffer.extend_from_slice(&resampled);
        // 4. 每满 1024 samples 编码一帧 AAC
        let mut output = Vec::new();
        while self.pcm_buffer.len() >= self.frame_size {
            let frame_pcm: Vec<i16> = self.pcm_buffer.drain(..self.frame_size).collect();
            let aac_data = self.encoder.encode(&frame_pcm);
            output.push(build_aac_avframe(aac_data, input, self.output_sample_rate));
        }
        output
    }
}
```

**集成方式**:

在 module 层的 egress 管线中按需插入转码：

```rust
// RTMP module play.rs — 检测是否需要转码
fn needs_audio_transcode(source_codec: CodecId, target_protocol: Protocol) -> bool {
    match (source_codec, target_protocol) {
        (CodecId::G711A | CodecId::G711U, Protocol::Rtmp) => true, // G.711 → AAC for RTMP
        _ => false,
    }
}

// 在 play 循环中应用转码
fn process_frame_for_play(frame: Arc<AVFrame>, transcoder: &mut Option<G711ToAacTranscoder>) -> Vec<Arc<AVFrame>> {
    if let Some(tc) = transcoder {
        tc.transcode(&frame).into_iter().map(Arc::new).collect()
    } else {
        vec![frame]
    }
}
```

**依赖选择**:
- 优先使用纯 Rust AAC 编码器（如 `aac-enc` crate）避免 C 依赖
- 备选：`fdk-aac` 通过 feature flag 启用（更高质量但需要 C 编译）
- G.711 解码为纯查表实现，无外部依赖

**配置**:
```yaml
modules:
  rtmp:
    audio_transcode:
      g711_to_aac: true          # 默认启用
      aac_sample_rate: 44100     # 输出采样率
      aac_channels: 1            # 输出声道数（G.711 通常单声道）
      aac_bitrate: 64000         # AAC 编码码率
```

**实现位置**: `cheetah-codec` transcode/g711_aac.rs，`cheetah-rtmp-module` egress.rs

**验证**:
- 单元测试：G.711 PCM 样本 → AAC 帧，验证输出可被 ffmpeg 解码
- 集成测试：RTSP 推流 G.711A → RTMP 拉流，ffplay 正常播放 AAC 音频

---

## 3.3 Opus↔AAC 跨协议实时转码

**问题**: Opus 是 WebRTC/现代 RTSP 设备常用的音频编码，但 RTMP/FLV 传统播放器不支持 Opus（Enhanced RTMP 支持但普及度低）。需要 Opus→AAC 转码。

**ZLMediaKit 方案**: 同 G.711，通过 Factory 插件机制在目标协议不支持时自动转码。

**本地现状**:
- Opus 在 RTMP（Enhanced）和 RTSP 中均已支持
- 但传统 RTMP 播放器（不支持 Enhanced RTMP）无法播放 Opus
- 缺少 Opus→AAC 转码能力

**实现方案**:

```rust
// crates/foundation/codec/src/transcode/opus_aac.rs

/// Opus → AAC 转码器
pub struct OpusToAacTranscoder {
    decoder: OpusDecoder,       // Opus → PCM (使用 opus crate)
    encoder: AacEncoder,        // PCM → AAC
    input_sample_rate: u32,     // 48000 (Opus 标准)
    output_sample_rate: u32,    // 44100 或 48000
    resampler: Option<Resampler>, // 48kHz → 44.1kHz (如果需要)
    channels: u8,
    frame_size: usize,          // AAC frame size (1024 samples)
    pcm_buffer: Vec<i16>,
}

impl OpusToAacTranscoder {
    pub fn transcode(&mut self, input: &AVFrame) -> Vec<AVFrame> {
        // 1. Opus → PCM
        let pcm = self.decoder.decode(&input.payload, self.channels);
        // 2. 可选重采样
        let resampled = self.resample_if_needed(pcm);
        // 3. 累积 + 编码
        self.pcm_buffer.extend_from_slice(&resampled);
        let mut output = Vec::new();
        while self.pcm_buffer.len() >= self.frame_size * self.channels as usize {
            let frame_pcm: Vec<i16> = self.pcm_buffer
                .drain(..self.frame_size * self.channels as usize)
                .collect();
            let aac_data = self.encoder.encode(&frame_pcm);
            output.push(build_aac_avframe(aac_data, input, self.output_sample_rate));
        }
        output
    }
}
```

**依赖**:
- Opus 解码：`opus` crate（libopus 绑定）或 `audiopus`
- AAC 编码：同 3.2 共用

**配置**:
```yaml
modules:
  rtmp:
    audio_transcode:
      opus_to_aac: true          # 默认启用（仅对不支持 Enhanced RTMP 的客户端）
      prefer_enhanced_rtmp: true # 优先使用 Enhanced RTMP 透传 Opus
```

**决策逻辑**:
```rust
fn select_audio_egress_strategy(source_codec: CodecId, client_caps: &ClientCapabilities) -> AudioEgressStrategy {
    match source_codec {
        CodecId::Opus if client_caps.supports_enhanced_rtmp => AudioEgressStrategy::Passthrough,
        CodecId::Opus => AudioEgressStrategy::Transcode(TranscodeTarget::Aac),
        CodecId::G711A | CodecId::G711U => AudioEgressStrategy::Transcode(TranscodeTarget::Aac),
        _ => AudioEgressStrategy::Passthrough,
    }
}
```

**实现位置**: `cheetah-codec` transcode/opus_aac.rs，`cheetah-rtmp-module` egress.rs

**验证**:
- 单元测试：Opus 帧 → AAC 帧，验证输出可被 ffmpeg 解码
- 集成测试：RTSP 推流 Opus → RTMP 拉流（传统客户端），ffplay 正常播放 AAC
- 集成测试：RTSP 推流 Opus → RTMP 拉流（Enhanced 客户端），直接透传 Opus
