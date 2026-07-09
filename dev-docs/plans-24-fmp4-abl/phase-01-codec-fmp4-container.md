# Phase 01 — 共享 fMP4 容器与时间戳补强

- **状态**: 规划中
- **范围**: 在 `cheetah-codec` 中补齐与 ABL 对齐最相关的容器、时间戳、参数集和关键帧起播基础语义。
- **完成标准**: 直播播放和后续录像路径都复用同一套共享 fMP4 容器逻辑。

## 1.1 sample entry 与输入兼容

保持现有输出矩阵，并补强 ABL 关注的弱标准输入：

- H264: `avc1` 输出，兼容 `avc2/avc3/avc4` 输入。
- H265: `hvc1` 输出，兼容 `hev1/dvh1/dvhe` 输入。
- AAC: `mp4a + esds(0x40)`。
- G711A/U: `alaw/ulaw`。
- MP3: `mp4a + esds(0x69)`，输入兼容 `0x69/0x6B`。
- MP2: `mp4a + esds(0x6B)`。
- MJPEG: `mp4v + esds(0x6C)`，输入兼容 `jpeg/mjpa/mjpb`。
- Opus/VP8/VP9/AV1 保持当前实现。

## 1.2 参数集与 init segment

对齐 ABL 的“参数集未就绪不输出”行为：

- H264 在 SPS/PPS 未就绪时不生成 video track init。
- H265 在 VPS/SPS/PPS 未就绪时不生成 video track init。
- 参数集变化时允许上层重建 muxer 并重发 init。
- demux 对重复 init segment 要能更新 `TrackInfo` 而不是崩溃或泄漏旧状态。

## 1.3 时间戳与真实帧率

ABL 版本信息多次修正真实帧率和 live/replay 时间戳。第一阶段先把公共能力打好：

- 继续以 canonical `AVFrame.pts/dts` 和 `*_us` 作为唯一真值。
- fragment duration 不依赖固定 25fps 假设，只从相邻 sample 时间戳计算。
- B-frame composition offset 继续支持负值。
- timescale 为 0、duration 反常、sample 时间回退时输出 diagnostic，不 panic。
- 为未来 replay 路径预留“按 frame index 还原时间戳”的可扩展点，但第一阶段不直接做 replay。

## 1.4 关键帧与 bootstrap 基础语义

容器层需要给 module 足够信号：

- `Fmp4MuxSample.is_keyframe` 作为 fragment 边界和 `sidx SAP` 计算基础。
- demux 继续从 `trun` flags 恢复 keyframe。
- 为未来“最近 I 帧/GOP bootstrap”策略保留辅助诊断，明确当前 fragment 是否从 keyframe 起始。

## 1.5 输入鲁棒性

保留已完成的 bounded 处理，并补充 ABL 风格样例：

- 无 `styp` / 无 `sidx` 的 `moof+mdat`。
- 重复 init。
- arbitrary chunk split。
- unknown top-level box。
- 异常 box size。
- H265 late parameter sets。

## 1.6 测试

```bash
cargo test -p cheetah-codec -- fmp4
cargo test -p cheetah-fmp4-property-tests
```

补充场景：

- 参数集晚到前不输出有效 video init。
- H265 `hev1/dvh1/dvhe` 输入正确识别为 H265。
- MP3 `0x69/0x6B` 输入映射稳定。
- MJPEG `jpeg/mjpa/mjpb` 兼容输入稳定。
- 重复 init 后 track 列表更新且无旧状态残留。
- fragment duration 与实际 sample 时间戳一致，不依赖固定 fps。
