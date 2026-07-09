# Phase 02: Codec Payload Audio Timestamp

- **状态**: 未开始
- **目标**: 补齐 ABL 对标中的动态 payload、音频兼容策略和 RTP timestamp 行为。

## 实现范围

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| 视频 payload 从 offer 提取 | 部分具备 | 不使用 ABL 固定 H264=103/H265=109 |
| Opus payload 从 offer 提取 | 部分具备 | 以浏览器 offer 为准 |
| G711A/G711U 直通 | 部分具备 | policy 已表达，需输出路径验证 |
| AAC/MP3 转 Opus | 未开始 | 优先放 `cheetah-codec` 或明确适配层 |
| live/replay timestamp 策略 | 未开始 | 区分直播自增与回放帧号/源时间 |

## 参考 ABL 行为

ABL 在 2025-06-12 修正了 H264 payload 必须从浏览器 SDP 提取的问题；2025-12-01 修正 Opus payload 提取；2025-07-28、2025-11-26、2025-12-01 连续调整 G711、AAC/MP3 到 Opus 的音频策略；2025-12-25 明确 WebRTC 回放音频 timestamp 应按帧号派生。

## 开发任务

### Task 01: payload 解析收敛为纯函数

- **状态**: 未开始
- **建议文件**:
  - 修改: `crates/protocols/webrtc/core` 中 SDP 相关模块
  - 修改: `crates/protocols/webrtc/module/src/http.rs`
  - 测试: `crates/protocols/webrtc/core` 或 `module` 的 SDP 测试

验收点：

- 从 offer 中提取 `H264/90000`、`H265/90000`、`opus/48000` 对应 payload。
- codec 名大小写不敏感。
- 找不到 payload 时返回结构化错误，module 释放会话。
- answer 使用 offer 中协商成功的 payload，不回退到固定常量。

### Task 02: 音频输出策略与配置

- **状态**: 未开始
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/codec_policy.rs`
  - 修改: `crates/protocols/webrtc/module/src/config.rs`
  - 视情况修改: `crates/foundation/codec` 下音频适配能力

验收点：

- G711A 使用 payload 8、G711U 使用 payload 0 时可直通。
- AAC/MP3 面向 Browser profile 时优先输出 Opus。
- 当转码能力不可用时，错误信息明确指出 codec 不可协商，而不是静默无音频。
- Opus 输出使用 48kHz、stereo、960 sample frame 的浏览器友好配置或等价内部表达。

### Task 03: timestamp 策略区分直播与回放

- **状态**: 未开始
- **建议文件**:
  - 修改: `cheetah-codec` 时间戳归一化相关模块
  - 修改: WebRTC module 输出适配
  - 测试: `cheetah-codec` 时间戳单元测试与 WebRTC module 回归测试

验收点：

- 直播场景可沿用单调递增/源时钟归一化策略。
- 回放场景优先使用源帧号或源 PTS 派生 RTP timestamp。
- G711 20ms 包 timestamp 步进为 160 或 320 时必须由采样率和 ptime 明确计算，不写死在 module。
- Opus timestamp 使用 48kHz clock。

## 测试计划

```powershell
cargo test -p cheetah-webrtc-core sdp
cargo test -p cheetah-webrtc-module codec
cargo test -p cheetah-codec timestamp
cargo clippy -p cheetah-webrtc-module
```

新增测试名称建议：

- `offer_payload_parser_uses_browser_h264_payload`
- `offer_payload_parser_uses_browser_opus_payload`
- `g711_passthrough_keeps_static_payload`
- `aac_browser_profile_requires_opus_output`
- `replay_timestamp_is_derived_from_source_frame_time`
