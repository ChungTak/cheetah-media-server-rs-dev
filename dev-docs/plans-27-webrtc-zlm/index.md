# WebRTC 协议完善设计与渐进式开发计划（对标 ZLMediaKit）

- **状态**: **Phase 01 完成**（ZLM/浏览器 SDP fixtures、`a=ssrc-group:SIM` 无 RID 自动注入、RTP extension 全矩阵模型、`RtpExtensionObserved` core 事件、`SdpCompatReport` 扩展、H264/H265 WebRTC egress contract 测试矩阵 14 条全部落地）；Phase 02 部分完成（RFC 4571 TCP framing、idle/handshake timeout、`RouteExpired` 诊断 + `compact_expired`、TCP keepalive 探针 `SO_KEEPALIVE`、`MigrationRejected` 诊断 + `try_bind_migration` 硬容量 cap、`Backpressure` 事件 + events 通道监控、`WebRtcIceTransportPolicy` 枚举 + 配置 wire 字段已落地；多线程 shard / candidate gathering 实际过滤 / packets 通道 backpressure 后续）；**Phase 03 完成**（ZLM URL 解析、play_bootstrap 度量、echo `msid` rewrite、`POST /zlm` 统一路由、H264 B 帧 flag、MultiStream 多 lease 路由 + module event worker 集成全部落地）；Phase 04 部分完成（telemetry 聚合、`SimulcastPolicy::Adaptive` + BWE 层切换 + 升层 PLI、`SimulcastPolicy::MultiStream` + `derive_multistream_key` + module 集成、NACK storm 边界测试矩阵、TWCC feedback counter 测试矩阵已落地；jitter 矩阵后续）；Phase 05 部分完成（property tests + fuzz harness 7 个上线、DataChannel 上界 + close-after-write 诊断、互操作 ignored 测试矩阵扩展到 9 条已落地；P2P 信令 / 互操作实体后续）。
- **目标**: 在现有 `cheetah-webrtc-core`、`cheetah-webrtc-driver-tokio`、`cheetah-webrtc-module` 基础上，对标 ZLMediaKit 的真实工程行为，补齐 WebRTC 推流、播放、协议互转、echo、simulcast、RTX/NACK、TWCC、DataChannel、WebRTC over TCP、WHIP/WHEP、client 与 P2P 能力。
- **方法**: 以 `str0m` 继续承担 WebRTC Sans-I/O 协议状态机；参考 `vendor-ref/ZLMediaKit/webrtc/` 的会话、信令、弱网、RTP extension、DataChannel、client/P2P 行为，并参考 `vendor-ref/ZLMediaKit/src/` 的 RTP/RTCP、HTTP、RTSP、codec 与媒体桥接实践。
- **完成标准**: 标准 WebRTC 路径可用，ZLMediaKit 风格非标准兼容落地，WebRTC 与 RTSP/RTMP/RTP/GB28181/HTTP-FLV/HLS/fMP4 互转通过单元、集成、弱网、互操作和 fuzz 测试。

---

## V1 完善范围

本轮是对现有 WebRTC 实现的协议完善，不重写项目总架构。

1. WebRTC 推流：浏览器、ZLMRTCClient、Pion/GStreamer 等客户端通过 WHIP、ZLM-style API 或 P2P 信令发布媒体，进入 engine 后统一为 `AVFrame + TrackInfo`。
2. WebRTC 播放：已有 RTSP、RTMP、RTP、GB28181、HTTP-FLV、HLS/fMP4 源可通过 WebRTC 输出。
3. 协议互转：WebRTC 入站流可转其他协议，其他协议入站流可转 WebRTC。
4. 双向 echo test：支持 media loopback 与 DataChannel echo，兼容 Chrome 对 echo answer `msid` 的处理。
5. Simulcast 推流：支持 RID、repaired RID、SSRC group SIM、Firefox 预绑定 SSRC/RID 和 SDP munging 兼容。
6. 上下行 RTX/NACK：支持发送侧 bounded RTP/RTX cache，接收侧 NACK 生成、重发节流、序号回绕和音视频不同窗口。
7. TWCC/BWE：解析 transport-wide-cc RTP extension，生成 TWCC RTCP，消费 str0m BWE/REMB 估计并驱动发送策略。
8. RTCP：支持 NACK、TWCC、REMB、PLI、FIR、SR、RR、SDES、BYE，并输出统计和诊断。
9. RTP extension：解析 audio-level、abs-send-time、transport-wide-cc、mid、rid、repaired-rid、video-orientation、video-timing、playout-delay、framemarking、AV1 dependency descriptor。
10. GOP 秒开：WebRTC 播放复用 engine bootstrap 和 `cheetah-codec` 参数集补发，首包优先送配置帧与关键帧。
11. DataChannel：支持建立、收发、echo、最大消息长度、PPID/stream id 兼容和背压上界。
12. WebRTC over TCP：支持与 UDP 同端口的 TCP listener、TCP framing、preferred TCP candidate 和连接迁移。
13. WHIP/WHEP：支持 `POST/PATCH/DELETE` 完整生命周期、trickle ICE、Location、Bearer/secret 鉴权和 ZLM URL 兼容。
14. ICE full：默认 `ice_lite=false`，支持作为服务端 answerer、客户端 offerer/answerer、P2P peer。
15. WebRTC client pull/push：支持 WHIP/WHEP 模式和 ZLM WebSocket P2P 模式的拉流、推流、状态查询、停止和重连。

本轮不做：

1. 不内置 TURN server；TURN/STUN 地址和 credential 只作为配置注入。
2. 不内置转码；目标协议不支持的编码返回明确诊断或按配置丢弃轨道。
3. 不复制 ZLMediaKit 的 STUN/ICE/DTLS/SRTP/SCTP 自研实现；协议状态仍由 `str0m` 承担。
4. 不在 module 中复制时间戳修正、NALU 参数集缓存或媒体格式转换逻辑。
5. 不绕过 `RuntimeApi` 在 SDK、engine 或 module 公共接口暴露 `tokio::*` 类型。

---

## ZLMediaKit 关键参考

| 领域 | ZLMediaKit 文件 | 重点行为 |
|------|-----------------|----------|
| WebRTC transport | `webrtc/WebRtcTransport.*` | SDP 协商、ICE/DTLS/SRTP/SCTP 编排、RTP/RTCP 收发、TCP 优先、统计、超时 |
| SDP | `webrtc/Sdp.*`、`offer.sdp`、`offer-simulcast.sdp`、`janus_*.sdp` | codec、extmap、RID、SSRC、RTX、WHIP/WHEP/P2P 兼容 |
| 推流 | `webrtc/WebRtcPusher.*` | WebRTC 入站转本地源、simulcast 多 RID 源、断续推兼容 |
| 播放 | `webrtc/WebRtcPlayer.*` | 本地源转 WebRTC、配置帧预发、H264 B 帧过滤、GOP 启动 |
| Echo | `webrtc/WebRtcEchoTest.*` | media echo、RTCP echo、Chrome `msid` 兼容 |
| Client / P2P | `webrtc/WebRtcClient.*`、`WebRtcSignaling*` | WHIP/WHEP client、WebSocket 自定义信令、房间保持 |
| NACK / RTX | `webrtc/Nack.*` | 丢包状态保留、重发节流、回绕测试、发送侧 RTP cache |
| TWCC | `webrtc/TwccContext.*` | 20 包或 256ms 触发 TWCC feedback |
| RTP extension | `webrtc/RtpExt.*` | extmap ID 映射、RID fallback、repaired RID、orientation、framemarking |
| RTCP | `src/Rtcp/*` | NACK、TWCC、REMB、SR、RR、XR、SDES、BYE 解析与构造 |
| API | `api/include/mk_webrtc.h`、`api/source/mk_common.cpp` | answer SDP、server start、DataChannel、room keeper、proxy player info |
| Web 客户端 | `www/webrtc/ZLMRTCClient.js`、`www/webrtc/index.html` | 浏览器兼容、SDP munging、老浏览器 extmap 兼容 |

---

## 与本地实现对比后的主要缺口

| 能力 | ZLM 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| WebRTC over TCP | `IceSession`、`mk_rtc_server_start` | RFC 4571 framing、listener、idle timeout 已落地；TCP keepalive 探针、迁移 fallback 与 multi-shard 后续 | Phase 02 follow-up |
| 多线程分片 | UDP server 按会话 poller 分发 | 单 driver task | Phase 02 |
| 完整连接迁移 | ICE pair 更新、socket/pair 管理 | 基础地址迁移 hook | Phase 02 |
| SDP 非标准兼容 | `Sdp.*`、ZLM/Janus fixtures | ZLM offer/simulcast/Janus 已 fixture 化并通过集成测试，`a=ssrc-group:SIM` 无 RID 自动注入 `r0/r1/r2` 已落地，浏览器 SDP munging follow-up | Phase 01 |
| RTP extension 全矩阵 | `RtpExt.*` | `RtpExtensionType` 全矩阵枚举 + `extract_rtp_extension_mappings` + `RtpExtensionObserved` core 事件已落地 | Phase 01 完成 |
| Simulcast SSRC/RID 兼容 | `WebRtcTransportImp::onStartWebRTC` | core 已暴露 `SimulcastLayerObserved` 与 `WebRtcSimulcastRidSource` 全 fallback 枚举；`SimulcastPolicy::Adaptive` 已加入 module 配置；BWE-driven 实时层切换 + 升层 PLI 请求已落地 | Phase 04 完成 |
| NACK 算法测试 | `Nack.*`、`test_rtcp_nack.cpp` | 使用 str0m 统计，缺少弱网矩阵 | Phase 04 |
| TWCC feedback 策略 | `TwccContext.*` | BWE 事件已通过 `WebRtcSessionTelemetry` 累积，TWCC 触发策略闭环（20 包 / 256 ms）后续小步迭代 | Phase 04 follow-up |
| REMB/PLI/SR/RR 观测 | `WebRtcTransportImp::onRtcp` | PLI/FIR 路由已有，REMB 写入 telemetry，RTT / loss / NACK / RTX 累积齐备；SR/RR 子类型细分留作 follow-up | Phase 04 follow-up |
| GOP 秒开 | `WebRtcPlayer::sendConfigFrames` | 复用 engine bootstrap，`WebRtcPlayBootstrapStats` 与 `/session/{id}.play_bootstrap` 已暴露 first packet/keyframe/decodable 时延 | Phase 03 follow-up |
| H264 B 帧过滤 | `H264BFrameFilter` | module 侧 `h264_bframe_filter` feature flag 已落地（默认 false），codec 侧实现待接入 | Phase 03 follow-up |
| DataChannel 兼容 | `SctpAssociation.*`、C API | 基础 echo + `max_data_channel_message_bytes` 上界已有，PPID / ordered / 关闭后再写 / 每通道队列上界后续 | Phase 05 follow-up |
| Client pull/push | `WebRtcClient.*` | WHIP/WHEP job 基础已存在 | Phase 05 |
| P2P 房间信令 | `WebRtcSignaling*` | 有 add/remove/list 管理雏形 | Phase 05 |
| fuzz / corpus | ZLM fixtures、浏览器 SDP | SDP / ZLM URL / TCP framing / trickle-ICE candidate 四个 fuzz harness 上线，RTP/RTCP/ICE-attribute 仍缺 corpus | Phase 05 follow-up |

---

## 标准与非标准兼容点

### 标准基线

- WebRTC JSEP / SDP offer-answer，BUNDLE，rtcp-mux，trickle ICE。
- ICE full，DTLS-SRTP，SRTP/SRTCP，RTP/RTCP，SCTP DataChannel。
- WHIP/WHEP HTTP 信令，`application/sdp`，session `Location`，PATCH trickle ICE，DELETE 关闭。
- RTP/RTCP 支持 NACK、PLI、FIR、SR、RR、REMB、TWCC。
- 媒体统一为 `AVFrame + TrackInfo`，时间戳和参数集归一化在 `cheetah-codec`。

### ZLM / 真实落地兼容优先

- 兼容 ZLM `rtc://` / `webrtc://` URL 参数：`signaling_protocols`、`peer_room_id`、`vhost/app/stream`。
- 兼容 ZLM-style `echo`、`play`、`push` SDP exchange 行为。
- 兼容 SDP munging 没有 RID 但存在 `a=ssrc-group:SIM` 的 simulcast，按 SSRC 顺序生成稳定 RID。
- 兼容 Firefox 通过 SDP 提前给出 SSRC/RID 的 simulcast。
- 兼容 Chrome echo answer 里本地与远端 `msid` 相同时忽略 track 的问题，echo answer 改写 `msid`。
- 兼容老浏览器 H264 STAP-A / single NALU packetization 差异，由 codec 层统一导出。
- 兼容收到未知 RTCP、损坏 RTCP、乱序 RTP、RTX 缺失关联 SSRC、缺少配置帧等脏输入时输出诊断而不是 panic。
- 兼容 TCP 优先 candidate、UDP/TCP 同端口部署、客户端网络地址迁移。

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [webrtc-zlm-architecture.md](webrtc-zlm-architecture.md) | 草案 | 总体架构、crate 边界、数据流、配置、API、观测 |
| [webrtc-zlm-gap-analysis.md](webrtc-zlm-gap-analysis.md) | 草案 | ZLM 行为拆解、本地现状、实现缺口、风险 |
| [phase-01-core-codec-sdp-rtp.md](phase-01-core-codec-sdp-rtp.md) | **完成** | core、codec、SDP、RTP extension、编码矩阵 |
| [phase-02-driver-ice-single-port-tcp-migration.md](phase-02-driver-ice-single-port-tcp-migration.md) | 部分完成 | 单端口 UDP/TCP、多线程、ICE、迁移、backpressure |
| [phase-03-module-whip-whep-publish-play-bridge.md](phase-03-module-whip-whep-publish-play-bridge.md) | **完成** | WHIP/WHEP、ZLM API、推流、播放、GOP 秒开、协议互转 |
| [phase-04-rtcp-loss-simulcast-bwe-jitter.md](phase-04-rtcp-loss-simulcast-bwe-jitter.md) | 部分完成 | simulcast、RTX/NACK、TWCC/BWE、RTCP、jitter |
| [phase-05-client-p2p-datachannel-interop-fuzz.md](phase-05-client-p2p-datachannel-interop-fuzz.md) | 部分完成 | client、P2P、DataChannel、互操作、fuzz |

---

## 渐进式执行顺序

1. **Phase 01** — 先固定 SDP、codec、RTP extension 与 core 事件边界，防止后续 driver/module 反复改协议模型。
2. **Phase 02** — 补齐单端口 UDP/TCP、多线程分片、ICE full 与连接迁移，建立高并发传输基础。
3. **Phase 03** — 在 module 层打通推流、播放、WHIP/WHEP、ZLM-style API、GOP 秒开和协议互转。
4. **Phase 04** — 做实时质量闭环：simulcast、RTX/NACK、TWCC/BWE、RTCP feedback、jitter/reorder 弱网能力。
5. **Phase 05** — 补 client pull/push、P2P、自定义信令、DataChannel 兼容、浏览器和 ZLM 互操作、fuzz/corpus。

---

## 总体验收

每个阶段完成后至少运行：

```powershell
cargo fmt
cargo clippy -p cheetah-webrtc-core
cargo clippy -p cheetah-webrtc-driver-tokio
cargo clippy -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-property-tests
```

影响 `cheetah-codec`、`cheetah-sdk`、`cheetah-engine` 或协议互转时，继续运行对应 crate 的测试。弱网、浏览器、ZLMediaKit、Pion、GStreamer、Janus 互操作用 ignored/manual 矩阵记录实际命令和结果。

