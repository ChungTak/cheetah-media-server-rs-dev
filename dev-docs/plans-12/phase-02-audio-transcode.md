# Phase 02 — G711→AAC 实时转码

- **状态**: 未开始
- **范围**: G711A/U 到 AAC-LC 实时转码，使 G711 源可跨协议播放（HTTP-FLV/HLS）
- **完成标准**: G711 推流 → RTMP/HTTP-FLV 拉流可正常播放 AAC 音频

---

## 目标

G711 是 IPC 摄像头最常用的音频编码，但 FLV/HLS 播放器通常只支持 AAC。需要在引擎层提供 G711→AAC 实时转码能力。

---

## 设计

### 架构归属

转码作为独立可选模块 `cheetah-transcode-module`，通过引擎事件总线监听流发布事件，自动为 G711 音频轨创建转码任务。

### 数据流

```
G711 源 → Engine (AVFrame G711Packet)
                ↓ (transcode module 订阅)
         G711 decode → PCM → AAC encode
                ↓
         Engine (AVFrame AacRaw) — 作为新音频轨发布
                ↓
         RTMP/HTTP-FLV 播放器订阅 AAC 轨
```

### 实现要点

- G711 解码：纯算法（A-law/μ-law 查表），无外部依赖
- AAC 编码：使用 `fdk-aac` 或 `libfaac` 绑定（通过 feature flag 可选）
- 采样率转换：G711 8kHz → AAC 通常 44.1kHz/48kHz（需 resample）
- 通道：G711 单声道 → AAC 单声道
- 帧对齐：G711 每帧 160/320 samples → AAC 每帧 1024 samples（需缓冲拼接）

### 配置

```yaml
modules:
  transcode:
    enabled: true
    g711_to_aac:
      enabled: true
      sample_rate: 44100
      bitrate: 64000
```
