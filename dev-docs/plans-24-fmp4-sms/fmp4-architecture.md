# fMP4 总体架构设计

- **状态**: 规划中
- **范围**: 固定 fMP4 协议 crate 边界、共享 ISO BMFF/fMP4 容器 API、HTTP/WS 传输语义、播放/拉流数据流、多轨道与兼容策略。
- **完成标准**: 实现者能够据此拆出 `cheetah-fmp4-core + cheetah-fmp4-driver-tokio + cheetah-fmp4-module`，并将 fMP4 容器能力收敛到 `cheetah-codec`。

## 架构目标

fMP4 协议在本项目中分为两类能力：

1. **ISO BMFF/fMP4 容器能力**：box 解析/写入、init segment、media fragment、sample entry、`moof/mdat`、`tfhd/tfdt/trun`、时间戳和 sample payload 转换。这属于共享媒体基础层，应放入 `cheetah-codec`。
2. **HTTP/WS fMP4 直播协议能力**：URL 路由、HTTP/HTTPS 长连接、WebSocket/WSS binary、播放会话、远端拉流任务。这属于独立 `fmp4` 协议三段式 crate。

首版实现播放和拉流，不实现客户端推流。播放方向是 engine `AVFrame + TrackInfo` 到 fMP4 bytes；拉流方向是远端 fMP4 bytes 到 engine `AVFrame + TrackInfo`。

## Crate 与依赖方向

新增目录与 package：

```text
crates/protocols/fmp4/
  core/                    # cheetah-fmp4-core
  driver-tokio/            # cheetah-fmp4-driver-tokio
  module/                  # cheetah-fmp4-module
  testing/property-tests/  # cheetah-fmp4-property-tests
  fuzz/                    # standalone cargo-fuzz workspace
```

依赖方向固定为：

```text
cheetah-fmp4-module
  -> cheetah-fmp4-driver-tokio
  -> cheetah-fmp4-core
  -> cheetah-codec

cheetah-fmp4-module -> cheetah-sdk -> cheetah-codec
cheetah-fmp4-driver-tokio -> cheetah-runtime-api
```

禁止关系：

- `cheetah-fmp4-core` 不依赖 Tokio、SDK、engine、socket、HTTP 框架
- `cheetah-fmp4-module` 不直接依赖 `tokio::net`、`tokio::time`、`tokio::sync`
- `cheetah-codec` 不依赖 fMP4 module、HTTP、数据库、engine 或 runtime
- fMP4 module 不复制 HLS 私有 fMP4 mux/demux 逻辑

## 共享 fMP4 容器 API

`cheetah-codec` 新增或增强：

```rust
pub struct Fmp4Muxer { /* Sans-I/O */ }

pub struct Fmp4MuxerConfig {
    pub segment_mode: Fmp4SegmentMode,
    pub include_styp: bool,
    pub include_sidx: bool,
    pub fragment_trigger: Fmp4FragmentTrigger,
    pub max_tracks: usize,
}

pub enum Fmp4MuxEvent {
    InitSegment(Bytes),
    MediaSegment { data: Bytes, keyframe: bool },
    Diagnostic(Fmp4Diagnostic),
}

pub struct Fmp4Demuxer { /* Sans-I/O */ }

pub enum Fmp4DemuxEvent {
    TrackInfo(Vec<TrackInfo>),
    Frame(AVFrame),
    InitSegment(Bytes),
    Diagnostic(Fmp4Diagnostic),
}
```

设计要求：

- muxer 输入 `TrackInfo` 和 `AVFrame`，输出 init segment 与 media segment
- demuxer 输入任意切片 bytes，输出 `TrackInfo` 与 `AVFrame`
- H264/H265/H266 输出使用 4 字节 length-prefixed NALU，输入可还原为 canonical H26x frame
- AAC 输出使用 raw AAC sample，codec config 来自 ASC；输入 AAC sample 进入 engine 为 `AacRaw`
- G711/Opus/MP2/MP3/MJPEG/VP8/VP9/AV1 使用 codec 原始 access unit payload
- PTS/DTS 输出使用 track timescale，来源为 canonical timeline
- 输入端保留 source timestamp side data，进入 engine 时仍以 canonical timeline 为准

## HTTP / WebSocket 路由

播放路由：

```text
GET  /{app}/{stream}.mp4
HEAD /{app}/{stream}.mp4
OPTIONS /{app}/{stream}.mp4
WebSocket GET /{app}/{stream}.mp4
```

兼容别名：

```text
GET /{app}/{stream}.live.mp4
WebSocket GET /{app}/{stream}.live.mp4
```

规则：

- `{app}/{stream}` 映射到 `StreamKey::new(app, stream)`
- 路径必须以 `.mp4` 或 `.live.mp4` 结尾
- query 首版只保留扩展参数，不定义播放器模式
- HTTP 响应头：`Content-Type: video/mp4`、`Cache-Control: no-cache`、`Connection: keep-alive`、CORS
- 非 WebSocket HTTP 响应使用 chunked streaming，先发送 init segment，再发送 media segment
- WebSocket 成功 upgrade 后仅发送 binary message
- WebSocket binary message 必须是完整 init segment 或完整 media segment

拉流源 URL：

```text
http://host/{app}/{stream}.mp4
https://host/{app}/{stream}.mp4
ws://host/{app}/{stream}.mp4
wss://host/{app}/{stream}.mp4
```

## 播放输出数据流

```text
Engine StreamManager
  -> SubscriberSource<Arc<AVFrame>>
  -> Fmp4PlaySession
  -> cheetah-codec::Fmp4Muxer
  -> cheetah-fmp4-driver-tokio command channel
  -> HTTP body bytes or WebSocket binary messages
```

播放启动顺序：

1. 等待 stream snapshot，拿到 `TrackInfo`
2. 过滤支持的 audio/video track，超过 `max_tracks` 的 track 跳过并输出 diagnostic
3. 初始化多轨 fMP4 muxer
4. 发送 init segment
5. 有视频时等待关键帧起播，避免 delta frame 起播
6. 将 frame 写入 muxer，按关键帧或时间窗口输出 media segment
7. track 变化或 codec config 变化时重建 muxer，并在下个关键帧重新发送 init segment

## 远端拉流数据流

```text
HTTP(S)/WS(S)-fMP4 source
  -> cheetah-fmp4-driver-tokio pull client
  -> cheetah-codec::Fmp4Demuxer
  -> TrackInfo + AVFrame
  -> cheetah-fmp4-module pull job
  -> PublisherApi exclusive lease
  -> Engine StreamManager
```

拉流规则：

- pull job 启动时获取目标 `StreamKey` 独占发布租约
- 远端 init segment 解析出 track 后调用 `update_tracks`
- demux 输出 frame 后按 canonical timeline 写入 engine
- 连接关闭或错误时释放租约并按配置退避重试
- 目标 stream 被占用时该 job 停止并记录配置/运行时错误

## 多轨道策略

- 默认最多 32 条 track
- MP4 track id 直接来自 `TrackInfo.track_id`，若冲突则 muxer 内部稳定重映射并记录映射
- 所有支持的 audio/video track 写入同一个 `moov`
- 每个 media fragment 可包含多个 `traf`
- fragment 内样本按输入顺序写入 `mdat`，`trun.data_offset` 指向对应 track 样本区域
- module 首版发布一个 presentation；多视频/多音频 track 全部进入 engine tracks
- 播放器是否选择多音轨/多视频轨由客户端能力决定

## 配置草案

```yaml
modules:
  fmp4:
    enabled: true
    listen: "0.0.0.0:8083"
    write_queue_capacity: 256
    read_buffer_size: 65536
    subscriber_queue_capacity: 256
    subscriber_backpressure: DropUntilNextKeyframe
    bootstrap_max_frames: 150
    play_wait_source_timeout_ms: 15000
    max_tracks: 32
    max_box_bytes: 4194304
    max_fragment_duration_ms: 1000
    force_fragment_on_keyframe: true
    include_styp: true
    include_sidx: true
    demand_mode: false
    tls:
      enabled: false
      listen: "0.0.0.0:8445"
      cert_path: ""
      key_path: ""
      handshake_timeout_ms: 5000
    pull_jobs: []
```

配置变更结果：

- 改变 listen、TLS、queue、pull_jobs、fragment、box 上限：`ModuleRestartRequired`
- 只改变统计/诊断阈值时可后续扩展为 `Immediate`

## 安全边界

- 拒绝包含 `%`、`..`、空 app、空 stream、超长路径的请求
- WebSocket 必须校验 version 13 和 `Sec-WebSocket-Key`
- TLS 私钥/证书只在 driver 读取
- HTTP header、WS message、box reassembly buffer 均有上限
- 慢客户端不能拖累同一 stream 的其他订阅者
- `mdat`、box size、track count、sample count 都必须 bounded，不能因远端输入无界增长
