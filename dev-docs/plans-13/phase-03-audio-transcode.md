# Phase 03 — G711→AAC 实时转码

- **状态**: 未开始
- **范围**: G711A/U 到 AAC-LC 实时转码，使 G711 源可跨协议播放
- **完成标准**: G711 推流 → RTMP/HTTP-FLV 拉流可正常播放 AAC 音频

---

## 设计

### 架构

转码作为独立可选模块 `cheetah-transcode-module`，通过引擎事件总线监听流发布事件，自动为 G711 音频轨创建转码任务。

### 数据流

```
G711 源 → Engine (AVFrame G711Packet)
                ↓ (transcode module 订阅)
         G711 decode (查表) → PCM 16bit 8kHz mono
                ↓
         Resample 8kHz → 44.1kHz (可选)
                ↓
         AAC-LC encode
                ↓
         Engine (AVFrame AacRaw) — 替换原音频轨
                ↓
         RTMP/HTTP-FLV 播放器订阅 AAC 轨
```

### 关键参数

| 参数 | G711 输入 | AAC 输出 |
|------|-----------|----------|
| 采样率 | 8000 Hz | 44100 Hz（可配置） |
| 位深 | 16 bit (解码后) | — |
| 通道 | 1 (mono) | 1 (mono) |
| 帧大小 | 160 samples (20ms) | 1024 samples |
| 比特率 | 64 kbps | 64 kbps（可配置） |

### 配置

```yaml
modules:
  transcode:
    enabled: true
    g711_to_aac:
      enabled: true
      output_sample_rate: 44100
      output_bitrate: 64000
```

### 依赖

- G711 解码：纯 Rust 查表实现，无外部依赖
- AAC 编码：`fdk-aac-sys` 绑定（通过 feature flag 可选）
- 重采样：简单线性插值或 `rubato` crate
