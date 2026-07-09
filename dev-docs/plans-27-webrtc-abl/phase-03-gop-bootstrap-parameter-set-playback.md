# Phase 03: GOP Bootstrap Parameter Set Playback

- **状态**: 未开始
- **目标**: 对标 ABL 的首屏播放和参数集补发修复，减少 WebRTC 播放黑屏、切流失败和回放异常。

## 实现范围

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| 播放启动等待首帧/关键帧 | 部分具备 | 本地已有 bootstrap 配置，需结合 ABL 样例验证 |
| H264 SPS/PPS 补发 | 部分具备 | 应沉淀到 `cheetah-codec` |
| H265 VPS/SPS/PPS 补发 | 部分具备 | 同上 |
| 非监控源 IDR 缺参数集兼容 | 未开始 | ABL 2025-10-14 发布记录明确修复 |
| 回放首包帧号处理 | 未开始 | 与 Phase 02 timestamp 策略联动 |

## 参考 ABL 行为

ABL 在 2025-10-14 记录中修正：非监控设备的视频流 I 帧可能不携带 SPS/PPS/VPS，WebRTC 播放需要在 I 帧前主动补参数集。该行为属于媒体归一化问题，不应散落在 WebRTC module 热路径。

## 开发任务

### Task 01: 建立参数集缓存能力核查清单

- **状态**: 未开始
- **建议文件**:
  - 检查: `crates/foundation/codec` 中 H264/H265 Access Unit 与参数集模块
  - 修改: 缺失能力对应的 `cheetah-codec` 模块

验收点：

- H264 可从 Annex-B 或 AVCC 输入中识别 SPS/PPS。
- H265 可识别 VPS/SPS/PPS。
- IDR 前可生成包含参数集的输出视图。
- 缓存大小有上界，异常参数集不会导致无限增长。

### Task 02: WebRTC 输出使用 codec bootstrap 视图

- **状态**: 未开始
- **建议文件**:
  - 修改: WebRTC module 播放输出适配
  - 修改: WebRTC RTP packetizer 接入点

验收点：

- WebRTC 不直接维护私有 SPS/PPS/VPS map。
- 新订阅者启动时优先发送可解码的关键帧序列。
- 若没有关键帧，遵守现有 wait timeout 并返回可观测诊断。
- H264 B-frame filter 与参数集补发互不覆盖。

### Task 03: 增加真实样例回归

- **状态**: 未开始
- **建议文件**:
  - 新增: WebRTC 或 codec 测试 fixtures
  - 修改: 对应测试文件

验收点：

- 样例覆盖 IDR 缺 SPS/PPS。
- 样例覆盖 H265 IDR 缺 VPS/SPS/PPS。
- 样例覆盖已有参数集变化后的更新。

## 测试计划

```powershell
cargo test -p cheetah-codec parameter
cargo test -p cheetah-webrtc-module bootstrap
cargo clippy -p cheetah-codec
cargo clippy -p cheetah-webrtc-module
```

新增测试名称建议：

- `h264_idr_without_sps_pps_is_bootstrapped`
- `h265_idr_without_vps_sps_pps_is_bootstrapped`
- `webrtc_new_subscriber_receives_decodable_keyframe`
- `bootstrap_timeout_reports_missing_keyframe`
