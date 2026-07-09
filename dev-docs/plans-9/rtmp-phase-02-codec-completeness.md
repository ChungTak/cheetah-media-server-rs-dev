# RTMP Phase 02 — 编解码路径补齐

- 状态：已完成
- 范围：补齐 VP8、G.711、MP3、Opus 的完整 RTMP 封装/解封装路径，实现不支持编码的透传机制
- 完成标准：所有目标编码可通过 RTMP 推流并被正确识别、解封装为 AVFrame，可转协议输出；非目标编码可透传

## 目标

确保以下编码在 RTMP 协议中具备完整的 ingest（解封装）和 egress（封装）路径：

**完整支持（可转协议）**：
- 视频：H.264、H.265、VP8、VP9、AV1
- 音频：AAC、G.711 (A-law/μ-law)、Opus、MP3

**透传支持（仅 RTMP→RTMP）**：
- 其他所有编码保持原始 RTMP 消息格式转发，不解析内部结构

## 现状分析

| 编码 | Ingest 状态 | Egress 状态 | 缺口 |
|------|------------|------------|------|
| H.264 | ✅ 完整 | ✅ 完整 | 无 |
| H.265 | ✅ Enhanced RTMP | ✅ Enhanced RTMP | 无 |
| AV1 | ✅ Enhanced RTMP | ✅ Enhanced RTMP | 无 |
| VP9 | ✅ Enhanced RTMP | ✅ Enhanced RTMP | 无 |
| VP8 | ❌ 枚举定义 | ❌ 枚举定义 | 需实现 Enhanced RTMP 路径 |
| AAC | ✅ 完整 | ✅ 完整 | 无 |
| Opus | ✅ Enhanced RTMP | ✅ Enhanced RTMP | 需验证完整路径 |
| G.711 | ⚠️ 枚举定义 | ⚠️ 枚举定义 | 需实现 legacy 音频路径 |
| MP3 | ⚠️ 枚举定义 | ⚠️ 枚举定义 | 需实现 legacy 音频路径 |

## 任务分解

### 2.1 VP8 编解码路径

**目标**：VP8 通过 Enhanced RTMP FourCC 模式完整支持。

**参考**：simple-media-server 的 `RtmpDecodeVPX` / `RtmpEncodeVPX`，VP8 强制使用 enhanced 模式。

**cheetah-rtmp-core 改动**：
- 在 `VideoCodecFourCc` 枚举中确认 `vp08` FourCC 已定义。
- Ingest 路径：Enhanced RTMP video 消息 → 检测 `vp08` FourCC → 提取 VP8 帧数据 → 生成 `AVFrame`。
- Egress 路径：`AVFrame`（VP8）→ 构建 Enhanced RTMP video 消息（FourCC=`vp08`）。
- VP8 无 sequence header 概念（无 decoder config record），首帧即可播放。

**cheetah-codec 改动**：
- 确认 `CodecId::VP8` 已定义。
- VP8 的 `TrackInfo` 不需要 config record，标记为 `config_required: false`。

**测试**：
- 属性测试：VP8 RTMP 消息 encode/decode roundtrip。
- 集成测试：VP8 流推入后可被订阅者正确接收。

### 2.2 G.711 完整路径

**目标**：G.711 A-law (PCMA) 和 μ-law (PCMU) 通过 legacy RTMP 音频格式完整支持。

**参考**：simple-media-server 的 `RtmpDecodeCommon` / `RtmpEncodeCommon`。

**RTMP 音频格式**：
- G.711 A-law：SoundFormat = 7，无 sequence header
- G.711 μ-law：SoundFormat = 8，无 sequence header
- 固定参数：8000Hz、16bit、mono（RTMP 头中的 rate/size/type 字段被忽略）

**cheetah-rtmp-core 改动**：
- Ingest：识别 SoundFormat 7/8 → 直接将 payload 作为 PCM 数据生成 `AVFrame`，无需 sequence header 等待。
- Egress：`AVFrame`（G711A/G711U）→ 构建 legacy 音频消息（SoundFormat=7/8 + raw payload）。
- 不需要 Enhanced RTMP 路径（G.711 是 legacy 编码）。

**cheetah-codec 改动**：
- 确认 `CodecId::G711A` 和 `CodecId::G711U` 已定义。
- G.711 的 `TrackInfo`：sample_rate=8000, channels=1, bits_per_sample=16, config_required=false。

**测试**：
- 单元测试：G.711 音频消息解析。
- 集成测试：G.711 流推入后时间戳正确递增。

### 2.3 MP3 完整路径

**目标**：MP3 通过 legacy RTMP 音频格式完整支持。

**RTMP 音频格式**：
- MP3：SoundFormat = 2，无 sequence header
- 采样率从 RTMP 头 SoundRate 字段获取（0=5.5kHz, 1=11kHz, 2=22kHz, 3=44kHz）
- MP3 8kHz：SoundFormat = 14

**cheetah-rtmp-core 改动**：
- Ingest：识别 SoundFormat 2/14 → payload 即为完整 MP3 帧 → 生成 `AVFrame`。
- Egress：`AVFrame`（MP3）→ 构建 legacy 音频消息（SoundFormat=2 + raw payload）。
- 从 MP3 帧头解析实际采样率（不完全依赖 RTMP 头字段）。

**cheetah-codec 改动**：
- 确认 `CodecId::MP3` 已定义。
- MP3 `TrackInfo`：从帧头提取 sample_rate/channels/bitrate。

**测试**：
- 单元测试：MP3 音频消息解析，采样率提取。
- 属性测试：MP3 RTMP 消息 roundtrip。

### 2.4 Opus 完整路径验证

**目标**：验证现有 Opus Enhanced RTMP 路径的完整性，补齐缺失环节。

**现状**：Opus 通过 Enhanced RTMP audio FourCC 已有基础支持，需验证：
- Opus sequence header（OpusHead）是否正确解析。
- Opus 帧数据是否正确提取。
- Egress 路径是否正确构建 Enhanced RTMP audio 消息。
- 时间戳处理是否正确（Opus 固定 48kHz）。

**验证项**：
1. OpusHead 解析：channels、sample_rate、pre_skip。
2. 多帧 Opus packet 处理。
3. 时间戳：每帧 960 samples @ 48kHz = 20ms。
4. 与 FFmpeg Opus RTMP 推流的互操作性。

**测试**：
- 集成测试：FFmpeg 推 Opus 音频流，验证解析正确。
- 属性测试：Opus Enhanced RTMP 消息 roundtrip。

### 2.5 编解码能力协商

**目标**：在 connect 响应中正确声明服务器支持的编解码能力。

**实现**：
- 在 connect `_result` 中设置 `capabilities` 字段（参考 simple-media-server 设置为 255）。
- 在 onMetaData 中正确映射 `videocodecid` / `audiocodecid`：
  - 支持数字 ID 和字符串名称两种格式。
  - 支持 FourCC 值映射。
- 当收到不支持的编码时，记录日志但不断开连接。

**cheetah-rtmp-core 改动**：
- 扩展 connect 响应构建，包含完整 capabilities。
- metadata 解析增加 codec ID 字符串→枚举映射。

### 2.6 不支持编解码的透传

**目标**：对于不在完整支持列表中的编码，实现原始消息透传。

**设计**：
- 当 ingest 检测到未知 codec ID / FourCC 时：
  - 生成 `AVFrame` 但标记 `codec: CodecId::Unknown(raw_id)`。
  - 保留原始 RTMP 消息 payload 不解析。
  - `TrackInfo` 标记为 `transcode_capable: false`。
- Egress 路径：
  - 对于 `CodecId::Unknown` 的帧，直接使用保存的原始 payload 构建 RTMP 消息。
  - 不尝试转协议（RTSP/HTTP-FLV 等不输出此类帧）。
- 透传仅在 RTMP→RTMP 场景有效（Pull→Play 或 Relay）。

**cheetah-codec 改动**：
- `CodecId` 增加 `Unknown(u32)` 变体。
- `AVFrame` 支持携带原始未解析 payload（`Bytes` 字段）。

**测试**：
- 集成测试：推送未知编码流，验证 RTMP 订阅者可正确接收原始数据。
- 验证未知编码不会导致 panic 或连接断开。

## 与 cheetah-codec 的边界

本阶段的改动严格遵守分层：
- **cheetah-rtmp-core**：只负责 RTMP 消息格式的封装/解封装映射，不做编解码内部处理。
- **cheetah-codec**：负责 config record 解析、参数集管理、时间戳归一化。
- 新增编码路径时，先确认 `cheetah-codec` 中对应的 `CodecId` 和 `TrackInfo` 支持，再在 RTMP core 中添加映射。

## 验证矩阵

| 编码 | 推流工具 | 拉流验证 | 转协议验证 |
|------|---------|---------|-----------|
| VP8 | FFmpeg `-c:v libvpx` | RTMP play | HTTP-FLV |
| G.711A | FFmpeg `-c:a pcm_alaw` | RTMP play | RTSP |
| G.711U | FFmpeg `-c:a pcm_mulaw` | RTMP play | RTSP |
| MP3 | FFmpeg `-c:a libmp3lame` | RTMP play | HTTP-FLV |
| Opus | FFmpeg `-c:a libopus` | RTMP play | WebRTC |
| Unknown | 自定义 RTMP 客户端 | RTMP play | 不转协议 |
