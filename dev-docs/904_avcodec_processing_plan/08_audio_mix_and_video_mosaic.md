# 08 · 音频混音与视频宫格

## 1. MIX-01：音频混音

`AudioMixSpec`：

- 2–16 个 source StreamKey
- 每路 track selector、gain（dB）、mute
- output sample rate、channels、AAC bitrate
- master clock source 或 system output clock
- 最大 jitter 和 stale timeout

处理过程：

1. 每路音频通过 avcodec 解码为统一 PCM。
2. 使用 avcodec/rubato 统一采样率和 channel layout。
3. 按 canonical PTS 放入每路有界 jitter buffer。
4. 在固定 output quantum 上做 f32 累加、gain 和 hard limiter。
5. 缺包补静音；过晚 frame 丢弃并计数。
6. 混合 PCM 通过 avcodec 编码为 AAC，发布独立目标流。

DSP 累加和 limiter 可以由处理模块实现；codec decode/encode/resample 不得自行实现。

## 2. MIX-02：固定宫格

`VideoMosaicSpec`：

- 2–9 个视频 source
- 预定义 layout：`Grid2x1`、`Grid2x2`、`Grid3x3`
- output width/height/fps/bitrate/GOP
- 每个 tile 的 source、fit policy、可选 label
- 可选 `AudioMixSpec`
- 可选全局图片/文字 overlay

每路视频由 avcodec 解码；通过 resize-pad/crop-resize 生成 tile，使用 blend/OSD 合成到固定 canvas，再由 avcodec 编码为 H.264。输出音频固定为 AAC。

不实现任意坐标图层、动画、转场、动态模板、透明视频或运行时脚本。

## 3. 同步与失联

- 输出时钟固定，不由到达最慢的 source 驱动。
- 每个 tick 选择不晚于目标 PTS 的最近视频 frame；未来帧保留在有界队列。
- 视频短缺继续使用最近帧；超过默认 2 秒 stale timeout 后 tile 变黑并保留 label。
- 音频短缺补静音；恢复时在 frame 边界重新接入，不做无界追赶。
- source reconnect、分辨率或 codec 变化只重建对应 input decoder；输出 encoder 和目标 StreamKey 保持稳定。
- 所有 source 同时失联超过配置 timeout 时 Job Failed。

## 4. 资源与更新

- 创建前一次性 reserve 全部 decoder、processor、encoder、像素率和队列预算。
- 目标 publisher 只有一个，仍遵守单发布者语义。
- gain/mute/label/overlay 更新可在下一 output tick 原子生效。
- layout、输出 codec/尺寸变更需要在下一随机访问点 rebuild output session，并提升 generation。
- 添加/删除 source 使用完整 next spec + expected generation；失败保留旧 spec。

## 5. 测试

- 双音正弦混合验证频率、gain、clipping limiter、静音和 sample count。
- 多采样率/声道/PTS 偏移验证 jitter 对齐和长期漂移。
- 2x1、2x2、3x3 使用纯色/编号 fixture 验证 tile 位置、fit、黑边和 label。
- source stale、重连、动态分辨率、单路 decoder error 不破坏其他 tile。
- 有/无 audio mix 的输出均可由独立 H.264/AAC decoder 播放。
- 16 路音频、9 路视频、像素率和并发配额边界分别验证允许与拒绝。
- 慢输出、cancel、module restart 后无 decoder、encoder、publisher 或缓存泄漏。

## 6. 完成标准

- [ ] 混音和宫格输出是独立派生流，输入源完全不修改。
- [ ] 失联策略确定、可观测且不会让最慢输入拖住整体输出。
- [ ] 所有 codec 与图片算子均使用 avcodec-rs；本地代码只做任务同步和必要 DSP。
- [ ] 资源预留失败和中途失败都能原子回滚。
