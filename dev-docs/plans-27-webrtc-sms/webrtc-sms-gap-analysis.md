# WebRTC 对标 SimpleMediaServer 缺口分析

- **状态**: 已完成（所有缺口已在 Phase 01-05 中补齐，实际进展见各 phase 文档）
- **范围**: 分析 `vendor-ref/simple-media-server/Src/Webrtc/` 与 `Api/WebrtcApi.cpp` 的能力边界，映射到本项目 `str0m 0.19.0 + core/driver/module + codec/engine` 架构
- **完成标准**: 实现者能明确哪些 SMS 行为需要兼容，哪些底层协议逻辑应交给 `str0m`，哪些能力必须由 Cheetah 自建

## SMS WebRTC 结构拆解

SMS 的 WebRTC 实现大致分为六组：

1. **HTTP API 层**
   - `Api/WebrtcApi.cpp`
   - 暴露 `/api/v1/rtc/play`、`/api/v1/rtc/publish`、`/api/v1/rtc/whep`、`/api/v1/rtc/whip`
   - 暴露 client pull/push 与 P2P 管理 API
   - 对 WHIP/WHEP 返回 `201`、`Content-Type: application/sdp`、`Location`
   - 对 SMS-style play/publish 返回 JSON，其中包含 `sdp`、`code`、`server`、`sessionid`

2. **服务端会话层**
   - `WebrtcContext.*`
   - 一个 context 绑定一个远端 SDP、ICE username、socket addr、DTLS/SRTP 状态、RTP/RTCP 处理、播放或发布角色
   - 内部持有 `_rtpCache` 用于 NACK 重传
   - 对播放场景订阅 `WebrtcMediaSource` ring 并发送 RTP
   - 对发布场景解包 RTP 并产生 frame

3. **单端口入口与会话路由**
   - `WebrtcServer.*`
   - `WebrtcContextManager.*`
   - 一个 UDP socket 接收 STUN/DTLS/RTP/RTCP 后按包类型和地址映射到 context
   - TCP server 使用 `WebrtcConnection` 接入 TCP WebRTC
   - 支持多 loop/multi-thread 绑定同一端口的风格

4. **媒体源与 RTP 打包**
   - `WebrtcMediaSource.*`
   - `WebrtcEncodeTrack.*`
   - `WebrtcDecodeTrack.*`
   - `WebrtcRtpPacket.*`
   - 以 ring buffer 分发 RTP 包，按 track 做 RTP encode/decode，并对 G711 做帧时长聚合

5. **协议基础组件**
   - `WebrtcStun.*`
   - `WebrtcIce.*`
   - `WebrtcDtlsSession.*`
   - `WebrtcSrtpSession.*`
   - `SctpAssociation.*`
   - 这些是 SMS 自研 STUN/ICE/DTLS/SRTP/SCTP/DataChannel 基础实现

6. **客户端与 P2P**
   - `WebrtcClient.*`
   - `WebrtcP2PClient.*`
   - `WebrtcP2PManager.*`
   - 负责作为 WebRTC 客户端拉流/推流，或建立 P2P 会话

## 本项目映射原则

不要按文件逐个移植 SMS。正确映射如下：

| SMS 组件 | Cheetah 对应实现 | 说明 |
|----------|------------------|------|
| `WebrtcApi` | `cheetah-webrtc-module` HTTP service | 保留 API 语义和字段兼容 |
| `WebrtcContext` | `cheetah-webrtc-core::WebRtcCoreSession` + module session | 协议状态在 core，业务状态在 module |
| `WebrtcContextManager` | `cheetah-webrtc-driver-tokio` session router | driver 负责单端口包分类和路由 |
| `WebrtcServer` | `cheetah-webrtc-driver-tokio` UDP/TCP listener | driver 负责 socket 和多线程 |
| `WebrtcMediaSource` | engine `StreamManager` + `BootstrapPolicy` | 不重复造 media source ring |
| `WebrtcEncodeTrack` | `cheetah-codec` WebRTC egress + `str0m` writer/RTP mode | packetizer 选择按 V1 策略固定 |
| `WebrtcDecodeTrack` | `cheetah-codec` WebRTC ingress + `str0m` media event/RTP mode | depacketize/归一化下沉 codec |
| `WebrtcRtpPacket` | `str0m` RTP/extension API + `cheetah-codec` 缺口补充 | 不重复写完整 RTP parser |
| `WebrtcRtcpPacket` | `str0m` RTCP events/stats + module metrics | RTCP 反馈优先从 `str0m` 读取 |
| `WebrtcStun/Ice/Dtls/Srtp/Sctp` | `str0m` | 不移植 SMS 自研协议基础组件 |
| `WebrtcClient` | module client job supervisor | HTTP client + `Rtc` offer/answer + engine bridge |
| `WebrtcP2PClient` | module P2P job supervisor | P2P 仍通过 core/driver 驱动 `Rtc` |

## API 兼容差距

### SMS-style play/publish

SMS play:

```text
POST /api/v1/rtc/play
body: appName, streamName, enableDtls, sdp
response: { "code": 200, "sdp": "..." }
```

SMS publish:

```text
POST /api/v1/rtc/publish
body: appName, streamName, sdp, preferVideoCodec?, preferAudioCodec?, enableDtls?
response: { "code": 0, "server": "sms", "sessionid": "sms", "sdp": "..." }
```

Cheetah 处理：

- 保留字段别名：`appName/app`、`streamName/stream`。
- `enableDtls` 只作为兼容输入；WebRTC 标准路径必须启用 DTLS/SRTP。
- `preferVideoCodec`、`preferAudioCodec` 映射到 codec negotiation policy，不做转码。
- response 中 `server` 固定为 `"cheetah"`，兼容模式可配置为 `"sms"`。
- `sessionid` 返回实际 WebRTC session id，不使用固定 `"sms"`。

### WHIP/WHEP

SMS 将 `/api/v1/rtc/whip` 映射为 publish，将 `/api/v1/rtc/whep` 映射为 play，并用请求 body 的 SDP 作为 offer。

Cheetah 处理：

- `POST /api/v1/rtc/whip?appName=live&streamName=demo` 或 JSON/URL alias 指定 stream。
- `POST /api/v1/rtc/whep?appName=live&streamName=demo`。
- 返回 `201 Created`、`Content-Type: application/sdp`、`Location: /api/v1/rtc/session/{session_id}`。
- `DELETE /api/v1/rtc/session/{session_id}` 释放 session。
- `PATCH /api/v1/rtc/session/{session_id}` 用于 trickle ICE / ICE restart，首版可先支持 full SDP offer/answer，再在 Phase 05 完整补 PATCH。

### Client pull/push/P2P

SMS client API：

```text
POST /api/v1/rtc/pull/start
POST /api/v1/rtc/pull/stop
POST /api/v1/rtc/pull/list
POST /api/v1/rtc/push/start
POST /api/v1/rtc/push/stop
POST /api/v1/rtc/push/list
POST /api/v1/rtc/p2p/add
POST /api/v1/rtc/p2p/remove
POST /api/v1/rtc/p2p/list
POST /api/v1/rtc/p2p/stop
```

Cheetah 处理：

- Phase 03 先提供 server play/publish 和 session stop。
- Phase 05 再提供 client pull/push/P2P。
- job 配置必须 bounded：`max_jobs`、`retry_backoff_ms`、`max_retry_backoff_ms`、`timeout_ms`。
- client pull 入站必须走 `publisher_api.acquire_publisher`，遵守单发布者租约。
- client push 出站必须走 `subscriber_api.subscribe`，遵守慢订阅者隔离。

## 协议与媒体差距

### SDP

SMS 自带 `WebrtcSdpParser`，能解析 `rtpmap`、`fmtp`、`rtcp-fb`、`extmap`、`mid`、`msid`、`ssrc`、`ssrc-group`、`ice-ufrag`、`ice-pwd`、`fingerprint`、`candidate`、`sctp-port`。

Cheetah 不复制 SDP parser：

- 常规 offer/answer 由 `str0m` 解析和生成。
- 需要保留 SMS fixture 做互操作测试：`Src/Webrtc/SdpExample/*.sdp`。
- 对无法被 `str0m` 接受的非标准 SDP，只做显式兼容预处理，且预处理必须集中在 `webrtc/module/src/compat.rs` 或 `webrtc/core/src/sdp_compat.rs`。

### RTP extension

SMS `RtpExtType` 覆盖：

- audio-level
- absolute-send-time
- TWCC / transport sequence number
- mid
- rtp-stream-id / repaired-rtp-stream-id
- video timing
- video orientation
- playout delay
- transmission offset
- frame marking
- AV1 dependency descriptor 等

Cheetah 策略：

- 首版必测：audio-level、abs-send-time、transport-wide-cc、mid、rid、repaired-rid、orientation。
- AV1 dependency descriptor、frame marking、video timing、playout delay 先作为可观测扩展，不作为业务决策硬依赖。
- extension id 以 SDP/extmap 或 `str0m` extension map 为准，禁止写死。

### RTP payload / codec

SMS 可以在 WebRTC 中处理 H264/H265/VP8/VP9/AV1/AAC/G711/Opus/MP3 等，具体依赖它自己的 RTP encoder/decoder。

Cheetah 策略：

- 浏览器 profile：默认允许 H264、VP8、VP9、AV1、Opus；H265/G711 仅在 peer SDP 明确支持时启用；AAC/MP3 默认不对浏览器 WebRTC 输出。
- 非浏览器/设备 profile：可启用 H265、G711、AAC、MP3 的 RTP passthrough 或 RTP mode，但必须写入配置并有互操作测试。
- 所有协议进入 engine 前必须变成 `AVFrame + TrackInfo`。
- 所有协议输出前优先通过 `cheetah-codec` 导出目标 payload view。

### GOP 秒开

SMS `WebrtcMediaSource` 使用 ring 分发 RTP 包，播放端接入时可以快速获得缓存。

Cheetah 策略：

- 不新增 WebRTC 私有 media source。
- 复用 engine ring buffer 和 `SubscriberOptions.bootstrap_policy`。
- WebRTC 播放订阅使用 `BootstrapPolicy::live_tail(max_frames, max_age)`，默认从最近 keyframe 开始。
- `cheetah-codec` 负责 H264/H265/H266 参数集补发、关键帧修复和 access-unit 边界。

### NACK/RTX/Jitter

SMS 使用 `_rtpCache` 维护 256 个 RTP 包用于重传，并有 NACK heartbeat。

Cheetah 策略：

- 发送侧使用 `str0m::rtp::StreamTx::set_rtx_cache` 或 `RtcConfig::set_send_buffer_video` 配置 bounded resend buffer。
- 接收侧使用 `str0m` NACK 生成和固定 reorder/depacketize buffer。
- 如果实测抗丢包能力不足，补自研 adaptive jitter buffer 只能进入 `cheetah-codec` 或明确 compat 层，不得散落 module。

## 风险清单

| 风险 | 影响 | 缓解 |
|------|------|------|
| `str0m` 单 ICE transport 限制 | 多 m-line SDP 兼容 | Phase 01 通过 fixture 验证 bundle-only 路径 |
| H265/AAC 浏览器兼容差 | 播放失败 | profile 化 codec policy，默认拒绝不兼容组合 |
| WebRTC over TCP 互操作复杂 | TCP candidate 建连失败 | Phase 02 先支持 passive TCP，Phase 05 补主动 TCP 和真实客户端 |
| 网络迁移不可预测 | 移动网络切换断流 | 通过 ICE restart/trickle + 5-tuple 更新测试，失败时快速重建 session |
| 自适应 jitter buffer 缺失 | 弱网体验不足 | Phase 04 建立丢包/乱序基线，再决定是否在 codec 层增强 |
| BWE 不等于码率控制 | 码率不随网络变化 | module 读取 BWE 后明确实现降层、限速、丢帧策略 |
| DataChannel buffer 无界 | 内存风险 | 所有 DataChannel 队列配置 capacity，超限关闭或丢弃低优先级消息 |

## 验收矩阵

| 场景 | 必须验证 |
|------|----------|
| Chrome WHIP publish | SDP answer、ICE connected、engine 有流 |
| Chrome WHEP play | 其他协议源转 WebRTC，首帧从 keyframe 开始 |
| SMS-style publish/play | JSON API 与旧字段兼容 |
| RTP/RTSP/RTMP 转 WebRTC | H264+AAC/H264+Opus/H265+G711 按 profile 处理 |
| WebRTC 转 RTSP/RTMP/HLS/fMP4 | WebRTC 入站归一化后其他协议可播放 |
| Simulcast publish | RID 识别、层选择、指标可见 |
| 丢包 NACK/RTX | 可触发 NACK、可重传、cache bounded |
| TWCC/BWE | stats 可见，发送策略发生预期变化 |
| DataChannel echo | text/binary echo 正常，超限行为可测 |
| Client pull/push/P2P | job 生命周期、重试、停止、列表 |

