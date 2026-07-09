# Phase 01 — Egress 鲁棒性

- **状态**: 未开始
- **范围**: A/V 同步校正集成、自动帧率检测、写入错误容忍
- **完成标准**: 长时间推流音视频不漂移；帧率可从 PTS 自动推断；网络抖动不导致误断

---

## 1.1 A/V 时间戳同步校正集成

**问题**: 长时间跨协议播放后，视频和音频时间戳可能逐渐漂移，导致唇音不同步。

**ABLMediaServer 方案**: `SyncVideoAudioTimestamp()` 每 500ms 比较 video DTS 与 audio DTS，若差值超过阈值则调整视频时间戳增量 ±10ms。

**本地现状**: `AvSyncAligner`（cheetah-codec egress.rs）已实现首帧对齐，但未集成到 RTMP/RTSP play 管线中。

**实现方案**:

在 RTMP module play 循环中，将已有的 `av_sync` 逻辑与 `AvSyncAligner` 统一：

```rust
// RTMP module play loop — 已有 av_sync.check() 逻辑
// 需要确认：当前 av_sync 是否覆盖了 ABL 的 ±10ms/500ms 校正
// 如果已覆盖，标记为已完成；如果未覆盖，补充周期性校正
```

在 RTSP module play 循环中集成 `AvSyncAligner`：

```rust
// RTSP play loop — 在计算 RTP timestamp 前应用 A/V 对齐
let adjusted_dts_us = av_aligner.adjust(track.media_kind, frame.dts_us);
let rtp_ts = media_ts_to_rtp_ticks(adjusted_dts_us, ...);
```

**实现位置**: `cheetah-rtmp-module` module.rs（验证现有 av_sync），`cheetah-rtsp-module` play.rs

**配置**:
```yaml
modules:
  rtsp:
    av_sync_correction: true
    av_sync_check_interval_ms: 500
    av_sync_max_drift_ms: 100
```

---

## 1.2 自动帧率检测

**问题**: 推流端 metadata 中的帧率可能不准确或缺失，影响时间戳生成和 pacing。

**ABLMediaServer 方案**: `CalcFlvVideoFrameSpeed()` 收集 250 个 PTS 差值样本，取平均值作为实际帧率。

**本地现状**: 无帧率检测。`TrackInfo.fps` 依赖推流端声明。

**实现方案**:

```rust
// cheetah-codec — 帧率估计器
pub struct FrameRateEstimator {
    samples: VecDeque<i64>,  // PTS 差值（微秒）
    max_samples: usize,      // 250
    last_pts_us: Option<i64>,
}

impl FrameRateEstimator {
    pub fn on_frame(&mut self, pts_us: i64) -> Option<f64> {
        if let Some(last) = self.last_pts_us {
            let delta = pts_us.saturating_sub(last);
            if delta > 0 && delta < 1_000_000 { // 排除异常值
                self.samples.push_back(delta);
                if self.samples.len() > self.max_samples {
                    self.samples.pop_front();
                }
            }
        }
        self.last_pts_us = Some(pts_us);
        if self.samples.len() >= self.max_samples {
            let avg_us: i64 = self.samples.iter().sum::<i64>() / self.samples.len() as i64;
            Some(1_000_000.0 / avg_us as f64)
        } else {
            None
        }
    }
}
```

**用途**:
- 更新 `TrackInfo.fps` 字段
- 用于 egress pacing 计算
- 用于 mute audio 注入间隔

**实现位置**: `cheetah-codec` egress.rs，RTMP/RTSP module ingest 管线

---

## 1.3 写入错误容忍

**问题**: 网络瞬时抖动导致单次写入失败就断开连接，过于激进。

**ABLMediaServer 方案**: 允许 30 次连续写入失败才断开（`nWriteErrorCount >= 30`）。

**本地现状**: RTMP driver 写入失败立即断开。

**实现方案**:

在 RTMP driver 的连接写入循环中增加错误计数：

```rust
// cheetah-rtmp-driver-tokio server.rs — 写入循环
let mut consecutive_write_errors: u32 = 0;
match tcp_write_result {
    Ok(_) => consecutive_write_errors = 0,
    Err(_) => {
        consecutive_write_errors += 1;
        if consecutive_write_errors >= config.max_write_errors {
            break; // 断开
        }
        continue; // 容忍，继续下一帧
    }
}
```

**配置**:
```yaml
modules:
  rtmp:
    max_write_errors: 30
  rtsp:
    max_write_errors: 30
```

**实现位置**: `cheetah-rtmp-driver-tokio` server.rs，`cheetah-rtsp-driver-tokio` connection.rs
