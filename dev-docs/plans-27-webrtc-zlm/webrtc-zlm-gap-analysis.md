# WebRTC 与 ZLMediaKit 差距分析

## ZLMediaKit 关键行为

ZLMediaKit WebRTC 代码主体在 `vendor-ref/ZLMediaKit/webrtc/`，不是 `src/`。`src/` 目录仍然重要，但主要提供 RTP/RTCP、HTTP、RTSP、codec、媒体源、API glue 等共享行为。

核心参考：

- `WebRtcTransport.*`：统一 WebRTC transport、SDP 协商、ICE、DTLS、SRTP、SCTP、RTP/RTCP、NACK、TWCC、timeout、统计。
- `WebRtcPusher.*`：WebRTC 推流，支持 simulcast 多 RID 源、断续推、loss rate 查询。
- `WebRtcPlayer.*`：WebRTC 播放，支持配置帧预发、H264 B 帧过滤、本地源订阅。
- `WebRtcEchoTest.*`：media echo，RTCP echo，answer `msid` 兼容。
- `WebRtcClient.*`：WHIP/WHEP 与 WebSocket P2P client，支持 URL 解析、candidate、bye、check-in/out。
- `WebRtcSignaling*`：P2P 房间与 WebSocket 自定义信令。
- `Nack.*`：接收侧 NACK 生成和发送侧 RTP cache。
- `TwccContext.*`：TWCC feedback 构造触发策略。
- `RtpExt.*`：RTP header extension 解析、RID fallback、extmap ID 转换。
- `Sdp.*`：大量浏览器、Janus、ZLMRTCClient 兼容 SDP 行为。

## 当前本地状态

本地已有：

- `cheetah-webrtc-core`：基于 `str0m` 的 Sans-I/O core，已有 offer/answer、candidate、network packet、timer、media event、BWE、DataChannel、PLI/FIR、stats 基础。
- `cheetah-webrtc-driver-tokio`：已有 UDP 单端口 listener、地址路由、基础迁移 hook、bounded channel；`listen_tcp` 仍未真正绑定。
- `cheetah-webrtc-module`：已有 WHIP/WHEP、SMS-style play/publish、session API、client pull/push job 雏形、P2P add/remove/list、DataChannel echo 基础、engine bridge。
- `cheetah-codec`：已有 WebRTC future protocol contract、RTP 基础、SDP 导出、参数集和时间戳能力。
- 测试：有 core/module/driver 基础测试、SDP property tests、SDP fuzz。

主要风险是现有能力偏“基础可用”，还缺 ZLM 级别的兼容矩阵、弱网算法观测、TCP 部署、P2P 信令互操作和真实客户端 fixture。

## 必须补齐的实现缺口

### SDP 与协商

- 补充 ZLM fixtures：`offer.sdp`、`offer-simulcast.sdp`、`janus_offer.sdp`、`janus_answer.sdp`、ZLMRTCClient 生成的 browser SDP。
- 兼容 `extmap-allow-mixed`、缺失 `a=rtcp-rsize`、candidate 行顺序变化、unknown rtcp-fb、plan-b 风格 `a=ssrc`。
- 明确 codec profile：browser、rtsp-compatible、surveillance、datachannel-only。
- H265、AV1、VP9、G711、AAC 的协商策略必须和 RTSP 输出矩阵一致；不做转码时给出明确错误。

### RTP extension

- 当前 core 主要消费 `str0m` 事件，需要补充本地可观测的 RTP extension mapping。
- 必须覆盖 ZLM `RTP_EXT_MAP` 中的常用项：audio-level、abs-send-time、transport-cc、mid、rid、repaired-rid、video-timing、orientation、playout-delay、framemarking、AV1 dependency descriptor。
- RID 解析失败时用 SSRC/RID map fallback；SDP munging 只有 SIM group 时生成稳定 RID。

### Driver 传输

- ~~`listen_tcp` 目前只被配置解析，runner 未绑定 TCP listener。~~（已落地：`spawn_driver` 当 `listen_tcp` 配置存在时绑定 `TcpListener`，inbound 走 RFC 4571 解码后进入与 UDP 共用的 `route_unbound_packet`，outbound 通过 `TcpWriterRegistry` 优先使用 TCP 通道。）
- 单 driver task 不适合高并发，需要 shard：新 session 选择 shard，route table 按 session 固定归属，unbound STUN 先解析 ufrag 再投递。
- 连接迁移需要从“观察到新地址”升级为完整 route lifecycle：active、stale、expired、candidate pair diagnostic。
- ~~UDP/TCP 同端口部署需要清楚的 candidate 和 socket 生命周期。~~（部分落地：listen 端口已经可与 UDP 共享，candidate 发布 / 生命周期诊断 `TcpAccepted` / `TcpClosed` 已上线，`TcpType=passive` 形式的 SDP candidate 由 str0m 协商；TCP keepalive 与超时关连接策略仍在 follow-up。）

### 推流与播放

- WebRTC 推流需要明确 simulcast 入 engine 策略：默认选一路，配置开启多 RID 子流。
- WebRTC 播放需要专门验证 GOP 秒开：tracks、codec config、关键帧、PLI 请求、首帧耗时。
- ZLM 的 H264 B 帧过滤不能直接放在 module，需由 `cheetah-codec` 提供检测或 egress policy。
- 推流断续重连必须遵守单发布者租约；不允许 module 私下替换 publisher 绕过 lifecycle。

### NACK / RTX / Jitter

- ZLM NACK 有明确窗口：最大保留个数、最大保留时间、最大重发次数、RTT 倍数节流、音视频不同 RTP 数量。
- 本地需要测试 `str0m` NACK/RTX 的行为边界，并在配置中暴露 bounded cache 和诊断。
- 对乱序、回绕、burst loss、RTX loss、late packet、duplicated packet 必须有 property/integration 测试。
- 自适应 jitter buffer 如不由 `str0m` 覆盖，需要在 codec/driver 边界明确最小实现：reorder window、deadline、drop policy、metrics。

### TWCC / BWE / RTCP

- ZLM TWCC 以最多 20 个 ext seq 或 256ms 触发 feedback，本地需要配置化并测试。
- BWE 不能只暴露事件；播放侧需要按估计码率执行降层、限速或 delta frame drop。
- REMB、PLI、FIR、SR、RR、BYE、SDES 应进入统一事件和指标；未知 RTCP 只诊断不崩溃。

### DataChannel

- 现有 echo 基础不足以覆盖生产兼容。
- 需要 PPID、stream id、binary/text、ordered/unordered、最大消息长度、关闭、背压、队列丢弃测试。
- DataChannel-only session 不应要求 media track。

### Client / P2P

- ZLM client 支持 `webrtc://` URL、`signaling_protocols=0|1`、WHIP/WHEP 和 WebSocket P2P。
- 本地需要把 pull/push job 从 HTTP 信令栈扩展到 P2P 房间信令，包含 check-in、candidate、bye、重连。
- P2P 模式仍不能绕过资源上界、SSRF 防护和鉴权。

## 编码矩阵

| Codec | WebRTC publish | WebRTC play | 转 RTSP 一致性 | 策略 |
|-------|----------------|-------------|----------------|------|
| H264 | 必须支持 | 必须支持 | 必须一致 | packetization-mode、SPS/PPS、B 帧策略、STAP-A/single NALU |
| H265 | 应支持 | 应支持 | 必须一致 | 浏览器兼容按 profile 控制，非浏览器优先 |
| Opus | 必须支持 | 必须支持 | RTSP 视目标能力 | WebRTC browser 默认音频 |
| G711A/U | 应支持 | 应支持 | 必须一致 | 监控设备和 RTSP 互转优先 |
| AAC | 入站可接收 | 浏览器默认禁用 | RTSP/FLV/HLS 一致 | 不转码，browser profile 拒绝或降级 |
| VP8/VP9 | 应支持 | 应支持 | RTSP 视目标能力 | 浏览器互操作 |
| AV1 | 可选支持 | 可选支持 | fMP4/HLS 视目标能力 | dependency descriptor 进入扩展观测 |

## 互操作风险

- Chrome、Firefox、Safari 对 H265、AAC、simulcast、DataChannel max message 和 TCP candidate 支持差异大。
- Janus/Pion/GStreamer 对 WHIP/WHEP PATCH、Location、Bearer 和 trickle ICE 行为不同。
- ZLMRTCClient 可能通过 SDP munging 提供非标准字段，必须 fixture 化。
- WebRTC over TCP 在 NAT、代理和防火墙环境下容易出现连接半开和长时间无包，需要 timeout 和 keepalive。
- P2P 自定义信令不是标准 WHIP/WHEP，必须单独标记兼容范围。

