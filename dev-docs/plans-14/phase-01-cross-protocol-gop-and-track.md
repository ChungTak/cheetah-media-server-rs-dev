# Phase 01 — 跨协议 GOP 秒开与 Track 同步

- **状态**: 未开始
- **范围**: 跨协议 GOP 秒开、Track 就绪同步等待、非标准编码器端到端验证、Track 动态变更
- **完成标准**: RTMP 推流后 RTSP 拉流首帧延迟 < 1 GOP；RTSP 推流后 RTMP 拉流秒开正常；H.265/AV1 跨协议播放通过

---

## 1.1 跨协议 GOP 秒开：RTMP 推流 → RTSP 拉流

**问题**: RTMP 推流后，RTSP 订阅者加入时需要等待下一个关键帧才能开始播放，首帧延迟可能达到数秒。

**ZLMediaKit 方案**: `RtspMediaSource` 的 `RingBuffer` 以 GOP 为单位缓存 RTP 包组，新订阅者从最近 keyframe 对应的 RTP 包组开始读取。`PacketCache` 在 `key_pos=true` 时标记 GOP 边界。

**本地现状**: 
- Engine 的 `RingBuffer` 已有 IDR 位置追踪和 `bootstrap_frames()` 机制
- RTSP play 订阅时使用 `BootstrapPolicy::live_tail()` 获取 bootstrap 帧
- 但 bootstrap 帧是 `AVFrame` 格式，RTSP egress 需要重新 packetize 为 RTP

**需要补齐的能力**:

1. **验证 bootstrap 帧的 RTSP packetize 正确性**：确保从 RTMP 源获取的 bootstrap AVFrame 能正确转换为 RTP 包
2. **参数集补发**：RTSP 订阅者加入时，确保首个 keyframe 前携带 SPS/PPS/VPS（通过 `ParameterSetCache.prepend_to_access_unit()`）
3. **SDP 生成时机**：DESCRIBE 响应时从 `TrackInfo.extradata` 生成 SDP，确保 RTMP 源的 AVCC/HVCC 能正确转换为 SDP fmtp

**实现方案**:

```rust
// RTSP module play.rs — 订阅 RTMP 源的流
fn handle_describe_for_cross_protocol(tracks: &[TrackInfo]) -> Sdp {
    // TrackInfo.extradata 已包含 SPS/PPS（从 RTMP 序列头解析）
    // export_media_description() 已能从 CodecExtradata 生成 SDP
    // 需要验证：AVCC 格式的 extradata → SDP sprop-parameter-sets 转换正确
    build_describe_sdp(tracks)
}

// RTSP module play.rs — bootstrap 帧发送
fn send_bootstrap_frames(frames: Vec<Arc<AVFrame>>, track_state: &mut PlayTrackState) {
    for frame in frames {
        // 确保 keyframe 携带参数集
        let frame = parameter_set_cache.repair_if_needed(&frame);
        // packetize 为 RTP（已有逻辑）
        packetize_frame_to_rtp_with_timestamp(frame, track_state);
    }
}
```

**实现位置**: `cheetah-rtsp-module` play.rs，`cheetah-codec` adapter.rs

**验证**: 
- 集成测试：RTMP 推流 H.264+AAC → RTSP DESCRIBE 返回正确 SDP → RTSP PLAY 秒开
- 测量首帧延迟 < 1 GOP 间隔

---

## 1.2 跨协议 GOP 秒开：RTSP 推流 → RTMP 拉流

**问题**: RTSP 推流后，RTMP 订阅者加入时需要收到完整的序列头（AVCC/HVCC + AudioSpecificConfig）才能正确解码。

**ZLMediaKit 方案**: `RtmpMediaSourceMuxer` 在 `addTrack` 时通过 `makeConfigPacket()` 生成序列头 RtmpPacket，新订阅者首先收到 metadata + 序列头 + GOP 缓存。

**本地现状**:
- RTMP play 的 `send_track_bootstrap()` 已能发送 metadata + 序列头 + bootstrap 帧
- 序列头从 `TrackInfo.extradata` 构建（`build_video_sequence_header`、`build_audio_sequence_header`）
- 但需要验证 RTSP 源的 in-band SPS/PPS 能正确填充 `TrackInfo.extradata`

**需要补齐的能力**:

1. **RTSP 源 extradata 完整性**：确保 RTSP ANNOUNCE 的 SDP 或 in-band 参数集能完整填充 `CodecExtradata`
2. **AVCC/HVCC 构建**：从 Annex-B 格式的 SPS/PPS 构建 AVCC box 用于 RTMP 序列头
3. **Track ready 时机**：RTSP 源可能在 SDP 中声明 track 但参数集在 RTP 流中才到达

**实现方案**:

```rust
// cheetah-codec — 从 Annex-B 参数集构建 AVCC/HVCC
// 已有：CodecExtradata::H264 { sps, pps, avcc }
// 需要确保：当 RTSP 源通过 in-band 发现 SPS/PPS 时，同步构建 avcc 字段

// RTSP module publish.rs — 参数集发现后更新 TrackInfo
fn on_parameter_set_discovered(session: &mut PublishSession, track_id: TrackId, ps: &ParameterSets) {
    let extradata = build_codec_extradata_from_parameter_sets(ps);
    let mut track_info = session.tracks[track_id].clone();
    track_info.extradata = extradata;
    session.sink.update_tracks(vec![track_info]);
}
```

**实现位置**: `cheetah-rtsp-module` publish.rs，`cheetah-codec` video.rs

**验证**:
- 集成测试：RTSP ANNOUNCE+RECORD H.264+AAC → RTMP PLAY 收到正确序列头 → 秒开播放
- 验证 in-band SPS/PPS 发现后 RTMP 订阅者能正确收到更新的序列头

---

## 1.3 Track 就绪同步等待机制

**问题**: 当 RTMP 推流时 metadata 声明了音视频两个 track，但音频数据延迟到达，此时 RTSP 订阅者可能收到不完整的 SDP（缺少音频描述）。

**ZLMediaKit 方案**: `MediaSink` 等待所有 track `ready()` 后才调用 `onAllTrackReady()`，之后才开始向 muxer 分发帧。超时机制：`kWaitTrackReadyMS` 后放弃等待未就绪的 track。

**本地现状**:
- `TrackInfo` 有 `TrackReadiness` 枚举（NotReady → PendingConfig → Ready）
- 但 Engine 的 `push_frame()` 不等待所有 track ready，逐帧分发
- RTSP DESCRIBE 可能在 track 未完全就绪时被调用

**实现方案**:

```rust
// cheetah-codec — Track 就绪聚合器
pub struct TrackReadinessAggregator {
    expected_tracks: usize,
    ready_tracks: HashSet<TrackId>,
    timeout_ms: u64,
    start_time: Option<Instant>,
}

impl TrackReadinessAggregator {
    pub fn on_track_ready(&mut self, track_id: TrackId) -> AggregateState;
    pub fn is_all_ready(&self) -> bool;
    pub fn is_timed_out(&self, now: Instant) -> bool;
}

pub enum AggregateState {
    Waiting,          // 还有 track 未就绪
    AllReady,         // 所有 track 就绪
    TimedOut,         // 超时，使用已就绪的 track
}
```

**应用位置**:
- RTSP module：DESCRIBE 时若 track 未全部 ready，等待或返回已 ready 的 track
- RTMP module：play bootstrap 时若 track 未全部 ready，等待序列头到齐

**配置**:
```yaml
modules:
  rtsp:
    track_ready_timeout_ms: 5000  # 等待所有 track 就绪的超时
  rtmp:
    track_ready_timeout_ms: 5000
```

**实现位置**: `cheetah-codec` track.rs，`cheetah-rtsp-module` play.rs，`cheetah-rtmp-module` module.rs

---

## 1.4 H.265/AV1/VP9 跨协议端到端验证与修复

**问题**: H.265（Enhanced RTMP）、AV1、VP9 各自在 RTMP 和 RTSP 中已实现，但跨协议路径未经端到端验证，可能存在参数集格式转换、RTP packetize 细节问题。

**ZLMediaKit 方案**: `Factory` 统一注册所有 codec 的 encoder/decoder，确保任意 codec 在任意协议间可转换。

**本地现状**:
- RTMP：支持 Enhanced RTMP 的 H.265/H.266/AV1/VP8/VP9
- RTSP：支持 H.265/AV1/VP8/VP9 的 RTP packetize/depacketize
- 但跨协议路径（如 Enhanced RTMP H.265 → RTP HEVC）未有集成测试覆盖

**需要验证和修复的路径**:

| 源协议 | 目标协议 | Codec | 关键转换点 |
|--------|----------|-------|-----------|
| RTMP (Enhanced) | RTSP | H.265 | HVCC → VPS/SPS/PPS → SDP sprop-vps/sps/pps → RTP FU |
| RTMP (Enhanced) | RTSP | AV1 | AV1CodecConfig → SDP → RTP OBU aggregation |
| RTMP (Enhanced) | RTSP | VP9 | VP9 config → SDP → RTP VP9 payload |
| RTSP | RTMP (Enhanced) | H.265 | RTP FU → Annex-B → HVCC → Enhanced RTMP HEVC tag |
| RTSP | RTMP (Enhanced) | AV1 | RTP OBU → AV1 frame → AV1CodecConfig → Enhanced RTMP AV1 tag |
| RTSP | RTMP (Enhanced) | VP9 | RTP VP9 → VP9 frame → Enhanced RTMP VP9 tag |

**实现方案**:

为每条路径编写集成测试，发现问题后修复：

```rust
#[tokio::test]
async fn test_rtmp_h265_to_rtsp_play() {
    // 1. RTMP 推流 Enhanced RTMP H.265
    // 2. RTSP DESCRIBE → 验证 SDP 包含正确的 H.265 参数
    // 3. RTSP PLAY → 验证 RTP 包格式正确
    // 4. 验证首帧可解码
}
```

**实现位置**: `cheetah-rtsp-module` tests/，`cheetah-rtmp-module` tests/

---

## 1.5 跨协议 Track 动态变更通知

**问题**: 推流过程中 codec 参数变更（如分辨率切换、SPS/PPS 更新），订阅端需要感知并更新。

**ZLMediaKit 方案**: Track 变更时重新触发 `onAllTrackReady()`，muxer 重新生成 config packet / SDP。

**本地现状**:
- RTMP module 有 track 变更检测（序列头变化时调用 `update_tracks()`）
- 但 RTSP 订阅端收到 track 变更后的行为未定义

**实现方案**:

```rust
// Engine — Track 变更事件
pub enum StreamEvent {
    TracksUpdated { stream_key: StreamKey, tracks: Vec<TrackInfo> },
    // ...
}

// RTSP module play.rs — 收到 track 变更
fn on_tracks_updated(play_session: &mut PlaySession, new_tracks: &[TrackInfo]) {
    // 对于 TCP interleaved：发送新的参数集 RTP 包（in-band）
    // 对于 UDP：下次 keyframe 时自动携带新参数集（通过 ParameterSetCache）
    play_session.parameter_set_cache.invalidate();
}

// RTMP module play.rs — 收到 track 变更（RTSP 源更新了 SPS/PPS）
fn on_tracks_updated(play_session: &mut PlaySession, new_tracks: &[TrackInfo]) {
    // 重新发送序列头
    send_sequence_headers(play_session, new_tracks);
}
```

**实现位置**: `cheetah-engine` stream.rs，`cheetah-rtsp-module` play.rs，`cheetah-rtmp-module` module.rs

**配置**: 无额外配置，默认行为。
