# Phase 01: RTSP 入口与播放时间戳收敛

- 状态：已完成（任务 1-4 已完成）
- 范围：修复 RTSP 推流入口与 RTSP 播放出口的时间戳策略，消除首帧偶发等待与播放连续性回归。
- 完成标准：RTSP 主线下视频 DTS 严格单调、播放首帧不被固定缓冲阻塞、现有 RTP/RTCP 连续性测试通过。

## 具体任务

### 1. 发布入口视频 DTS 单调修正（已完成）

- [x] 在 `cheetah-rtsp-module` 发布入口为所有视频编码统一执行 DTS 单调修正。
- [x] 取消固定 lookahead 缓冲，避免首帧输出被延后。
- [x] B 帧标记与 DTS/PTS 关系保持一致。

### 2. 发布生命周期状态清理（已完成）

- [x] `ANNOUNCE` 替换会话时清理旧时间戳状态。
- [x] publish `PAUSE` 与连接 `cleanup` 时清理时间戳状态，避免污染下一轮推流。

### 3. 播放端 RTP 时间戳优先级调整（已完成）

- [x] 视频轨优先使用 `PTS` 计算 RTP timestamp。
- [x] 音频轨优先使用 `DTS`，缺失时回退 `PTS`。

### 4. RTSP 回归与稳定性验证（已完成）

- [x] `cargo fmt`
- [x] `cargo clippy -p cheetah-rtsp-module`
- [x] `cargo test -p cheetah-rtsp-module`
- [x] `tcp_interleaved_play_pause_play_rtp_rtcp_continuity` 通过

## 最新进展

- 2026-04-28：任务 1-4 全部完成，并新增非 H264 视频路径回归测试，确认“所有视频编码”都走统一单调 DTS 修正。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-rtsp-module`
- `cargo test -p cheetah-rtsp-module`
