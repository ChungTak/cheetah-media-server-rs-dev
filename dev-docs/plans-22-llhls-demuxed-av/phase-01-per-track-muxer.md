# Phase 01 — Per-Track Muxer

- **目标**: 将 LLHLS fMP4 从 video-only workaround 改为 per-track audio/video 打包，每个 lane 生成独立 init segment、parts 和 segments
- **OME 参考**: `llhls_stream.cpp::AddPackager()`、`fmp4_packager/fmp4_packager.cpp`、`fmp4_storage.cpp`

---

## 1. 当前实现问题

当前 `StreamMuxer` 在 LLHLS fMP4 模式下使用单一 `Fmp4Muxer` 和单一 `LowLatencyState`。为了避免 Chrome MSE 的 muxed `audiovideo` SourceBuffer 问题，代码临时跳过了 audio track/audio frame：

- `init_fmp4_muxer()` 在 `ll_state.is_some()` 时不把 audio track 写入 init segment。
- `push_frame_fmp4()` 在 `ll_state.is_some()` 且 frame 不是 video 时直接返回。

这使 LLHLS 能出画面，但输出不是完整音视频。Phase 01 的核心任务是移除这个 workaround，并用 per-track 架构恢复 audio。

---

## 2. 目标结构

新增 focused module，避免继续扩大 `muxer.rs`：

| 文件 | 职责 |
|------|------|
| `crates/protocols/hls/module/src/track_muxer.rs` | 单 track fMP4 packager：init、part、segment、ring、LL state |
| `crates/protocols/hls/module/src/demuxed_muxer.rs` | video/audio lane 编排、rendition state、兼容旧入口 |
| `crates/protocols/hls/module/src/muxer.rs` | 保留 `StreamMuxer` 外部入口，按配置选择 TS、legacy fMP4、demuxed LLHLS |

### Track Lane

`TrackLane` 是稳定 URL/API 语义，不等于真实 `TrackId`：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackLane {
    Video,
    Audio,
}
```

v1 只暴露默认 video/audio lane。内部仍保存真实 `TrackId`，为后续多音轨/ABR 扩展留空间。

### TrackMuxer

```rust
struct TrackMuxer {
    lane: TrackLane,
    source_track_id: TrackId,
    media_kind: MediaKind,
    codec: CodecId,
    fmp4_muxer: Fmp4Muxer,
    init_segment: Bytes,
    ll_state: LowLatencyState,
    pending_samples: Vec<Fmp4Sample>,
    pending_segment_parts: Vec<Bytes>,
    segment_start_dts_ms: Option<u64>,
    segment_last_dts_ms: u64,
    ring: SegmentRing,
    concluded: bool,
}
```

要求：

- 每个 `TrackMuxer` 的 `Fmp4Muxer::new()` 只接收一个 `Fmp4TrackDesc`。
- 对外 track id 使用稳定 fMP4 track id：video = 1，audio = 2；内部 `source_track_id` 只用于 frame 路由。
- `init_segment` 必须只包含当前 lane 的一个 `trak`。
- `write_part()` / `write_segment()` 输入只包含当前 lane samples，因此每个 `moof` 只包含一个 `traf`。

### DemuxedStreamMuxer

```rust
struct DemuxedStreamMuxer {
    video: Option<TrackMuxer>,
    audio: Option<TrackMuxer>,
    stream_key: String,
    wallclock_offset_ms: Option<i64>,
    concluded: bool,
}
```

`StreamMuxer` 在 `container=fmp4 && ll_hls_enabled && ll_hls_packaging_mode=demuxed-av` 时委托给 `DemuxedStreamMuxer`。

---

## 3. 切片与时间模型

不要实现成“video keyframe 强制切 audio”。OME 的模型是 per-track packager 独立产生 chunk/segment，chunklist 用 shared wallclock 和 rendition report 同步。

本项目 v1 采用以下规则：

- Video lane：等待首个 video keyframe 后开始，part 按 video frame DTS 累积到 `part_target_ms`；segment 优先在 keyframe 且达到 `segment_duration_ms` 后完成，长时间无 keyframe 时沿用现有 `force_segment_after_ms` 行为。
- Audio lane：不等待 video keyframe；part 按 AAC frame DTS 累积到音频对齐后的 part target；segment 按 `segment_duration_ms` 完成。
- 两个 lane 共享同一个 `wallclock_offset_ms`，第一个产生的 part/segment 初始化 offset，所有 `PROGRAM-DATE-TIME` 使用同一 offset。
- `EXT-X-RENDITION-REPORT` 用于让播放器同步另一 lane 的最新 MSN/PART，不要求两个 lane 的 part 数完全相同。
- Conclude 时必须 flush 所有 lane，并让所有 lane chunklist 输出 `EXT-X-ENDLIST`。

Part target 需要按 track 类型独立对齐：

- Video：`part_target_ms` 对齐到 video frame duration 的整数倍。
- Audio：AAC 使用 `1024 / sample_rate`，Opus 使用 `960 / sample_rate`，对齐到音频 frame duration 的整数倍。

---

## 4. 配置与兼容入口

新增配置：

```yaml
hls:
  container: "fmp4"
  ll_hls_enabled: true
  ll_hls_packaging_mode: "demuxed-av" # demuxed-av | video-only | muxed
```

语义：

- `demuxed-av`：默认；有 video+audio 时输出完整 demuxed A/V。
- `video-only`：保留当前 workaround，便于线上回退。
- `muxed`：仅用于非浏览器兼容验证；不作为 hls.js 默认路径。
- TS 容器和非 LLHLS fMP4 不受该配置影响。

旧方法保留兼容，但在 demuxed 模式下默认映射到 video lane：

```rust
impl StreamMuxer {
    pub fn init_segment(&self) -> Option<Bytes>;      // demuxed: video init
    pub fn get_part(&self, seq: u64) -> Option<Bytes>; // demuxed: video part
    pub fn get_segment(&self, name: &str) -> Option<Bytes>; // demuxed: video segment
    pub fn playlist(&self, session_id: Option<u64>) -> String; // demuxed: video chunklist
}
```

新增 lane-aware 方法：

```rust
impl StreamMuxer {
    pub fn is_demuxed(&self) -> bool;
    pub fn track_init_segment(&self, lane: TrackLane) -> Option<Bytes>;
    pub fn track_part(&self, lane: TrackLane, seq: u64) -> Option<Bytes>;
    pub fn track_segment(&self, lane: TrackLane, name: &str) -> Option<Bytes>;
    pub fn track_playlist(&self, lane: TrackLane, session_id: Option<u64>, legacy: bool, include_stream_key: bool) -> Option<String>;
    pub fn rendition_state(&self, lane: TrackLane) -> Option<(u64, u64)>;
}
```

---

## 5. 资源命名

v1 使用稳定、可读 URL，内部仍能映射到真实 track：

| Lane | Init | Part | Segment | Chunklist |
|------|------|------|---------|-----------|
| Video | `init_video.mp4` | `video_part_N.m4s` | `video_seg_N.m4s` | `chunklist_video.m3u8` |
| Audio | `init_audio.mp4` | `audio_part_N.m4s` | `audio_seg_N.m4s` | `chunklist_audio.m3u8` |

`N` 是该 lane 的全局 part/segment 序号。LL-HLS blocking playlist 的 `_HLS_part` 仍按规范表示目标 MSN 内的 part index；module 需要在 lane state 内转换和判断，不要把 query `_HLS_part` 当成全局 part 序号。

Stream key validation 继续通过 query string `?k=<stream_key>` 注入所有 media URL，包括 init、segment、part、preload hint。

---

## 6. 移除 workaround

完成本 phase 后必须移除：

- `init_fmp4_muxer()` 中 LLHLS 跳过 audio track 的分支。
- `push_frame_fmp4()` 中 LLHLS 跳过 audio frame 的分支。

移除后，audio frame 必须进入 audio `TrackMuxer`，不能进入 video `TrackMuxer` 或 muxed `Fmp4Muxer`。

---

## 7. 测试计划

| 测试 | 验证点 |
|------|--------|
| `demuxed_video_init_has_one_video_track` | `init_video.mp4` 只有一个 video `trak` |
| `demuxed_audio_init_has_one_audio_track` | `init_audio.mp4` 只有一个 audio `trak` |
| `demuxed_video_part_has_single_traf` | `video_part_N.m4s` 只有一个 video `traf` |
| `demuxed_audio_part_has_single_traf` | `audio_part_N.m4s` 只有一个 audio `traf` |
| `demuxed_audio_frames_are_not_dropped` | LLHLS demuxed 模式下 audio frame 会产生 audio part/segment |
| `track_part_target_is_aligned_per_track` | video/audio part target 独立按帧时长对齐 |
| `conclude_flushes_all_lanes` | 结束时 video/audio lane 都 flush 并输出 ENDLIST |
| `video_only_mode_keeps_current_workaround` | 回退配置仍只输出 video |
| `legacy_muxed_mode_still_works` | 非 LLHLS fMP4 保持旧 muxed 行为 |
