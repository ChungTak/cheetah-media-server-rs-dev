# Phase 02 — 跨协议时间戳精度与同步

- **状态**: 未开始
- **范围**: 跨协议时间戳精度保持、A/V 同步对齐、B 帧处理、DTS 生成增强、断流恢复
- **完成标准**: RTMP→RTSP 和 RTSP→RTMP 长时间播放（>1h）音画同步偏差 < 40ms；B 帧流跨协议播放正常

---

## 2.1 跨协议时间戳精度保持（ms↔RTP ticks 无损转换）

**问题**: RTMP 使用毫秒时间戳（1/1000），RTSP 使用 RTP clock rate 时间戳（视频 1/90000，音频 1/48000 或 1/44100）。双向转换时存在精度损失和累积误差。

**ZLMediaKit 方案**: 
- 内部统一使用毫秒作为中间表示
- `RtpInfo::makeRtp()` 转换：`stamp * sample_rate / 1000`
- `Stamp::revise()` 使用增量计算避免绝对值累积误差

**本地现状**:
- `AVFrame` 使用 `timebase` 字段保存原始精度（RTMP 源为 1/1000，RTSP 源为 1/90000）
- `EgressAdapterView` 提供 `rtp_timestamp_ticks` 和 `rtmp_timestamp_ms` 转换
- `media_ts_to_rtp_ticks()` 做 timebase→clock_rate 转换
- 但 RTMP(1/1000) → canonical → RTSP(1/90000) 路径中，1ms = 90 ticks，存在 ±0.5ms 舍入误差

**需要补齐的能力**:

1. **高精度中间表示**：确保 canonical timeline 使用足够精度避免累积舍入
2. **增量转换**：egress 时使用增量而非绝对值转换，避免长时间累积误差
3. **源时间戳透传**：当源和目标使用相同 clock rate 时，直接透传原始时间戳

**实现方案**:

```rust
// cheetah-codec egress.rs — 增量式 RTP 时间戳生成
pub struct IncrementalRtpTimestampGenerator {
    last_media_dts_us: i64,
    last_rtp_timestamp: u32,
    clock_rate: u32,
    // 累积小数部分避免舍入漂移
    fractional_ticks: f64,
}

impl IncrementalRtpTimestampGenerator {
    /// 使用增量计算避免绝对值转换的累积误差
    pub fn next_timestamp(&mut self, dts_us: i64, pts_us: i64) -> RtpEgressTimestamp {
        let delta_us = dts_us - self.last_media_dts_us;
        // 精确增量：delta_us * clock_rate / 1_000_000 + 累积小数
        let exact_ticks = delta_us as f64 * self.clock_rate as f64 / 1_000_000.0 + self.fractional_ticks;
        let int_ticks = exact_ticks.round() as i64;
        self.fractional_ticks = exact_ticks - int_ticks as f64;
        
        self.last_rtp_timestamp = self.last_rtp_timestamp.wrapping_add(int_ticks as u32);
        self.last_media_dts_us = dts_us;
        
        RtpEgressTimestamp {
            rtp_timestamp: self.last_rtp_timestamp,
            // PTS offset for B-frames
            pts_offset_ticks: ((pts_us - dts_us) as f64 * self.clock_rate as f64 / 1_000_000.0).round() as i32,
        }
    }
}

// 源时间戳透传优化
pub fn select_egress_rtp_timestamp(frame: &AVFrame, generator: &mut IncrementalRtpTimestampGenerator) -> u32 {
    // 优先使用源 RTP 时间戳（RTSP→RTSP 零损耗）
    if let Some(source_rtp) = frame.source_rtp_timestamp() {
        return source_rtp;
    }
    // 否则使用增量生成器
    generator.next_timestamp(frame.dts_us, frame.pts_us).rtp_timestamp
}
```

**实现位置**: `cheetah-codec` egress.rs，`cheetah-rtsp-module` play.rs

**验证**:
- 单元测试：1 小时模拟流，验证 RTP 时间戳累积误差 < 1 tick
- 集成测试：RTMP 推流 1 小时 → RTSP 拉流，对比源/目标时间戳偏差

---

## 2.2 跨协议 A/V 时间戳同步对齐

**问题**: 音频和视频可能从不同时间点开始（如视频先到、音频延迟），导致跨协议播放时音画不同步。

**ZLMediaKit 方案**: `Stamp::syncTo()` 让音频 Stamp 同步到视频 Stamp，确保两者的相对时间戳从同一起点开始。`MultiMediaSourceMuxer::trySyncTrack()` 在首帧时执行同步。

**本地现状**:
- `TimestampNormalizer` 各 track 独立归一化，各自从 0 开始
- 但音视频的"第 0 时刻"可能不对齐（如视频首帧 DTS=100ms，音频首帧 DTS=150ms）
- `EgressAdapterView` 不处理 A/V 起始偏移

**实现方案**:

```rust
// cheetah-codec — A/V 同步对齐器
pub struct AvSyncAligner {
    video_epoch_us: Option<i64>,  // 视频首帧的 dts_us
    audio_epoch_us: Option<i64>,  // 音频首帧的 dts_us
    sync_offset_us: i64,          // 音频相对视频的偏移量
    synced: bool,
}

impl AvSyncAligner {
    /// 记录各 track 首帧时间戳，计算同步偏移
    pub fn on_first_frame(&mut self, media_kind: MediaKind, dts_us: i64) {
        match media_kind {
            MediaKind::Video => self.video_epoch_us = Some(dts_us),
            MediaKind::Audio => self.audio_epoch_us = Some(dts_us),
            _ => {}
        }
        if let (Some(v), Some(a)) = (self.video_epoch_us, self.audio_epoch_us) {
            self.sync_offset_us = a - v; // 正值=音频晚于视频
            self.synced = true;
        }
    }

    /// 对 egress 时间戳应用同步偏移
    pub fn adjust_for_egress(&self, media_kind: MediaKind, dts_us: i64) -> i64 {
        if !self.synced { return dts_us; }
        match media_kind {
            // 视频保持不变，音频减去偏移使两者对齐
            MediaKind::Audio => dts_us - self.sync_offset_us,
            _ => dts_us,
        }
    }
}
```

**应用位置**: 在 RTSP/RTMP egress 管线中，`EgressAdapterView` 计算协议时间戳前应用 A/V 同步偏移。

**实现位置**: `cheetah-codec` time.rs，`cheetah-rtsp-module` play.rs，`cheetah-rtmp-module` egress.rs

**配置**:
```yaml
modules:
  rtsp:
    av_sync_alignment: true  # 默认启用
  rtmp:
    av_sync_alignment: true
```

---

## 2.3 B 帧 PTS 回退在 RTSP egress 的正确处理

**问题**: 含 B 帧的 H.264/H.265 流中 PTS 可能小于 DTS（显示顺序 ≠ 解码顺序）。RTMP 通过 CompositionTimeOffset (CTS) 表达，RTSP 通过 RTP timestamp 直接使用 PTS。

**ZLMediaKit 方案**: 
- Video Stamp `enableRollback(true)` 允许 PTS 回退
- RTP packetize 时直接使用 PTS 作为 RTP timestamp（RFC 6184 要求）
- RTMP 使用 DTS + CTS 表达

**本地现状**:
- RTMP ingress 正确解析 CTS，`AVFrame` 中 pts ≠ dts 时设置 `B_FRAME` flag
- RTSP egress 的 `select_egress_timestamps()` 对视频使用 pts 作为 primary
- 但 `repair_monotonic_timestamp()` 可能错误地"修复"了合法的 PTS 回退

**需要修复**:

```rust
// cheetah-rtsp-module play.rs — B 帧时间戳处理
fn compute_rtp_timestamp_for_video(frame: &AVFrame, generator: &mut IncrementalRtpTimestampGenerator) -> u32 {
    // RFC 6184: RTP timestamp = PTS（显示时间）
    // PTS 可能回退（B 帧），这是合法的，不应被 monotonic repair 修正
    let ts = generator.next_timestamp(frame.dts_us, frame.pts_us);
    // 使用 PTS 对应的 RTP timestamp，允许非单调
    ts.rtp_timestamp_pts()
}

// 修改 repair_monotonic_timestamp 的调用条件
fn should_repair_monotonic(media_kind: MediaKind, codec: CodecId) -> bool {
    match media_kind {
        // 音频始终要求单调
        MediaKind::Audio => true,
        // 视频：仅对不含 B 帧的流强制单调
        MediaKind::Video => !codec.may_have_b_frames(),
    }
}
```

**实现位置**: `cheetah-codec` egress.rs，`cheetah-rtsp-module` play.rs

**验证**:
- 单元测试：含 B 帧的 H.264 流（IBP 编码顺序），验证 RTP timestamp 正确反映 PTS
- 集成测试：RTMP 推流含 B 帧 → RTSP 拉流，ffprobe 验证 PTS 顺序正确

---

## 2.4 PTS-only 源的 DTS 生成增强

**问题**: RTSP 推流时 RTP 只携带 PTS（显示时间戳），不提供 DTS。对于含 B 帧的流，需要从 PTS 序列推导 DTS。

**ZLMediaKit 方案**: `DtsGenerator` 维护一个排序窗口（默认 8 帧），对 PTS 排序后输出最小值作为 DTS。

**本地现状**:
- `TimestampNormalizer` 有 `DtsGenerator` 使用平滑步长估计生成 DTS
- 但对于 B 帧密集的流（如 IBBP），步长估计可能不够准确

**实现方案**:

```rust
// cheetah-codec time.rs — 增强 DTS 生成器
pub struct EnhancedDtsGenerator {
    /// 排序窗口：缓存最近 N 帧的 PTS，排序后输出最小值作为 DTS
    pts_window: VecDeque<i64>,
    window_size: usize,  // 默认 8，可配置
    /// 已输出的最大 DTS（保证单调）
    last_output_dts: i64,
    /// 模式检测：是否检测到 B 帧模式
    b_frame_detected: bool,
    /// 回退到步长估计（无 B 帧时）
    step_estimator: StepEstimator,
}

impl EnhancedDtsGenerator {
    pub fn generate_dts(&mut self, pts: i64) -> i64 {
        self.pts_window.push_back(pts);
        
        if self.pts_window.len() < self.window_size {
            // 窗口未满，使用步长估计
            return self.step_estimator.estimate_dts(pts);
        }
        
        // 窗口满：取最小 PTS 作为 DTS 候选
        let min_pts = *self.pts_window.iter().min().unwrap();
        let dts = min_pts.max(self.last_output_dts + 1); // 保证单调
        
        self.pts_window.pop_front();
        self.last_output_dts = dts;
        
        // 检测 B 帧：如果输出 DTS 顺序与输入 PTS 顺序不同
        if !self.b_frame_detected && dts != pts {
            self.b_frame_detected = true;
        }
        
        dts
    }
}
```

**配置**:
```yaml
modules:
  rtsp:
    dts_generator_window_size: 8  # PTS 排序窗口大小
```

**实现位置**: `cheetah-codec` time.rs

**验证**:
- 属性测试：随机 IBBP/IBP/IP 模式的 PTS 序列，验证生成的 DTS 单调递增且 DTS ≤ PTS
- 集成测试：RTSP 推流含 B 帧 → RTMP 拉流，验证 CTS 正确（CTS = PTS - DTS ≥ 0）

---

## 2.5 时间戳断流恢复与连续性保证

**问题**: 推流端网络中断后重连，时间戳可能出现大跳变或回退。跨协议订阅者需要平滑过渡，不能出现播放卡顿或快进。

**ZLMediaKit 方案**: `Stamp::revise()` 检测 >3s 的跳变，自动切换到增量模式。`FrameStamp` 包装器在 `MultiMediaSourceMuxer` 入口统一应用。

**本地现状**:
- `TimestampNormalizer` 有断流检测（`max_forward_gap_us` 默认 2s）
- 检测到断流时设置 `FrameFlags::DISCONTINUITY`
- 但 egress 端对 DISCONTINUITY 的处理未统一

**实现方案**:

```rust
// cheetah-codec — 断流恢复策略
pub enum DiscontinuityRecoveryStrategy {
    /// 重置 egress 时间戳生成器，从当前位置继续（平滑过渡）
    ResetAndContinue,
    /// 插入静默帧填充间隙（适用于音频）
    FillSilence { max_fill_ms: u64 },
    /// 通知订阅者断流（让播放器自行处理）
    NotifySubscriber,
}

// RTSP module play.rs — 断流处理
fn handle_discontinuity_frame(frame: &AVFrame, track_state: &mut PlayTrackState) {
    // 重置 RTP 时间戳生成器
    track_state.rtp_ts_generator.reset(frame.dts_us, frame.pts_us);
    // 发送 RTCP BYE + 新 SSRC（可选，通知播放器重置解码器）
    // 或：保持 SSRC 不变，仅重置时间戳基准（更平滑）
}

// RTMP module play.rs — 断流处理
fn handle_discontinuity_frame(frame: &AVFrame, play_state: &mut PlayTimestampRebaseState) {
    // 重置 RTMP 时间戳 rebase 状态
    play_state.reset_rebase(frame.dts_us);
    // 重新发送序列头（确保解码器状态正确）
    send_sequence_headers_if_keyframe(frame);
}
```

**实现位置**: `cheetah-codec` time.rs，`cheetah-rtsp-module` play.rs，`cheetah-rtmp-module` egress.rs

**验证**:
- 集成测试：模拟推流中断 5s 后重连，验证订阅端播放连续无卡顿
- 验证断流后时间戳不出现大跳变
