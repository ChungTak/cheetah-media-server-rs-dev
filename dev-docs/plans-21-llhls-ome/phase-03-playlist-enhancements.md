# Phase 03 — Playlist 增强

- **目标**: 实现帧对齐 Part Duration 计算、EXT-X-RENDITION-REPORT、ConcludeLive 结束标记、Wallclock 偏移
- **OME 参考**: `llhls_stream.cpp::ComputeOptimalPartDuration` / `llhls_chunklist.cpp::MakeChunklist (RENDITION-REPORT)` / `ConcludeLive` / `_wallclock_offset_ms`

---

## 1. ComputeOptimalPartDuration（帧对齐 Part 时长）

### 1.1 原理（OME 实现）

```cpp
// Video: part_target 对齐到帧时长的整数倍
auto frame_duration_ms = (1.0 / track->GetFrameRate()) * 1000.0;
auto optimal_frame_count = std::round(part_target / frame_duration_ms);
optimal_part_target = optimal_frame_count * frame_duration_ms;

// Audio: part_target 对齐到音频帧时长的整数倍
auto frame_duration_ms = (samples_per_frame / sample_rate) * 1000.0;
auto optimal_frame_count = std::round(part_target / frame_duration_ms);
optimal_part_target = optimal_frame_count * frame_duration_ms;
```

目的：确保每个 part 包含整数帧，避免 part 边界打断帧导致解码问题。

### 1.2 实现方案

**module 层 muxer.rs**:
- `set_tracks` 时计算 optimal part duration：
  ```rust
  fn compute_optimal_part_duration(&self, tracks: &[TrackInfo]) -> u64 {
      let target_ms = self.config.part_target_ms as f64;

      // 优先用视频帧率对齐
      if let Some(video) = tracks.iter().find(|t| t.media_kind == MediaKind::Video) {
          if let Some(fps) = video.frame_rate {
              if fps > 0.0 {
                  let frame_ms = 1000.0 / fps;
                  let frames = (target_ms / frame_ms).round();
                  return (frames * frame_ms).round() as u64;
              }
          }
      }

      // 回退到音频采样率对齐
      if let Some(audio) = tracks.iter().find(|t| t.media_kind == MediaKind::Audio) {
          let sample_rate = audio.sample_rate.unwrap_or(44100) as f64;
          let samples_per_frame = if audio.codec == CodecId::AAC { 1024.0 } else { 960.0 };
          let frame_ms = samples_per_frame / sample_rate * 1000.0;
          let frames = (target_ms / frame_ms).round();
          return (frames * frame_ms).round() as u64;
      }

      self.config.part_target_ms
  }
  ```
- 将计算结果注入 `LowLatencyState` 的 `part_target_secs`

**core 层 ll_hls.rs**:
- `LowLatencyState` 新增 `set_part_target_ms(ms: u64)` 方法

### 1.3 TrackInfo 扩展

- `cheetah-codec` 的 `TrackInfo` 需确保包含 `frame_rate: Option<f64>` 字段
- RTMP 源通常通过 metadata 提供 frame_rate；RTSP 通过 SDP

---

## 2. EXT-X-RENDITION-REPORT

### 2.1 规范定义

```
#EXT-X-RENDITION-REPORT:URI="chunklist_2_audio.m3u8",LAST-MSN=5,LAST-PART=3
```

在每个 chunklist 末尾，报告其他 rendition 的最新 msn/part 状态。用于 ABR 切换时播放器知道其他 rendition 的进度。

### 2.2 OME 实现

```cpp
for (const auto &[track_id, rendition] : _renditions) {
    if (track_id == _track->GetId()) continue;  // skip self
    int64_t last_msn, last_part;
    rendition->GetLastSequenceNumber(last_msn, last_part);
    playlist.AppendFormat("#EXT-X-RENDITION-REPORT:URI=\"%s\",LAST-MSN=%" PRIu64 ",LAST-PART=%" PRIu64 "\n",
                          rendition->GetUrl(), last_msn, last_part);
}
```

### 2.3 实现方案

**前置条件**: Phase 04 的 Per-Track Chunklist。本阶段先实现单 track 场景下的结构准备。

**core 层 ll_hls.rs**:
- 新增 `RenditionInfo` 结构：
  ```rust
  pub struct RenditionInfo {
      pub uri: String,
      pub last_msn: u64,
      pub last_part: u64,
  }
  ```
- `LowLatencyState` 新增 `renditions: Vec<RenditionInfo>` 和 `set_renditions` 方法
- `build_media_ll` 在非 legacy 模式下追加 `#EXT-X-RENDITION-REPORT` 标签

**module 层**:
- 多 track 场景：每个 track 的 StreamMuxer 知道其他 track 的进度
- 单 track 场景：不输出 RENDITION-REPORT（自身不报告自己）

---

## 3. ConcludeLive（直播结束标记）

### 3.1 原理（OME 实现）

```cpp
std::tuple<bool, ov::String> LLHlsStream::ConcludeLive() {
    _concluded = true;
    // 向所有 chunklist 追加 EXT-X-ENDLIST
    for (auto &[track_id, chunklist] : _chunklist_map) {
        chunklist->SetEndList();
    }
    // 不再接受新 segment/chunk
}
```

### 3.2 实现方案

**module 层 muxer.rs**:
- 新增 `conclude(&mut self)` 方法：
  - 调用 `flush()` 写入最后一个 segment
  - 设置 `concluded = true` 标志
  - 重建 playlist 缓存（带 `#EXT-X-ENDLIST`）
  - 后续 `push_frame` 直接返回空

**core 层 playlist.rs**:
- `build_media_ll` 新增 `concluded: bool` 参数
- 当 `concluded = true` 时在 playlist 末尾追加 `#EXT-X-ENDLIST\n`

**module 层 module.rs**:
- 流结束事件（unpublish / stream_ended）触发 `muxer.conclude()`
- 已结束的流仍可被播放器请求 playlist（返回带 ENDLIST 的版本）

---

## 4. Wallclock Offset（挂钟偏移）

### 4.1 原理（OME 实现）

```cpp
if (_first_chunk == true) {
    _first_chunk = false;
    auto first_chunk_timestamp_ms = (DTS / timescale) * 1000.0;
    _wallclock_offset_ms = publish_time_epoch_ms - first_chunk_timestamp_ms;
    // 所有 chunklist 共享此 offset
}

// PROGRAM-DATE-TIME 计算：
start_timestamp_ms = (partial.start_timestamp / timescale) * 1000.0;
start_timestamp_ms += _wallclock_offset_ms;
```

### 4.2 实现方案

**module 层 muxer.rs**:
- 新增字段：
  ```rust
  wallclock_offset_ms: Option<i64>,
  publish_time_ms: Option<i64>,  // 流发布时的挂钟时间（module 注入）
  ```
- `set_publish_time(epoch_ms: i64)` — module 在流创建时注入
- 第一次 `finalize_part` 时计算：
  ```rust
  if self.wallclock_offset_ms.is_none() {
      let first_dts_ms = first_sample_dts_ms as i64;
      let publish_ms = self.publish_time_ms.unwrap_or_else(|| now_epoch_ms());
      self.wallclock_offset_ms = Some(publish_ms - first_dts_ms);
  }
  ```
- `program_date_time_ms` 计算：`segment_start_dts_ms + wallclock_offset_ms`

**已有实现对接**:
- `Segment::program_date_time_ms` 字段已存在
- `SegmentRing::push` 已接受 `program_date_time_ms`
- 需要确保 wallclock offset 正确传播到每个 segment 和 part

---

## 5. 涉及文件变更

| 层 | 文件 | 变更 |
|----|------|------|
| core | `ll_hls.rs` | 新增 set_part_target_ms / RenditionInfo / rendition_report 生成 |
| core | `playlist.rs` | build_media_ll 新增 legacy / concluded 参数 |
| module | `muxer.rs` | compute_optimal_part_duration / conclude / wallclock_offset |
| module | `module.rs` | 流结束触发 conclude / publish_time 注入 |

---

## 6. 测试计划

| 测试 | 层 | 方法 |
|------|-----|------|
| optimal part duration 计算 | module | 单元测试：30fps → 200ms 对齐到 200ms(6帧)，25fps → 200ms 对齐到 200ms(5帧) |
| rendition report 格式 | core | 单元测试：构造 RenditionInfo 验证标签输出 |
| conclude 后 playlist 有 ENDLIST | module | 单元测试：conclude() 后 playlist 包含 EXT-X-ENDLIST |
| conclude 后不再产出 segment | module | 单元测试：conclude() 后 push_frame 返回空 |
| wallclock offset 正确性 | module | 单元测试：验证 program_date_time_ms 基于 publish_time + DTS |

---

## 7. 完成标准

- [x] 30fps 视频 part_target_ms=200 → 实际 part 时长对齐到 200ms（6帧×33.33ms）
- [x] 流结束后 playlist 包含 `#EXT-X-ENDLIST`
- [x] EXT-X-PROGRAM-DATE-TIME 使用 wallclock offset 正确计算绝对时间
- [x] RENDITION-REPORT 结构就绪（单 track 场景不输出，多 track 场景正确输出）
