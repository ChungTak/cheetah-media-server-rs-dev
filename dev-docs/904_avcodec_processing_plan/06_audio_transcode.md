# 06 · 音频转码

## 1. 发布矩阵

| 输入 | 输出 | 必需场景 |
| --- | --- | --- |
| G711A / G711U | AAC-LC | RTMP、HTTP-FLV、HLS |
| AAC-LC | G711A / G711U | 监控/语音兼容输出 |
| G711A / G711U | Opus | WebRTC |
| Opus | G711A / G711U | 监控/语音兼容输出 |
| AAC-LC | Opus | WebRTC browser |
| Opus | AAC-LC | RTMP、HTTP-FLV、HLS |
| MP3 | Opus | SRT/TS 等输入到 WebRTC |

不承诺 Opus/AAC → MP3、HE-AAC 编码或未在 preflight matrix 中出现的转换。

## 2. AUD-01：上游适配

- G711、Opus、FDK-AAC 和 rubato 只通过顶层 avcodec feature 启用。
- MP3 依赖 UP-01；没有包含 MP3 的 pinned revision 时相关 capability/operation 不注册。
- 音频 Registry 在处理模块内部按已编译 backend 构造，selection report 转换为稳定 diagnostic。
- 删除 `cheetah-codec::transcode` 的 `AacDecoder/AacEncoder/OpusDecoder/OpusEncoder` traits、G711 查表和 nearest-neighbor resampler。

## 3. 统一处理链

```text
compressed AVFrame
  -> avcodec decoder
  -> planar/interleaved PCM normalization
  -> avcodec/rubato resample + channel adapt
  -> avcodec encoder
  -> compressed AVFrame + TrackInfo
```

- 输入 timebase、sample rate、channels 和 codec config 必须来自 Ready `TrackInfo`。
- 输出时钟由累计编码 sample 数生成，不能从 wall clock 或每包输入 PTS 重新取整。
- discontinuity 清空重采样残留并在输出标记 discontinuity；不得跨断流拼接 PCM。
- AAC 输出固定为 AAC-LC；RTMP/FLV 输出 AudioSpecificConfig，TS/HLS 输出适合封装的 AAC view。
- Opus 输出固定 48 kHz；ptime 默认 20 ms，可配置范围 10–60 ms。
- G711 输出固定 8 kHz mono；输入多声道先按明确 downmix matrix 合并。

## 4. Backpressure 与错误

- 音频 frame 不做字节级截断；队列满时丢完整 frame，增加 discontinuity 和 drop counter。
- decoder/encoder `Pending` 正常推进，不触发任务重启。
- bitstream 错误按可恢复 packet error 与不可恢复 session error 分类；连续错误达到配置阈值后 Failed。
- flush 必须输出 encoder delay 和 resampler 尾部；测试验证尾样本数量。
- 动态 sample-rate/channel 变化触发受控 session rebuild，并在新输出前发布更新后的 TrackInfo。

## 5. 协议策略

- WebRTC `Auto`：源为 AAC/G711/MP3 且 Opus preflight 通过时复用 Opus 派生任务；否则延续当前 auto 丢弃不兼容音频但保持视频。
- WebRTC `Transcode`：无法创建 Opus 派生任务则会话失败。
- RTMP/HTTP-FLV/HLS `Auto`：G711/Opus 转 AAC；没有处理能力时不宣称支持该音轨。
- Pull Proxy `Passthrough` 不转码；`Auto/Transcode` 创建显式目标或共享目标。

## 6. 测试

- 每个矩阵方向使用固定正弦/语音 fixture，验证输出可解码、sample count、sample rate、channels 和 timestamp。
- 与独立 decoder 比较时长、频率和 RMS；禁止仅检查非空 bytes。
- 覆盖 8k/16k/44.1k/48k、mono/stereo、不同 packet duration、PTS 抖动、回绕和断流。
- 验证 AAC codec config、Opus pre-skip/ptime、G711 A/μ law golden。
- MP3 覆盖 CBR/VBR、不同采样率、truncated frame 和 flush。
- WebRTC Chrome 真实播放确认 inbound audio packets、concealment 和 audible tone；无处理 feature 时验证现有降级。

## 7. 完成标准

- [ ] 所有七条必需转换有 production provider 和非空、可独立解码证据。
- [ ] 时间戳和样本累计在长稳中无持续漂移。
- [ ] 处理 feature 关闭时不构建音频 backend，能力和协议行为诚实降级。
- [ ] `cheetah-codec` 不再包含编解码 session、私有 encoder trait 或临时重采样器。
