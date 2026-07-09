# Phase 04 — 高级特性

- **状态**: 未开始
- **范围**: 多轨道、时间戳鲁棒性、快速注册、延迟 playlist、segment 保留
- **完成标准**: 多音频轨 HLS 可播放，时间戳异常不导致崩溃

---

## 4.1 多轨道 TS Muxer

**ZLMediaKit 参考**: `MpegMuxer` 维护 `unordered_map<int, MP4Track>`，支持多 video/audio track。

**实现方案**:

扩展 `TsMuxer` 支持动态 PID 分配：

```rust
pub struct TsMuxer {
    tracks: Vec<TsTrack>,
    // ...
}

struct TsTrack {
    pid: u16,
    stream_type: u8,
    cc: u8,
    media_kind: MediaKind,
    codec: CodecId,
}

impl TsMuxer {
    pub fn new_multi(tracks: &[TrackDesc]) -> Self;
    pub fn write_frame(&mut self, track_index: usize, data: &[u8], pts: u64, dts: u64, keyframe: bool);
}

pub struct TrackDesc {
    pub codec: CodecId,
    pub media_kind: MediaKind,
}
```

**PID 分配策略**:
- Video tracks: 0x0100, 0x0101, ...
- Audio tracks: 0x0110, 0x0111, ...
- PCR PID: 第一个 video track

**PMT 生成**: 遍历所有 tracks 写入 stream entry。

---

## 4.2 时间戳回退/回绕处理

**ZLMediaKit 参考**: 检测 `stamp < _last_seg_timestamp`，重置并 warn。

**实现方案**:

```rust
// muxer.rs — push_frame 中
let frame_dts = frame.dts as u64;
if let Some(start_dts) = self.segment_start_dts {
    if frame_dts < start_dts {
        // Timestamp rollback detected — reset segment timing
        warn!("HLS timestamp rollback: {} < {}", frame_dts, start_dts);
        self.segment_start_dts = Some(frame_dts);
        self.segment_last_dts = frame_dts;
        return false;
    }
}
```

**33-bit 回绕** (90kHz clock):
- 在 `ms_to_90k()` 后对 `0x1_FFFF_FFFF` 取模
- 检测 PTS/DTS 跳变 > 半周期 → 视为回绕

---

## 4.3 快速注册模式

**ZLMediaKit 参考**: `kFastRegister=true` 时，首 1-2 个 segment 不等待 duration 阈值，立即在 keyframe 处切割。

**实现方案**:

```rust
// muxer.rs — should_cut 逻辑中
let should_cut = if let Some(start_dts) = self.segment_start_dts {
    let elapsed_ms = frame_dts - start_dts;
    let normal_cut = is_video && is_keyframe && elapsed_ms >= self.config.segment_duration_ms;
    let force_cut = elapsed_ms >= self.config.force_segment_after_ms;
    // Fast register: first 2 segments cut on any keyframe
    let fast_cut = self.config.fast_register && self.segment_seq < 2 && is_video && is_keyframe;
    normal_cut || force_cut || fast_cut
} else {
    false
};
```

**配置**: `fast_register: bool`（默认 false）

---

## 4.4 延迟 Playlist

**ZLMediaKit 参考**: 生成第二个 `*_delay.m3u8`，包含 `seg_number + seg_delay` 个 segment。

**实现方案**:

```rust
// PlaylistBuilder
pub fn build_media_delayed(
    ring: &SegmentRing,
    extra_segments: usize,  // 额外保留的 segment 数
    session_id: Option<u64>,
) -> String;
```

需要 `SegmentRing` 支持更大的内部容量（`segment_count + delay_count`），但 playlist 只暴露 `segment_count` 个给普通 m3u8，`segment_count + delay_count` 个给 delay m3u8。

**配置**: `segment_delay: usize`（默认 0，禁用）

---

## 4.5 Segment 保留 + 删除延迟

**ZLMediaKit 参考**:
- `kSegmentRetain`: 磁盘上保留超出 m3u8 的 N 个 segment
- `kDeleteDelaySec`: 流结束后延迟 N 秒再删除文件

**实现方案**:

```rust
// file_writer.rs
pub struct SegmentRetentionPolicy {
    /// Extra segments to keep on disk beyond what's in the m3u8.
    pub retain_count: usize,
    /// Delay before deleting files after stream ends (seconds).
    pub delete_delay_secs: u64,
}
```

**删除逻辑**:
- 正常运行: 当 segment 从 m3u8 移除后，再保留 `retain_count` 个才删除
- 流结束: 启动定时器，`delete_delay_secs` 后批量删除

**配置**:
```yaml
modules:
  hls:
    segment_retain: 2
    delete_delay_secs: 10
```

---

## 验证方法

1. 多轨道: 推送含 2 音频轨的流 → ffprobe 验证 .ts 含多 PID
2. 时间戳回退: 注入回退帧 → 验证不 panic，日志有 warn
3. 快速注册: 推流后 < 2s 内 m3u8 可用
4. 延迟 playlist: 验证 `_delay.m3u8` 比 `index.m3u8` 多 N 个 segment
5. 保留策略: 停止推流 → 验证文件在 delay 后才被删除
