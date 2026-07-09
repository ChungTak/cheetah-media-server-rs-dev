# SRT 总体设计

- **状态**: 已完成并随实现同步更新
- **范围**: SRT 协议接入的架构边界、数据流、Stream ID 语义、配置模型、跨协议转换方式
- **完成标准**: 后续实现者可以据此新增 `cheetah-srt-*` crate，而不需要重新决定分层、接口或媒体转换路径

---

## 设计目标

SRT module 的定位是低延迟公网传输入口和出口，而不是一个独立媒体内核。所有 SRT 输入进入 Cheetah 后必须收敛到统一的 `AVFrame + TrackInfo`，再由 RTSP、RTMP、HLS、WebRTC、fMP4 等 module 按既有订阅模型输出。

反向输出同理：SRT egress 不直接从其他协议模块拿私有包，而是订阅引擎中的 canonical media frame，封装为 MPEG-TS，再交给 SRT driver 发送。

---

## 分层边界

### `cheetah-srt-core`

职责：

- 封装 `shiguredo_srt` 所需的 runtime-neutral 输入/输出模型。
- 定义 SRT URL、Stream ID、role、mode、payload type、错误类型。
- 定义 driver 与 module 之间的命令和事件结构。
- 提供配置校验的纯函数。

禁止：

- 不依赖 Tokio。
- 不持有 UDP socket。
- 不启动任务。
- 不直接访问 `EngineContext`、`PublisherApi`、`SubscriberApi`。
- 不实现 MPEG-TS demux/mux。

### `cheetah-srt-driver-tokio`

职责：

- UDP bind/connect、send/recv loop。
- 驱动 `shiguredo_srt::SrtConnection`。
- 把 `ConnectionOutput::SendPacket` 写入 UDP。
- 把 `ConnectionOutput::SetTimer/ClearTimer` 映射到 Tokio timer。
- 管理 listener connection map、caller connection、send queue 和背压。
- 上报 connection lifecycle、payload、stats、diagnostics。

Tokio 类型只允许出现在 driver crate 内部，不进入 SDK、module 公共接口。

### `cheetah-srt-module`

职责：

- 提供 `SrtModuleFactory`、`SrtModuleConfig`、schema 注册。
- 管理 listener、ingress job、egress job、relay job 生命周期。
- 基于 Stream ID / URL / auth 配置决定 publish 或 request/play。
- 摄入侧获取发布租约并调用 `PublisherSink`。
- 输出侧订阅本地 stream 并把 frame 交给 MPEG-TS muxer。
- 维护 metrics、health、event bus、任务取消和重启语义。

module 不直接依赖 `tokio::net` 或 `tokio::time`；需要运行 driver 时通过 driver crate 暴露的 async 函数和 runtime-neutral command/event 类型交互。

---

## 数据流

### SRT 摄入到其他协议

```text
OBS/FFmpeg/libsrt
  -> SRT UDP packet
  -> cheetah-srt-driver-tokio
  -> shiguredo_srt::SrtConnection
  -> SrtDriverEvent::Payload
  -> cheetah-srt-module ingest session
  -> cheetah-codec::MpegTsDemuxer
  -> TrackInfo + AVFrame
  -> PublisherSink
  -> engine stream
  -> RTSP/RTMP/HLS/WebRTC subscribers
```

### 本地流输出到 SRT

```text
engine stream
  -> SubscriberSource
  -> cheetah-codec::MpegTsMuxer
  -> SrtDriverCommand::SendPayload
  -> shiguredo_srt::SrtConnection
  -> UDP packet
  -> remote SRT listener/caller peer
```

### Relay

```text
remote SRT source
  -> local target StreamKey
  -> engine
  -> remote SRT target
```

Relay 不走内存包直通，默认经过引擎统一媒体模型。后续如要增加 SRT->SRT TS packet 直通，必须显式标注“不可跨协议转换”，并与 RTMP direct proxy 一样隔离。

---

## Stream ID 语义

优先兼容 SRT Access Control 格式：

```text
#!::r=live/test,m=publish,u=user
#!::r=live/test,m=request
#!::r=live/test,m=play
```

字段含义：

| 字段 | 含义 | 默认 |
|------|------|------|
| `r` | 资源名，映射为 `StreamKey` | 无默认；缺失时拒绝，除非 job 显式配置 |
| `m` | 模式：`publish`、`request`、`play` | listener ingress 默认为 `publish` |
| `u` | 用户名或账号标识 | 空 |
| `h` | host hint | 空 |
| `s` | session hint | 空 |

兼容格式：

- `live/test`：bare stream key，按 listener 默认模式解释。
- `/live/test`：去掉前导 `/` 后映射。
- URL query `streamid=`：优先级高于 path 推导。

拒绝策略：

- `StreamKey` 为空。
- `StreamKey` 包含 `..`、控制字符或不可接受的分隔符。
- `m` 不是已知值。
- 鉴权开启时，用户、token 或 passphrase 不匹配。

---

## 配置模型

建议新增：

```rust
pub struct SrtModuleConfig {
    pub enabled: bool,
    pub listen: String,
    pub max_connections: usize,
    pub idle_timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub latency_ms: u64,
    pub stats_interval_ms: u64,
    pub payload: SrtPayloadConfig,
    pub encryption: SrtEncryptionConfig,
    pub auth: SrtAuthConfig,
    pub ingress: SrtIngressConfig,
    pub egress: SrtEgressConfig,
    pub ingress_jobs: Vec<SrtIngressJobConfig>,
    pub egress_jobs: Vec<SrtEgressJobConfig>,
    pub relay_jobs: Vec<SrtRelayJobConfig>,
}
```

默认值：

- `enabled = true`
- `listen = "0.0.0.0:9000"`
- `payload.kind = "mpegts"`
- `latency_ms = 120`
- `max_connections = 1024`
- `idle_timeout_ms = 30000`
- `connect_timeout_ms = 5000`
- `stats_interval_ms = 5000`
- `encryption.enabled = false`
- `ingress.publish_keepalive_ms = 0`
- `egress.subscriber_queue_capacity = 256`
- `egress.backpressure = DropUntilNextKeyframe`

配置应用语义：

- listen、encryption、max_connections、payload kind 改变返回 `ModuleRestartRequired`。
- job 列表改变可先采用 `ModuleRestartRequired`，后续再做热更新。
- 运行中 restart 仍由基础层执行 `create -> init -> start`，module 不维护私有重启流程。

---

## 媒体格式

v1 只支持：

- `MPEG-TS over SRT`
- 视频：以 `cheetah-codec::MpegTsDemuxer` 当前支持为准，例如 H264、H265、H266/VVC、AV1、VP8、VP9、MJPEG。
- 音频：AAC、G711A、G711U、Opus、MP3、MP2、ADPCM 等 `cheetah-codec` 已支持格式。

当前 codec 适配边界：

- SRT 不内置音视频转码器；支持范围是 MPEG-TS 中可识别、可封装、可按下游协议能力透传的 codec。
- H266/VVC egress 会注入 AUD 并在关键帧前补发 VPS/SPS/PPS。
- MJPEG/ADPCM 通过私有 registration descriptor 在 MPEG-TS PMT 中标识，保证 Cheetah 自身 mux/demux roundtrip 可识别。
- MPEG audio 会从 PES 首帧头进一步区分 MP2/MP3，并推导 sample rate 与 channel count，避免 FFmpeg `libmp3lame` 输出被误标为 MP2。
- WebRTC 浏览器输出仍受浏览器 codec 能力限制；无转码能力时 AAC/MP3 等音频不会自动变为 Opus。

不支持：

- 裸 H264/H265 over SRT。
- 任意 data payload 转发。
- SRT message payload 到 WebRTC DataChannel 的自动桥接。

---

## 兼容策略

入口兼容：

- 兼容 FFmpeg/OBS/libsrt 常见 URL query：`mode=caller`、`streamid=`、`latency=`、`passphrase=`、`pbkeylen=`。
- streamid 中 `#!::` 前缀和 URL percent-encoding 都要处理。
- 对缺少 Stream ID 的 caller，只有配置了 `default_publish_stream_key` 时才允许进入。

内部规范化：

- 所有 stream key 进入 module 前转为 `cheetah_sdk::StreamKey`。
- 所有媒体时间戳通过 `cheetah-codec` 归一化。
- SRT 连接统计转换为 module metrics，不混入媒体 frame metadata。

出口稳定：

- SRT egress 固定输出 MPEG-TS。
- 当订阅源无视频关键帧时，按 `start_from_keyframe` 配置等待或超时失败。
- 慢连接按配置丢弃到下一个关键帧或断开，不阻塞其他订阅者。

---

## 风险

1. `shiguredo_srt` 当前版本较新且处于 canary，API 可能变化；实现时应集中封装在 core/driver 边界，避免泄漏到 module 大范围代码。
2. SRT 是 UDP 多连接协议，listener 需要严格限制 connection map、buffer、timer 和 send queue 上界。
3. 跨协议成败主要取决于 MPEG-TS demux/mux 和时间戳处理，不能在 SRT module 侧复制私有修正逻辑。
4. 加密和 Stream ID 同时出现时，错误原因要清晰区分：握手失败、passphrase 错误、鉴权拒绝、StreamKey 无效。
