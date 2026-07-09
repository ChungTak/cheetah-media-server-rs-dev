# Phase 01 — 时间戳鲁棒性与播放质量

- **状态**: 未开始
- **范围**: 音视频同步校正、自动帧率检测、写入错误容忍
- **完成标准**: 长时间推流音视频不漂移；帧率可从 PTS 自动推断；网络抖动不导致误断

---

## 1.1 音视频时间戳同步校正

**问题**: 长时间推流后，视频和音频时间戳可能逐渐漂移，导致播放端唇音不同步。

**ABLMediaServer 方案**: `SyncVideoAudioTimestamp()` 每 500ms 比较视频 DTS 与音频 DTS，若差值超过阈值则调整视频时间戳增量 ±10ms。

**本地实现方案**:

在 RTMP module egress（play 管线）中增加 A/V 同步监控：

```rust
struct AvSyncState {
    last_check_micros: u64,
    last_video_dts_ms: u32,
    last_audio_dts_ms: u32,
    video_drift_correction_ms: i32, // 累计校正量
}

impl AvSyncState {
    /// 每 500ms 检查一次，若 video_dts - audio_dts 偏差 > 阈值，
    /// 对后续视频帧 DTS 施加 ±10ms 微调。
    fn check_sync(&mut self, video_dts_ms: u32, audio_dts_ms: u32, now_micros: u64) -> i32;
}
```

**实现位置**: `cheetah-rtmp-module` egress 管线，在 `clamp_media_command_timestamp` 之后应用。

**配置**:
```yaml
modules:
  rtmp:
    av_sync_correction: true  # 默认启用
    av_sync_check_interval_ms: 500
    av_sync_max_drift_ms: 100
    av_sync_step_ms: 10
```

---

## 1.2 自动帧率检测

**问题**: 推流端 metadata 中的帧率可能不准确或缺失，影响时间戳生成和 pacing。

**ABLMediaServer 方案**: `CalcFlvVideoFrameSpeed()` 收集 250 个 PTS 差值样本，取平均值作为实际帧率。

**本地实现方案**:

在 RTMP module ingest 管线中增加帧率采样器：

```rust
struct FrameRateEstimator {
    samples: VecDeque<i64>,  // PTS 差值（微秒）
    max_samples: usize,      // 250
    last_pts_us: Option<i64>,
    estimated_fps: Option<f64>,
}

impl FrameRateEstimator {
    fn on_frame(&mut self, pts_us: i64) -> Option<f64>;
}
```

**用途**:
- 更新 `TrackInfo.fps` 字段
- 用于 egress pacing 计算
- 用于 mute audio 注入间隔

**实现位置**: `cheetah-rtmp-module` ingest 管线，每个视频帧到达时调用。

---

## 1.3 写入错误容忍

**问题**: 网络瞬时抖动导致单次写入失败就断开连接，过于激进。

**ABLMediaServer 方案**: 允许 30 次连续写入失败才断开。

**本地实现方案**:

在 RTMP driver 的连接写入循环中增加错误计数：

```rust
// driver server.rs 写入循环
let mut consecutive_write_errors: u32 = 0;
// 写入成功时重置
consecutive_write_errors = 0;
// 写入失败时递增
consecutive_write_errors += 1;
if consecutive_write_errors >= config.max_write_errors {
    break; // 断开
}
```

**配置**:
```yaml
modules:
  rtmp:
    max_write_errors: 30  # 默认 30，0 = 立即断开（当前行为）
```

**实现位置**: `cheetah-rtmp-driver-tokio` server.rs 写入循环。
