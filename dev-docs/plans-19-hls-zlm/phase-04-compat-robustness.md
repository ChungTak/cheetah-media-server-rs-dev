# Phase 04 — 非标兼容与鲁棒性

- **状态**: 未开始
- **范围**: TS 容错解析、PES 非标处理、时间戳平滑、MP2 编码支持、厂商 quirks 兼容
- **完成标准**: 可正确解析各种非标 TS 流，时间戳异常不导致播放卡顿或崩溃

---

## 4.1 TS Sync Byte 搜索与容错

**ZLMediaKit 参考**: 实际工程中 TS 流可能存在前导垃圾数据或丢包导致 sync byte (0x47) 偏移。

**实现方案**:

```rust
// cheetah-hls-core/src/ts_demux.rs — 增加 sync 搜索

impl TsDemuxer {
    /// Feed raw data that may not be aligned to 188-byte boundaries.
    pub fn feed_unaligned(&mut self, data: &[u8]) -> Vec<TsDemuxEvent> {
        let mut events = Vec::new();
        let mut offset = self.find_sync(data);

        while offset + TS_PACKET_SIZE <= data.len() {
            if data[offset] != SYNC_BYTE {
                // Lost sync — search forward
                offset = self.find_sync(&data[offset..]).map(|o| offset + o)
                    .unwrap_or(data.len());
                self.stats.sync_losses += 1;
                continue;
            }
            events.extend(self.parse_packet(&data[offset..offset + TS_PACKET_SIZE]));
            offset += TS_PACKET_SIZE;
        }
        // Save remainder for next call
        self.remainder = data[offset..].to_vec();
        events
    }

    fn find_sync(&self, data: &[u8]) -> Option<usize> {
        // Find 0x47 that is followed by another 0x47 at +188 (double-check)
        for i in 0..data.len().saturating_sub(TS_PACKET_SIZE) {
            if data[i] == SYNC_BYTE {
                if i + TS_PACKET_SIZE >= data.len() || data[i + TS_PACKET_SIZE] == SYNC_BYTE {
                    return Some(i);
                }
            }
        }
        None
    }
}

const SYNC_BYTE: u8 = 0x47;
const TS_PACKET_SIZE: usize = 188;
```

**统计**:
- `sync_losses`: sync 丢失次数
- `corrupted_packets`: CRC 错误或格式异常的包数

---

## 4.2 PES 非标处理

**问题**: 部分编码器输出的视频 PES 包 `PES_packet_length=0`（表示长度未知，合法但需特殊处理）。

**ZLMediaKit 参考**: 视频 PES 允许 unbounded length，以下一个 PES start 或 segment 结束为边界。

**实现方案**:

```rust
// cheetah-hls-core/src/ts_demux.rs — PES 重组

struct PesBuffer {
    data: Vec<u8>,
    pts: Option<u64>,
    dts: Option<u64>,
    /// PES declared length (0 = unbounded, use next PES start as delimiter).
    declared_length: u16,
    started: bool,
}

impl PesBuffer {
    fn feed_payload(&mut self, payload: &[u8], pusi: bool) -> Option<PesPacket> {
        if pusi && self.started {
            // New PES start → flush previous (even if declared_length=0)
            let completed = self.flush();
            self.start_new(payload);
            return completed;
        }
        if pusi {
            self.start_new(payload);
            return None;
        }
        // Continuation
        self.data.extend_from_slice(payload);
        // If declared_length > 0 and we have enough data, flush
        if self.declared_length > 0 && self.data.len() >= self.declared_length as usize + 6 {
            return self.flush();
        }
        None
    }
}
```

**非标场景**:
- `PES_packet_length=0`: 视频流常见，以 PUSI 为分界
- PES header 缺少 PTS/DTS: 使用上一帧的时间戳 + duration 推算
- PES stuffing bytes 过多: 跳过 0xFF 填充

---

## 4.3 时间戳平滑器

**ZLMediaKit 参考**: `Common/Stamp.h` 实现时间戳平滑，处理跳变、回退、非单调递增。

**实现方案**:

```rust
// cheetah-hls-core/src/stamp_smoother.rs (新增)

pub struct StampSmoother {
    /// Last output timestamp (ms).
    last_output: i64,
    /// Last input timestamp (ms).
    last_input: i64,
    /// Accumulated offset for correction.
    offset: i64,
    /// Maximum allowed forward jump before triggering reset (ms).
    max_forward_jump_ms: i64,
    /// Maximum allowed backward jump before triggering reset (ms).
    max_backward_jump_ms: i64,
    /// Whether first frame has been received.
    started: bool,
}

impl StampSmoother {
    pub fn new(max_forward_jump_ms: i64, max_backward_jump_ms: i64) -> Self;

    /// Smooth an input timestamp, returning the corrected output timestamp.
    pub fn smooth(&mut self, input_ms: i64) -> i64 {
        if !self.started {
            self.started = true;
            self.last_input = input_ms;
            self.last_output = 0;
            self.offset = -input_ms;
            return 0;
        }

        let delta = input_ms - self.last_input;

        if delta > self.max_forward_jump_ms || delta < -self.max_backward_jump_ms {
            // Discontinuity detected — adjust offset to maintain continuity
            // Output continues from last_output + estimated_frame_duration
            let estimated_duration = 33; // ~30fps fallback
            self.offset = self.last_output + estimated_duration - input_ms;
        }

        self.last_input = input_ms;
        self.last_output = input_ms + self.offset;
        self.last_output.max(0)
    }

    /// Reset the smoother state.
    pub fn reset(&mut self);
}
```

**应用场景**:
- HLS 拉流后 demux 的帧时间戳可能有跳变（segment 边界）
- 推流端重启导致时间戳重置
- 编码器 bug 导致时间戳非单调

**集成点**: 在 `HlsPlaybackPacer` 输出帧之前应用 `StampSmoother`。

---

## 4.4 33-bit PCR/PTS 回绕增强

**现有**: 已有基础回退检测。需增强为完整的 33-bit 回绕处理。

**实现方案**:

```rust
const PCR_WRAP: u64 = 1 << 33; // 8589934592 (90kHz ticks)
const PCR_HALF_WRAP: u64 = PCR_WRAP / 2;

/// Unwrap a 33-bit timestamp relative to a reference, handling wrap-around.
pub fn unwrap_33bit(reference: u64, raw: u64) -> u64 {
    let raw_mod = raw % PCR_WRAP;
    let ref_mod = reference % PCR_WRAP;

    if raw_mod > ref_mod && (raw_mod - ref_mod) > PCR_HALF_WRAP {
        // Backward wrap: raw is actually before reference
        reference.saturating_sub(ref_mod - raw_mod + PCR_WRAP)
    } else if ref_mod > raw_mod && (ref_mod - raw_mod) > PCR_HALF_WRAP {
        // Forward wrap: raw has wrapped past 2^33
        reference + (PCR_WRAP - ref_mod + raw_mod)
    } else {
        // Normal case
        reference + raw_mod.wrapping_sub(ref_mod)
    }
}
```

---

## 4.5 MP2 编码支持

**需求**: MPEG-1 Audio Layer II (MP2) 在广播 TS 流中常见。

**实现方案**:

1. 在 `cheetah-codec` 添加 `CodecId::MP2`
2. TS stream_type `0x03` (MPEG-1 Audio) 和 `0x04` (MPEG-2 Audio) 映射到 MP2
3. TS muxer: MP2 直接写入 PES（无需 ADTS 封装）
4. fMP4 muxer: 使用 `mp4a` box + esds (objectTypeIndication=0x69)

```rust
// cheetah-codec/src/codec_id.rs
pub enum CodecId {
    // ... existing ...
    MP2,
}

// cheetah-hls-core/src/ts_mux.rs — stream_type 映射
fn stream_type_for_codec(codec: CodecId) -> u8 {
    match codec {
        CodecId::MP2 => 0x03, // MPEG-1 Audio
        // ...
    }
}
```

---

## 4.6 厂商 Quirks 兼容层

**原则**: 入口允许兼容脏数据，内部规范化，出口稳定可预测。

**实现方案**:

```rust
// cheetah-hls-core/src/compat.rs (新增)

/// Known vendor quirks for HLS streams.
pub struct HlsCompatConfig {
    /// Allow TS packets with adaptation_field_length exceeding remaining bytes.
    pub allow_oversized_adaptation: bool,
    /// Allow PES without PTS (infer from previous frame).
    pub allow_pes_without_pts: bool,
    /// Allow non-standard stream_type values (map to raw passthrough).
    pub allow_unknown_stream_types: bool,
    /// Tolerate m3u8 with missing #EXTM3U header.
    pub allow_missing_extm3u: bool,
    /// Tolerate segment duration exceeding EXT-X-TARGETDURATION.
    pub allow_duration_overflow: bool,
}

impl Default for HlsCompatConfig {
    fn default() -> Self {
        Self {
            allow_oversized_adaptation: true,
            allow_pes_without_pts: true,
            allow_unknown_stream_types: true,
            allow_missing_extm3u: true,
            allow_duration_overflow: true,
        }
    }
}
```

**已知厂商问题**:
- 海康/大华: TS 中 adaptation field 长度可能超出包剩余空间
- 某些 CDN: m3u8 缺少 `#EXTM3U` 头
- OBS: segment 实际时长可能超过 `EXT-X-TARGETDURATION` 的 1.5 倍
- 某些编码器: 音频 PES 无 PTS，需从视频 PTS 推算

---

## 验证方法

1. 构造含前导垃圾的 TS 数据 → 验证 sync 搜索正确恢复
2. 构造 PES_packet_length=0 的视频流 → 验证正确以 PUSI 分界
3. 注入时间戳跳变 (+10s, -5s) → 验证平滑器输出连续
4. 构造 33-bit 回绕点附近的 PTS → 验证 unwrap 正确
5. 推送含 MP2 音频的 TS → 验证 demux + remux 正确
6. 使用海康摄像头 TS 流 → 验证 compat 层容错
7. 属性测试: 随机 TS 数据 → 不 panic
