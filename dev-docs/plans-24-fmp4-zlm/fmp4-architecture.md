# fMP4 ZLM 对齐总体架构

- **状态**: 规划中
- **范围**: 固定现有 fMP4 crate 的后续完善方向，明确共享容器、HTTP/WS 传输、module 播放/拉流、ZLM 兼容策略和多轨模型。
- **完成标准**: 实现者可以按本文判断能力归属，避免把容器、driver、module 和业务编排混写。

## 架构目标

fMP4 能力继续分为两层：

1. **ISO BMFF/fMP4 容器能力**：box 解析/写入、init segment、media fragment、sample entry、`moof/mdat`、`tfhd/tfdt/trun`、时间戳、H26x length-prefixed payload。这些属于 `cheetah-codec`。
2. **HTTP/WS fMP4 直播协议能力**：URL 路由、HTTP/HTTPS 长连接、WebSocket/WSS binary、播放会话、远端拉流任务。这些属于 `cheetah-fmp4-core + cheetah-fmp4-driver-tokio + cheetah-fmp4-module`。

对齐 ZLMediaKit 时，只对齐外部行为和工程策略，不复制其内部类结构。ZLM 的 `FMP4MediaSource + RingBuffer + PacketCache` 在 Cheetah 中对应 engine stream snapshot、subscriber queue、bootstrap policy、每连接 muxer 和 driver write queue。

## Crate 边界

```text
cheetah-fmp4-module
  -> cheetah-fmp4-driver-tokio
  -> cheetah-fmp4-core

cheetah-fmp4-module -> cheetah-sdk -> cheetah-codec
cheetah-fmp4-driver-tokio -> cheetah-runtime-api
cheetah-codec -> no runtime / no engine / no HTTP
```

约束：

- `cheetah-codec` 只处理容器和媒体数据，不依赖 engine、HTTP、Tokio 或 module。
- `cheetah-fmp4-core` 保持 Sans-I/O，只处理请求状态、路由、WebSocket upgrade、响应模型和 core command。
- `cheetah-fmp4-driver-tokio` 独占 TCP/TLS、HTTP/1.1、chunked、WebSocket frame、pull client、写队列和 backpressure。
- `cheetah-fmp4-module` 独占 engine subscribe/publish、播放 session、pull supervisor、配置校验和 demand mode。
- module 公共接口不得暴露 `tokio::*` 或 `tokio_util::*`。

## 路由与传输

播放路由必须兼容两种形态：

```text
GET /{app}/{stream}.mp4
GET /{app}/{stream}.live.mp4
HEAD /{app}/{stream}.mp4
OPTIONS /{app}/{stream}.mp4
WebSocket GET /{app}/{stream}.mp4
WebSocket GET /{app}/{stream}.live.mp4
```

ZLM 原生 HTTP-fMP4 使用 `.live.mp4`；当前 Cheetah 已支持 `.mp4` 与 `.live.mp4`，后续测试必须覆盖两者。

HTTP 输出：

- `Content-Type: video/mp4`
- `Cache-Control: no-cache`
- `Connection: keep-alive`
- `Transfer-Encoding: chunked`
- 先发送 init segment，再持续发送 media fragment。

WebSocket 输出：

- WebSocket 成功 upgrade 后只发送 binary。
- 每个 binary message 默认承载完整 init segment 或完整 media segment。
- driver 必须支持 ping/pong、close、masked client frame 和 continuation reassembly。
- 单个重组 message 默认上限 4 MiB，对齐 ZLM `MAX_WS_PACKET`。

## 播放数据流

```text
Engine StreamManager
  -> stream snapshot + SubscriberApi
  -> Fmp4PlaySession
  -> cheetah-codec::Fmp4Muxer
  -> Fmp4DriverCommand::SendData
  -> HTTP chunk or WebSocket binary
```

播放启动规则：

1. 等待 stream snapshot，超时关闭连接。
2. 过滤 audio/video track，最多使用 `max_tracks`。
3. 初始化每连接独立 muxer。
4. 发送 init segment。
5. 有视频时等待关键帧起播；audio-only 直接输出。
6. 按关键帧、fragment 时长或样本数量上限 flush media segment。
7. track list 或 codec config 变化时重建 muxer，并在下个关键帧重发 init segment。

## 拉流数据流

```text
HTTP(S)/WS(S)-fMP4 source
  -> driver pull client
  -> cheetah-codec::Fmp4Demuxer
  -> TrackInfo + AVFrame
  -> fMP4 module pull job
  -> PublisherApi exclusive lease
  -> Engine StreamManager
```

拉流规则：

- pull job 启动前获取目标 `StreamKey` 独占发布租约。
- 远端 init segment 解析出 tracks 后调用 `publisher.update_tracks`。
- frame 进入 engine 前统一转换为 canonical `AVFrame + TrackInfo`。
- 重复 init 或 track 变化必须更新 tracks，并标记 discontinuity。
- 连接断开或 demux 连续错误后释放 lease，再按 backoff 重试。

## 多轨道策略

- 默认 `max_tracks = 32`，但 ZLM 默认 `max_track = 2` 需要作为兼容参考写入测试矩阵。
- 所有支持的 audio/video track 写入同一个 `moov`。
- 每个 media fragment 可包含多个 `traf`。
- `TrackInfo.track_id` 优先作为 MP4 track id；冲突时 muxer 内部稳定重映射并记录 diagnostic。
- 输入侧不支持的 track 不能阻塞其他 track。
- 输出侧弱播放器支持的 codec 仍允许封装，但要有 diagnostic 和互操作说明。

## ZLM 兼容策略

必须明确支持：

- `.live.mp4` 路由。
- HTTP 和 WebSocket 共用同一个 fMP4 播放语义。
- init segment 缓存，客户端连接后先发 init。
- 关键帧起播，避免 delta frame 起播。
- demand mode：无人观看时可停止或延后 fMP4 封装，重新有观看者时清旧缓存并等待新关键帧。
- WebSocket message 4 MiB 上限。
- G711 `alaw/ulaw`、Opus `Opus+dOps`、MJPEG/MP2/MP3 的弱标准 MP4 表达。

不照搬：

- 不把 ZLM 的 `FMP4MediaSource` 类直接映射为新共享状态对象。
- 不在 module 侧复制媒体时间戳修正、NALU 转换或参数集缓存；这些必须在 `cheetah-codec`。
- 不为了 demand mode 破坏 engine stream 的发布租约和单发布者语义。

