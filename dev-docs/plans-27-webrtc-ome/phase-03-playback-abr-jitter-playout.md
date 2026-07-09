# Phase 03: Playback ABR Jitter Playout

- **状态**: 已完成
- **目标**: 补齐 OME 播放侧 playlist/rendition、`WebRtcAutoAbr`、JitterBuffer、PlayoutDelay 与周期性 FIR 兼容，使 WebRTC 播放行为更接近主流流媒体服务器。

## 实现范围

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| 现有 play bridge 与 bootstrap | 已有/复用 | 已有 play subscriber、bootstrap、simulcast 选择 |
| playlist/rendition 语义 | 已完成 | 已映射为 publish bridge rendition snapshot，并通过 session GET 暴露当前/已见 RID |
| ABR 自动切换 | 已完成 | 新增 `webrtc_auto_abr`，可按模块配置启停 BWE/REMB 驱动层选择 |
| JitterBuffer | 已完成 | 新增 `play_jitter_buffer_ms` 并在播放发送链路实现可选平滑延迟 |
| PlayoutDelay hint | 已完成 | 新增 `playout_delay_{min,max}_ms`、有效延迟计算、playout-delay extmap 注入 |
| FIRInterval | 已完成 | 新增 `fir_interval_ms`，周期任务对 WebRTC publish/bidirectional 会话通过 driver `RequestKeyframe(FIR)` 请求关键帧，并对 play/bidirectional 会话向 `StreamManagerApi` 请求上游关键帧 |

## 参考 OME 行为

OME 的播放模型不是单一 answer SDP，而是：

- session 持有 playlist 与当前 rendition。
- `WebRtcAutoAbr` 按带宽估计自动升降档。
- `JitterBuffer` 在低延迟与平滑播放之间折中。
- `PlayoutDelay` 给播放器视频缓冲提示。
- `FIRInterval` 周期性要求关键帧，改善长 GOP 播放恢复。

## 开发任务

### Task 01: playlist/rendition 映射设计落地

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/bridge.rs`
  - 修改: `crates/protocols/webrtc/module/src/session.rs`

验收点：

- 明确 OME playlist/rendition 如何映射到本地 simulcast 或多流模型。
- 保留 operator 可观测的当前层/目标层信息。

实现记录：

- OME playlist/rendition 在本地映射为 `(MID, current_rid, seen_rids)`；RID 顺序复用 Phase 02 的 OME/ZLM 质量序。
- `WebRtcPublishBridge::rendition_snapshot` 和 `WebRtcBridgeRegistry::publish_renditions` 提供内部观测面。
- `GET /session/{id}` 对 publish session 返回 `renditions` JSON 字段，便于后续 ABR/Jitter/FIR 行为排查。

### Task 02: JitterBuffer 与 PlayoutDelay

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/config.rs`
  - 修改: `crates/protocols/webrtc/module/src/bridge.rs`
  - 修改: `crates/protocols/webrtc/core/src/sdp_compat.rs`

验收点：

- 可配置启用 jitter smoothing。
- playout-delay extmap 与配置值能正确出现在 SDP/发送策略中。
- 不破坏现有低延迟默认行为。

实现记录：

- 配置新增 `play_jitter_buffer_ms`、`playout_delay_min_ms`、`playout_delay_max_ms`，默认保持低延迟直通（0）。
- `spawn_play_subscriber` 引入 `PlaybackTimingPolicy` 与平滑状态机，按 `pts_us` + 有效延迟执行可选 `sleep`，并把 `delayed_frames` / `delayed_total_micros` 暴露到 session GET。
- 本地 SDP 在返回前按配置注入 playout-delay extmap（仅 video m-line，避免重复注入）。

### Task 03: 周期性 FIR 与关键帧恢复

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/module.rs`
  - 修改: `crates/protocols/webrtc/core/src/input.rs`

验收点：

- 可配置周期性 FIR/PLI。
- 与现有 NACK storm、BWE 驱动关键帧请求不互相打架。

实现记录：

- 新增配置 `fir_interval_ms`（`0` 关闭）。
- driver 新增 `WebRtcDriverCommand::RequestKeyframe`，贯通 io-front -> shard -> core `WebRtcCoreCommand::RequestKeyframe`。
- module 在启动时按 `fir_interval_ms` 启动周期任务：对已连接 publish/bidirectional 会话按 video MID 发送 FIR；对已连接 play/bidirectional 会话通过 `StreamManagerApi::request_keyframe` 请求对应 `StreamKey` 的上游关键帧，且不影响既有 NACK/BWE 逻辑。

## 测试计划

```powershell
cargo test -p cheetah-webrtc-module play
cargo test -p cheetah-webrtc-module bootstrap
cargo clippy -p cheetah-webrtc-module
```
