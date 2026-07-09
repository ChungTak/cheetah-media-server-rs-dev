# fMP4 ABL 对齐总体架构

- **状态**: 规划中
- **范围**: 固定对齐 ABL 后的 fMP4 能力归属、播放与拉流数据流、录像能力边界和兼容策略。
- **完成标准**: 后续实现者能清楚判断一个能力应该落在 `cheetah-codec`、`core`、`driver` 还是 `module`。

## 架构目标

ABL 的实现把 HTTP-MP4 播放、fMP4 录像、回放下载、时间戳修正和 I 帧快速起播揉在一起。Cheetah 不复制其类结构，只对齐外部行为，并继续保持清晰分层：

1. **ISO BMFF/fMP4 容器能力**：
   `ftyp/moov/styp/sidx/moof/mdat`、sample entry、H26x payload 变换、时间戳展开、重复 init/unknown box 兼容。这些属于 `cheetah-codec`。
2. **HTTP/WS fMP4 协议能力**：
   `.mp4` / `.live.mp4` 路由、HTTP chunked streaming、WebSocket binary、TLS、pull client。这些属于 `cheetah-fmp4-core + cheetah-fmp4-driver-tokio`。
3. **engine 接入与业务能力**：
   订阅播放、拉流发布、关键帧起播、bootstrap、pull supervisor、后续录像接入。这些属于 `cheetah-fmp4-module`。

## Crate 边界

```text
cheetah-fmp4-module
  -> cheetah-fmp4-driver-tokio
  -> cheetah-fmp4-core

cheetah-fmp4-module -> cheetah-sdk -> cheetah-codec
cheetah-fmp4-driver-tokio -> cheetah-runtime-api
cheetah-codec -> no runtime / no engine / no file I/O / no HTTP
```

约束：

- `cheetah-codec` 不依赖 engine、HTTP、Tokio、文件系统录像流程。
- `cheetah-fmp4-core` 保持 Sans-I/O，只做请求/响应/close reason/session 状态机。
- `cheetah-fmp4-driver-tokio` 独占 TCP/TLS、HTTP/1.1、chunked、WebSocket frame、pull socket I/O。
- `cheetah-fmp4-module` 独占 engine 订阅/发布、播放会话、pull job 生命周期和后续录像接入。

## 播放路由

继续支持两种 URL：

```text
GET /{app}/{stream}.mp4
GET /{app}/{stream}.live.mp4
HEAD /{app}/{stream}.mp4
OPTIONS /{app}/{stream}.mp4
WebSocket GET /{app}/{stream}.mp4
WebSocket GET /{app}/{stream}.live.mp4
```

说明：

- ABL 主要暴露 `.mp4`。
- 现有 Cheetah 已兼容 `.live.mp4`，不回退这条兼容路径。
- HTTP 和 WebSocket 共用同一播放语义：先 init，再持续 media fragment。

## HTTP/WS 数据流

```text
Engine StreamManager
  -> stream snapshot + SubscriberApi
  -> Fmp4PlaySession
  -> cheetah-codec::Fmp4Muxer
  -> Fmp4DriverCommand::SendData
  -> HTTP chunk / WebSocket binary
```

播放规则：

1. 拿到 stream snapshot 后筛选 audio/video track。
2. 初始化每连接独立 muxer，不跨连接共享可变状态。
3. 先发送 init segment。
4. 有视频时等待关键帧起播，对齐 ABL 的 `bWaitIFrameSuccessFlag` 语义。
5. 按关键帧、fragment 时长或样本数量上限 flush。
6. track/config 变化时 flush 当前 fragment，重建 muxer，重发 init。

## Pull 数据流

```text
HTTP(S)/WS(S)-fMP4 source
  -> driver pull client
  -> cheetah-codec::Fmp4Demuxer
  -> TrackInfo + AVFrame
  -> module pull job
  -> PublisherApi exclusive lease
  -> Engine StreamManager
```

拉流规则：

- pull job 启动前获取独占发布租约。
- 重复 init 或 track 变化时更新 tracks。
- frame 进入 engine 前统一转为 canonical `AVFrame + TrackInfo`。
- 连续错误、对端关闭或取消时释放 lease，再按 backoff 重试。

## 录像与回放边界

ABL 在 `NetServerHTTP_MP4` 和 `StreamRecordFMP4` 中同时实现直播与录像。Cheetah 不这样做。

后续录像阶段固定边界：

- **录像切片文件生成**：新建 recording 边界，依赖 `cheetah-codec::Fmp4Muxer`。
- **录像回放读取 / 合并下载**：新建 replay/read path，不能塞到 `cheetah-fmp4-core`。
- **录像 hook/config**：留在 module 或 control 面，不进入 core/driver。

## ABL 兼容策略

必须对齐：

- `.mp4` 路由。
- HTTP chunked 长连接输出。
- WebSocket binary 输出。
- 关键帧起播。
- H264/H265 参数集就绪后再发 init。
- 真实帧率驱动 fragment 时间戳，而不是固定 25fps 假设。
- 对弱标准 `alaw/ulaw`、`mp4v/jpeg`、`mp4a(0x69/0x6B)` 保持兼容。

不照搬：

- 不引入 ABL 的大对象继承树和私有 FIFO。
- 不把录像回放控制语义塞进直播模块。
- 不为了 ABL 录像需求污染 `cheetah-codec` 的纯容器边界。
