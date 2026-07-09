# GB28181 与 RTP 协议完善计划（对标 ABLMediaServer）

- **状态**: 已完成
- **方法**: 对比 `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer` 中 GB28181、RTP、JTT1078、RTCP、SIP 与 HTTP API 相关实现，结合本项目现有 `cheetah-codec`、RTSP/TS 能力、控制面 API 与三段式协议约束逐项补齐
- **完成标准**: 标准 GB28181/RTP 能力可用，ABL 风格非标准兼容落地，RTP/GB28181 与 RTSP/RTMP/HLS 互转通过单元、集成、端到端、互操作和 fuzz 测试

---

## V1 完善范围

本轮是对现有媒体能力的协议完善，不重新定义本项目总架构。

1. `[x]` 支持 UDP/TCP RTP 推流服务端，覆盖 `ps`、`ts`、`es`、`jtt1078`
2. `[x]` 支持 UDP/TCP RTP 转推客户端，覆盖主动模式、被动模式、收发双工模式
3. `[x]` 支持本地 RTSP/RTMP/HLS/内部流转 RTP 推流
4. `[x]` 支持 RTP 推流进入 engine 后再转 RTSP/RTMP/HLS 等协议输出
5. `[x]` 支持 GB28181 主动拉流和被动收流
6. `[x]` 支持双向语音对讲
7. `[x]` 支持 H264/H265/AAC/G711A/G711U/OPUS/MP3/VP8/VP9/AV1
8. `[x]` 支持多视频/多音频轨道模式
9. `[x]` 完善单元测试、集成测试、property tests 和 fuzz tests

本轮不做：

1. 完整国标平台级目录联动、云台控制、告警订阅、录像查询
2. 转码；不被目标协议原生支持的编码只保证稳定转封装和诊断
3. 浏览器 WebRTC 方案替代国标媒体主路径

---

## ABLMediaServer 关键参考

| 领域 | ABL 文件 | 重点行为 |
|------|----------|----------|
| RTP recv server | `NetGB28181RtpServer.*` | UDP/TCP active/passive、收发双工、PS/ES/XHB/JTT1078、RTCP、动态切包 |
| RTP send client | `NetGB28181RtpClient.*` | UDP/TCP send、TCP 2-byte/4-byte framing、PS/ES/JTT1078、收发双工 |
| generic RTP PS/TS ingress | `NetServerRecvRtpTS_PS.*` | 单端口 RTP 入口、按 PS 头或 TS 长度自动分流 |
| PS ingress | `RtpPSStreamInput.*` | PS demux、AAC/G711 识别、真实帧率计算 |
| TS ingress | `RtpTSStreamInput.*` | TS demux、AAC ADTS、H264/H265、188 字节对齐 |
| RTCP | `RtcpPacket.*` | SR/RR 基础结构、SSRC report block |
| SIP parse | `ABLSipParse.*`、`DigestAuthentication.*` | 宽松行解析、重复 header、Digest 参数拆分 |
| HTTP API | `NetServerHTTP.cpp` | `openRtpServer`、`closeRtpServer`、`startSendRtp`、`stopSendRtp`、`sendJtt1078Talk` |
| shared config | `stdafx.h` | `openRtpServerStruct`、`startSendRtpStruct`、`nGBRtpTCPHeadType`、`ForceSendingIFrame`、`nSaveGB28181Rtp` |

---

## 版本信息里的关键能力

必须在设计和测试中显式覆盖 `vendor-ref/ABLMediaServer-src-2026-05-09/版本信息.txt` 中已经落地的行为：

1. 2026-05-08 到 2026-04-23：RTP 接收缓冲区切割持续优化，重点兼容海康下级平台发送的国标流
2. 2026-04-24：GB TCP 接收超时回收、国标发送真实帧率更新、`recv_app/recv_stream` 修复
3. 2026-04-23 到 2026-03-24：RTP/GB/1078 视频帧率按 RTP timestamp 或 `frame_interval` 动态计算，不能固定 25fps
4. 2026-03-21：`ForceSendingIFrame` 优先发送最新 IDR 缓存
5. 2026-03-19 到 2026-03-15：JTT1078 音视频时间戳、PT 映射、平均帧率计算
6. 2026-03-13：按 I 帧长度动态更新 `nMaxRtpLength`
7. 2026-03-10：`nSaveGB28181Rtp` 调试落盘、H264/H265 SDP 修正
8. 2026-03-09：缺失时补回 SPS/PPS，分发前优先推送参数集
9. 2026-03-06：JTT1078 拼帧缓存长度受 `Ma1078CacheBufferLength` 上界保护
10. 2026-02-12 到 2026-02-05：`recv_app/recv_stream` 重复接入保护、TCP 2-byte 与 4-byte 头兼容、动态最大 RTP 长度

---

## 与本地实现对比后的主要缺口

| 能力 | ABL 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| 独立 RTP server/client | `NetGB28181RtpServer`、`NetGB28181RtpClient` | `crates/protocols/rtp/` 已出现但能力未闭环 | Phase 02/03 |
| RTP over TCP 2-byte 和 4-byte framing | `GB28181SentRtpVideoData`、TCP cache split | 无统一实现 | Phase 01/02 |
| 动态切包与坏流恢复 | `nMaxRtpLength`、缓冲区切割 | 无 | Phase 02 |
| 单端口 RTP PS/TS 自动分流 | `NetServerRecvRtpTS_PS` | 只有局部 TS ingress | Phase 01/02 |
| RTCP SR/RR | `RtcpPacket.*` | 只有局部 RTSP 内部实现 | Phase 02 |
| `openRtpServer/startSendRtp` 双工 API | `NetServerHTTP.cpp` | 无完整 REST 语义 | Phase 03 |
| JTT1078 2013/2016/2019 | `SplitterJt1078CacheBuffer*` | 无 | Phase 05 |
| 真实帧率与 IDR/SPS 缓存 | `CalcFlvVideoFrameSpeed`、`ForceSendingIFrame` | 基础时间戳能力不完整 | Phase 01 |
| 宽松 SIP/Digest 兼容 | `ABLSipParse`、`DigestAuthentication` | 无独立 GB28181 crate | Phase 04 |
| 测试和 fuzz | ABL 实战行为丰富 | 现有 RTP/GB 体系缺失 | Phase 05 |

---

## 标准与非标准兼容点

### 标准基线

- RTP 基于 RFC 3550，支持 UDP 与 TCP 传输
- RTCP 至少支持 SR、RR，首版保留向 XR/RTT/jitter/loss 扩展的上下文
- PS/TS/ES/JTT1078 输入最终统一为 `AVFrame + TrackInfo`
- GB28181 控制面遵循 SIP/SDP/REGISTER/INVITE/ACK/BYE/Keepalive 基本流程
- 单一 `StreamKey` 保持单发布者独占

### ABL / 真实落地兼容优先

- 兼容 RTP over TCP 前置 2 字节长度头
- 兼容 `$ + channel + length` 4 字节 interleaved 头
- 兼容单端口 RTP 中混合的 PS、TS、ES 和私有载荷
- 兼容 payload type 的历史映射和显式配置注入
- 兼容 `ssrc` 未指定时按首包或默认路径建流
- 兼容设备脏数据、乱序、半包、粘包、超长包和异常 source address
- 兼容 JTT1078 常开端口、SIM/channel 建流、live/playback/talk/sub 命名
- 兼容海康类下级平台在 TCP RTP 切包上的历史实现差异

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [gb28181-rtp-architecture.md](gb28181-rtp-architecture.md) | `[x]` 已完成 | 总体架构、crate 边界、数据流、配置、REST API |
| [gb28181-rtp-abl-gap-analysis.md](gb28181-rtp-abl-gap-analysis.md) | `[x]` 已完成 | ABL 行为、兼容点、本地缺口 |
| [phase-01-codec-rtp-ps-ts-es.md](phase-01-codec-rtp-ps-ts-es.md) | `[x]` 已完成 | `cheetah-codec` 媒体内核、JTT1078、时间戳、payload |
| [phase-02-rtp-core-driver-transport.md](phase-02-rtp-core-driver-transport.md) | `[x]` 已完成 | `cheetah-rtp-core`、`cheetah-rtp-driver-tokio`、UDP/TCP/RTCP |
| [phase-03-rtp-module-rest-bridge.md](phase-03-rtp-module-rest-bridge.md) | `[x]` 已完成 | `cheetah-rtp-module`、REST API、转推和桥接 |
| [phase-04-gb28181-sip-control.md](phase-04-gb28181-sip-control.md) | `[x]` 已完成 | GB28181 SIP control、主动拉流、媒体会话 |
| [phase-05-voice-talk-jtt1078-interop-testing.md](phase-05-voice-talk-jtt1078-interop-testing.md) | `[x]` 已完成 | 语音对讲、JTT1078、互操作、fuzz 和生产化验证 |

---

## 渐进式执行顺序

1. **Phase 01** — 先补齐 `cheetah-codec` 的 RTP/PS/TS/ES/JTT1078、多编码、多轨和时间戳能力
2. **Phase 02** — 建立 `cheetah-rtp-core + cheetah-rtp-driver-tokio`，跑通 UDP/TCP RTP/RTCP 和 ABL 风格兼容路径
3. **Phase 03** — 接入 engine 和控制面，形成 RTP server/client、转推、主动/被动模式和 REST API
4. **Phase 04** — 实现 GB28181 SIP 控制面、主动拉流和媒体会话编排
5. **Phase 05** — 补语音对讲、JTT1078 兼容、fixture、互操作、property/fuzz 和运维指标
