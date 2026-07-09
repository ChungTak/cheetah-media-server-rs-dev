# TS 总体架构设计

- **状态**: 规划中
- **范围**: 固定 TS 协议 crate 边界、共享 MPEG-TS 容器 API、HTTP/WS 传输语义、播放/拉流数据流、多轨道与兼容策略。
- **完成标准**: 实现者能够据此拆出 `cheetah-ts-core + cheetah-ts-driver-tokio + cheetah-ts-module`，并将 TS 容器能力收敛到 `cheetah-codec`。

## 架构目标

TS 协议在本项目中分为两类能力：

1. **MPEG-TS 容器能力**：PAT/PMT/PES/PCR/PTS/DTS、stream_type、PID、continuity、packet 重同步。这属于共享媒体基础层，应放入 `cheetah-codec`。
2. **HTTP/WS TS 直播协议能力**：URL 路由、HTTP/HTTPS 长连接、WebSocket/WSS binary、播放会话、远端拉流任务。这属于独立 `ts` 协议三段式 crate。

首版实现播放和拉流，不实现客户端推流。播放方向是 engine `AVFrame + TrackInfo` 到 TS bytes；拉流方向是远端 TS bytes 到 engine `AVFrame + TrackInfo`。

## Crate 与依赖方向

新增目录与 package：

```text
crates/protocols/ts/
  core/                    # cheetah-ts-core
  driver-tokio/            # cheetah-ts-driver-tokio
  module/                  # cheetah-ts-module
  testing/property-tests/  # cheetah-ts-property-tests
  fuzz/                    # standalone cargo-fuzz workspace
```

依赖方向固定为：

```text
cheetah-ts-module
  -> cheetah-ts-driver-tokio
  -> cheetah-ts-core
  -> cheetah-codec

cheetah-ts-module -> cheetah-sdk -> cheetah-codec
cheetah-ts-driver-tokio -> cheetah-runtime-api
```

禁止关系：

- `cheetah-ts-core` 不依赖 Tokio、SDK、engine、socket、HTTP 框架
- `cheetah-ts-module` 不直接依赖 `tokio::net`、`tokio::time`、`tokio::sync`
- `cheetah-codec` 不依赖 TS module、HTTP、数据库、engine 或 runtime
- TS module 不复制 HLS 私有 TS mux/demux 逻辑

## 共享 MPEG-TS 容器 API

`cheetah-codec` 新增：

```rust
pub struct MpegTsMuxer { /* Sans-I/O */ }

pub struct MpegTsTrackDesc {
    pub track_id: TrackId,
    pub media_kind: MediaKind,
    pub codec: CodecId,
    pub clock_rate: u32,
}

pub enum MpegTsMuxEvent {
    Packet(Bytes),
    Diagnostic(MpegTsDiagnostic),
}

pub struct MpegTsDemuxer { /* Sans-I/O */ }

pub enum MpegTsDemuxEvent {
    TrackFound(TrackInfo),
    Frame(AVFrame),
    Diagnostic(MpegTsDiagnostic),
}
```

设计要求：

- muxer 输入 `TrackInfo` 和 `AVFrame`，输出按 188 字节对齐的 TS bytes
- demuxer 输入任意切片 bytes，输出 `TrackInfo` 与 `AVFrame`
- H264/H265/H266 输出使用 Annex-B，并在关键帧前补参数集
- AAC 输出到 TS 时必须带 ADTS；输入 ADTS 后进入 engine 可还原为 `AacRaw`
- PTS/DTS 输出使用 90kHz，来源为 canonical timeline
- 输入端保留 source timestamp side data，进入 engine 时仍以 canonical timeline 为准

## HTTP / WebSocket 路由

播放路由：

```text
GET  /{app}/{stream}.ts
HEAD /{app}/{stream}.ts
OPTIONS /{app}/{stream}.ts
WebSocket GET /{app}/{stream}.ts
```

规则：

- `{app}/{stream}` 映射到 `StreamKey::new(app, stream)`
- 路径必须以 `.ts` 结尾
- query 首版只保留扩展参数，不定义播放器模式
- HTTP 响应头：`Content-Type: video/mp2t`、`Cache-Control: no-cache`、`Connection: keep-alive`、CORS
- WebSocket 成功 upgrade 后仅发送 binary message
- WebSocket binary message 必须由完整 TS packet 组成，message 可包含多个 packet

拉流源 URL：

```text
http://host/{app}/{stream}.ts
https://host/{app}/{stream}.ts
ws://host/{app}/{stream}.ts
wss://host/{app}/{stream}.ts
```

## 播放输出数据流

```text
Engine StreamManager
  -> SubscriberSource<Arc<AVFrame>>
  -> TsPlaySession
  -> cheetah-codec::MpegTsMuxer
  -> cheetah-ts-driver-tokio command channel
  -> HTTP body bytes or WebSocket binary messages
```

播放启动顺序：

1. 等待 stream snapshot，拿到 `TrackInfo`
2. 初始化多轨 TS muxer
3. 发送 PAT/PMT
4. 有视频时等待关键帧起播，避免 delta frame 起播
5. 关键帧前补参数集；H264/H265 写 AUD
6. 按帧生成 PES，并按有界 write queue 输出
7. track 变化或 discontinuity 时补发 PAT/PMT，并在下个关键帧恢复输出

## 远端拉流数据流

```text
HTTP(S)/WS(S)-TS source
  -> cheetah-ts-driver-tokio pull client
  -> cheetah-codec::MpegTsDemuxer
  -> TrackInfo + AVFrame
  -> cheetah-ts-module pull job
  -> PublisherApi exclusive lease
  -> Engine StreamManager
```

拉流规则：

- pull job 启动时获取目标 `StreamKey` 独占发布租约
- 远端 track 发现后调用 `update_tracks`
- demux 输出 frame 后按 canonical timeline 写入 engine
- 连接关闭或错误时释放租约并按配置退避重试
- 目标 stream 被占用时该 job 停止并记录配置/运行时错误

## 多轨道策略

- 默认最多 32 条 elementary stream
- PID 分配：video 从 `0x0100` 起，audio 从 `0x0110` 起，data 从 `0x0120` 起
- stream_id 分配：video `0xE0..0xEF`，audio `0xC0..0xDF`
- PCR PID 优先首个视频轨；无视频时首个音频轨；无媒体轨时拒绝启动播放
- module 首版发布一个 program；demux 发现多个 program 时选择第一个包含支持 codec 的 program，并发出 diagnostic
- 多音轨/多视频轨全部放入同一 PMT，播放器是否选择由客户端能力决定

## 兼容与鲁棒性策略

- 输入端支持任意 chunk 边界，demux 保留最多 `max_reassembly_bytes`
- 前导垃圾、错位、缺 sync 时扫描下一组可信 `0x47`
- PAT/PMT CRC 错误默认 tolerant；strict 模式返回错误
- continuity 缺口默认发 diagnostic 并标记后续 frame `DISCONTINUITY`
- PES length 0 用下一 PUSI flush；连接结束时 flush 当前 PES
- 不认识的 stream_type 跳过，不影响其他 PID
- private stream 优先解析 registration descriptor
- Opus 同时接受 `0x06 + Opus descriptor` 与 SMS `0x9C`
- AV1/VP9/VP8 in TS 仅承诺 mux/demux 和转发稳定，不承诺所有客户端播放

## 配置草案

```yaml
modules:
  ts:
    enabled: true
    listen: "0.0.0.0:8082"
    write_queue_capacity: 256
    read_buffer_size: 65536
    subscriber_queue_capacity: 256
    bootstrap_max_frames: 150
    play_wait_source_timeout_ms: 15000
    max_tracks: 32
    strict_crc: false
    max_reassembly_bytes: 4194304
    pat_pmt_interval_ms: 500
    tls:
      enabled: false
      listen: "0.0.0.0:8444"
      cert_path: ""
      key_path: ""
      handshake_timeout_ms: 5000
    pull_jobs: []
```

配置变更结果：

- 改变 listen、TLS、queue、pull_jobs、strict_crc、buffer 上限：`ModuleRestartRequired`
- 只改变统计/诊断阈值时可后续扩展为 `Immediate`

## 安全边界

- 拒绝包含 `%`、`..`、空 app、空 stream、超长路径的请求
- WebSocket 必须校验 version 13 和 `Sec-WebSocket-Key`
- TLS 私钥/证书只在 driver 读取
- HTTP header、WS message、TS reassembly buffer 均有上限
- 慢客户端不能拖累同一 stream 的其他订阅者
