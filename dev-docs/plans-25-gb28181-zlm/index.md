# GB28181 与 RTP 协议完善计划（对标 ZLMediaKit）

- **状态**: 已完成
- **方法**: 对比 `vendor-ref/ZLMediaKit/src/Rtp`、`Rtcp`、`Common`、`Rtsp`、`TS`、`Http`、`Pusher` 相关实现，结合本项目现有 `cheetah-codec`、RTSP module、TS module、控制面 API 与三段式协议约束逐项补齐
- **完成标准**: 标准 GB28181/RTP 能力可用，ZLMediaKit 风格非标准兼容落地，RTP/GB28181 与 RTSP/RTMP/HLS 互转通过单元、集成、端到端、互操作和 fuzz 测试

---

## V1 完善范围

本轮是对现有媒体能力的协议完善，不重新定义本项目总架构。

1. 支持 UDP/TCP RTP 推流服务端，覆盖 `ps`、`ts`、`es`、`ehome`
2. 支持 UDP/TCP RTP 转推客户端，覆盖主动模式、被动模式、收发双工模式
3. 支持本地 RTSP/RTMP/HLS/内部流转 RTP 推流
4. 支持 RTP 推流进入 engine 后再转 RTSP/RTMP/HLS 等协议输出
5. 支持 GB28181 主动拉流和被动收流
6. 支持双向语音对讲
7. 支持 H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1
8. 支持多视频/多音频轨道模式
9. 完善单元测试、集成测试、property tests 和 fuzz tests

本轮不做：

1. 完整国标平台级目录联动、云台控制、告警订阅、录像查询
2. 转码；不被目标协议原生支持的编码只保证稳定转封装和诊断
3. 浏览器 WebRTC 方案替代国标媒体主路径

---

## ZLMediaKit 关键参考

| 领域 | ZLMediaKit 文件 | 重点行为 |
|------|-----------------|----------|
| RTP server | `src/Rtp/RtpServer.*` | UDP/TCP active/passive、单端口多流、SSRC 锁定、RTCP |
| RTP session | `src/Rtp/RtpSession.*` | TCP 2-byte framing、SSRC 校验、异常上下文恢复 |
| RTP splitter | `src/Rtp/RtpSplitter.*` | Ehome 识别、2-byte/4-byte framing 兼容 |
| RTP process | `src/Rtp/RtpProcess.*` | 推流鉴权、frame cache、超时、统计、默认建流 |
| GB28181 media process | `src/Rtp/GB28181Process.*` | RTP payload type 到 codec/PS/TS 判定 |
| RTP sender | `src/Rtp/RtpSender.*` | ES/PS/TS 发送、UDP/TCP active/passive、voice talk、RTCP |
| RTP encoder/cache | `src/Rtp/PSEncoder.*`、`RawEncoder.*`、`RtpCache.*` | G711 包时长、按帧聚合发送 |
| RTCP | `src/Rtcp/*` | SR/RR/XR、RTT、jitter、loss |
| 公共发送入口 | `src/Common/MultiMediaSourceMuxer.cpp` | `startSendRtp/stopSendRtp`、多目标发送 |
| 时间戳与 packet cache | `src/Common/Stamp.cpp`、`PacketCache.h` | 回绕、乱序、merge flush |

---

## 与本地实现对比后的主要缺口

| 能力 | ZLM 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| 独立 RTP server/client | `RtpServer`、`RtpSender` | 无独立协议 crate | Phase 02/03 |
| RTP over TCP 2-byte framing | `RtpSession`、`RtpSplitter` | 无统一实现 | Phase 01/02 |
| RTP TCP 上下文恢复 | `searchBySSRC/searchByPsHeaderFlag` | 无 | Phase 02 |
| Ehome 私有头兼容 | `RtpSplitter` | 无 | Phase 01/02 |
| RTP 推流鉴权前 frame cache | `RtpProcess` | 无 | Phase 03 |
| RR/SR/XR 与 RTT/jitter/loss | `RtcpContext` | RTSP 内部有一部分，未独立抽象 | Phase 02 |
| `startSendRtp` ES/PS/TS/voice talk | `RtpSender`、`MultiMediaSourceMuxer` | 无通用 RTP 转推能力 | Phase 03 |
| G711 包时长配置 | `kRtpG711DurMs` | 无显式 RTP 包时长策略 | Phase 01 |
| only audio/video | `OnlyTrack` | 无 | Phase 02/03 |
| 单端口多流默认 `/live/{ssrc}` | `RtpSession/RtpProcess` | 无 | Phase 02/03 |
| GB28181 media plane 兼容 | `GB28181Process` | 无独立 GB28181 crate | Phase 04 |
| 主动拉流/被动收流 | `RtpServer + 上层 API` | 无 | Phase 04 |
| 双向语音对讲 | `kVoiceTalk` | 无 | Phase 05 |
| 测试和 fuzz | ZLM 无直接参考但行为丰富 | 现有 RTP/GB 体系缺失 | Phase 05 |

---

## 标准与非标准兼容点

### 标准基线

- RTP 基于 RFC 3550，支持 UDP 与 TCP 传输
- RTCP 至少支持 SR、RR，首版补 XR DLRR 兼容
- PS/TS/ES 输入最终统一为 `AVFrame + TrackInfo`
- GB28181 控制面遵循 SIP/SDP/REGISTER/INVITE/ACK/BYE/Keepalive 基本流程
- 单一 `StreamKey` 保持单发布者独占

### ZLMediaKit / 真实落地兼容优先

- 兼容 RTP over TCP 前置 2 字节长度头
- 兼容 Ehome 私有头 + RTP/RTSP-style framing 变体
- 兼容 TCP 上下文损坏后按 SSRC 或 PS system header 恢复
- 兼容 payload type 到 codec 的历史映射和显式配置注入
- 兼容 G711 国标默认 100ms 打包时长
- 兼容 `ssrc` 未指定时按首包或默认路径建流
- 兼容主动/被动 RTP 发送和语音对讲共用现有链路
- 兼容厂商脏数据、乱序、半包、粘包、异常 source address 和 timestamp 回绕

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [gb28181-rtp-architecture.md](gb28181-rtp-architecture.md) | 已完成 | 总体架构、crate 边界、数据流、配置、REST API |
| [gb28181-rtp-zlm-gap-analysis.md](gb28181-rtp-zlm-gap-analysis.md) | 已完成 | ZLM 行为、兼容点、本地缺口 |
| [phase-01-codec-rtp-ps-ts-es.md](phase-01-codec-rtp-ps-ts-es.md) | 已完成 | `cheetah-codec` 媒体内核、Ehome、时间戳、payload |
| [phase-02-rtp-core-driver-transport.md](phase-02-rtp-core-driver-transport.md) | 已完成 | `cheetah-rtp-core`、`cheetah-rtp-driver-tokio`、UDP/TCP/RTCP |
| [phase-03-rtp-module-rest-bridge.md](phase-03-rtp-module-rest-bridge.md) | 已完成 | `cheetah-rtp-module`、REST API、转推和桥接 |
| [phase-04-gb28181-sip-control.md](phase-04-gb28181-sip-control.md) | 已完成 | GB28181 SIP control、主动拉流、媒体会话 |
| [phase-05-voice-talk-ehome-interop-testing.md](phase-05-voice-talk-ehome-interop-testing.md) | 已完成 | 语音对讲、Ehome、互操作、fuzz 和生产化验证 |

---

## 渐进式执行顺序

1. **Phase 01** — 先补齐 `cheetah-codec` 的 RTP/PS/TS/ES/Ehome、多编码、多轨和时间戳能力
2. **Phase 02** — 建立 `cheetah-rtp-core + cheetah-rtp-driver-tokio`，跑通 UDP/TCP RTP/RTCP 和 ZLM 风格兼容路径
3. **Phase 03** — 接入 engine 和控制面，形成 RTP server/client、转推、主动/被动模式和 REST API
4. **Phase 04** — 实现 GB28181 SIP 控制面、主动拉流和媒体会话编排
5. **Phase 05** — 补语音对讲、Ehome 兼容、fixture、互操作、property/fuzz 和运维指标
