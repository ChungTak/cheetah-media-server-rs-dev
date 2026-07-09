# Phase 03 — 多格式录制 + RTSP VOD 回放

- **状态**: 未开始
- **范围**: FMP4/MP4/TS/FLV 录制、分片策略、保留策略、RTSP VOD 回放（Seek/Pause/Speed）
- **完成标准**: 可通过 API 启停录制，生成合法媒体文件；RTSP 客户端可回放录制文件

---

## 3.1 录制引擎

### 支持格式

| 格式 | 用途 | 优先级 |
|------|------|--------|
| FLV | RTMP 生态兼容 | 高 |
| FMP4 | 浏览器直接播放 | 高 |
| TS | HLS 分片 | 中 |
| MP4 | 通用存储 | 中 |

### 分片策略

- 按时长分片（默认 180 秒）
- 按文件大小分片
- 仅在关键帧处切割

### 保留策略

- 按最大保留时长自动清理
- 按磁盘空间水位自动清理
- 清理时发送 Webhook 事件

---

## 3.2 RTSP VOD 回放

### 功能

- DESCRIBE 返回录制文件的 SDP
- PLAY 支持 Range 头（NPT 时间定位）
- PAUSE/PLAY 暂停恢复
- Scale 头支持倍速（1x/2x/4x/8x）
- 多文件连续播放（跨分片无缝衔接）

### 实现位置

`cheetah-rtsp-module` 中增加 VOD 播放路径，通过 URL 模式区分直播和回放：
- 直播：`rtsp://host/live/stream`
- 回放：`rtsp://host/record/stream?start=20240101T120000&end=20240101T130000`
