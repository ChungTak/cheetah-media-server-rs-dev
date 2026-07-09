# GB28181 / RTP 与 SimpleMediaServer 差距分析

- **状态**: 已完成
- **范围**: 记录 SMS 的 RTP 与 GB28181 实际实现行为、兼容落地点、本项目现状和必须补齐的能力缺口
- **完成标准**: 实现阶段可以按本文逐项补齐媒体面、控制面、API 和互操作测试

## SMS 关键路径

| 领域 | SMS 文件 | 观察到的行为 |
|------|----------|--------------|
| RTP API | `Api/RtpApi.cpp` | 提供 server create/stop、client create/start/stop |
| GB28181 API | `Api/GB28181Api.cpp` | 提供 recv/send create/stop，区分 active/passive |
| RTP server | `Rtp/RtpServer.cpp` | 同时处理 UDP/TCP 接收、发送和 RTCP |
| RTP context | `Rtp/RtpContext.cpp` | 管理 SSRC、source address、payload mode、send/recv 模式 |
| RTP parser | `Rtp/RtpParser.cpp` | TCP 模式采用前置 2 字节长度头，处理半包/粘包 |
| RTP decode track | `Rtp/RtpDecodeTrack.cpp` | 支持 `ps`、`ts` 和 raw audio/video |
| RTP encode track | `Rtp/RtpEncodeTrack.cpp` | 支持 `ps`、`ts` 和 raw audio/video 输出 |
| GB28181 server | `GB28181/GB28181Server.cpp` | 更像 GB 媒体会话封装，不是完整 SIP 平台 |
| GB28181 context | `GB28181/GB28181Context.cpp` | 逻辑与 RTP context 相近，默认 payload 为 `ps` |
| Ehome | `Ehome2/`、`Ehome5/` | 有单独厂商兼容实现 |

## SMS 实际行为判断

从 `vendor-ref/simple-media-server/Src` 看，SMS 的 GB28181 重点在媒体面和 REST 编排，不是完整平台级 SIP 生态：

1. 提供 GB28181 收流/推流 API，但主路径仍是 RTP server/client 与媒体上下文
2. RTP 与 GB28181 都高度依赖 PS mux/demux 和 RTP over TCP 2-byte framing
3. API 上大量依赖 `appName`、`streamName`、`ssrc`、`socketType`、`payloadType`
4. send/recv 模式和主动/被动模式通过 REST 编排，不靠复杂状态机自动发现

这意味着 Cheetah 不能只抄 REST 形态，还需要把缺失的标准 GB28181 SIP 控制面独立补上。

## SMS 标准行为

- RTP v2 包头解析、sequence/ssrc/source address 管理
- UDP/TCP 双传输
- TCP 模式使用 2 字节大端长度前缀
- `ps`、`ts`、raw payload 进入统一 decode track
- server/client 分离，支持 `recv_only`、`send_only`、`send_recv`
- GB28181 REST 支持 active/passive 两种媒体建立方式

## SMS 落地兼容行为

1. **RTP over TCP 固定 2-byte 长度头**  
   `RtpParser.cpp` 与 `GB28181Parser.cpp` 都按 2 字节大端长度解析 TCP 负载。本项目需要把该 framing 抽成共享 compat 逻辑。

2. **GB28181 与 RTP 默认 payloadType 都偏向 `ps`**  
   SMS 代码中 `ps` 是默认路径。Cheetah 首版也以 `ps` 为默认，但 `ts/es/ehome` 必须作为一等能力。

3. **未知 SSRC 可默认映射到 `/live/{ssrc}`**  
   `RtpManager` 在未显式绑定 `app/stream` 时，会按 SSRC 生成默认本地流。Cheetah 应保留该兼容策略，同时支持显式映射。

4. **UDP source address 被首包锁定**  
   SMS 接收后会固定来源地址，后续地址变化通常被拒绝。Cheetah 默认保持一致，并允许兼容模式下重绑定。

5. **RTCP 实现偏最小可用**  
   SMS 主要做基础 SR/SDES 回应。Cheetah 首版不追求完整 RTCP profile，但要具备可观测和 bounded 行为。

6. **raw codec 映射依赖 payload type 和显式配置**  
   SMS raw 模式下要从 API 参数拿 codec、sample rate、channel 等信息。Cheetah 也必须在 REST 配置中保留这些字段。

7. **GB28181 实现与完整 SIP 设备管理存在空隙**  
   SMS 更像“国标 RTP 兼容流媒体服务”。Cheetah 需要把真正的 REGISTER/INVITE/Keepalive 等流程补全。

## 本项目现状

| 能力 | 当前位置 | 状态 |
|------|----------|------|
| RTP header parse/encode | `crates/foundation/cheetah-codec/src/rtp.rs` | 基础能力已有 |
| RTP reorder | `crates/foundation/cheetah-codec/src/rtp_reorder.rs` | 可复用 |
| TS demux/mux | `crates/foundation/cheetah-codec/src/ts_demux.rs`、`ts_mux.rs` | 能力较完整 |
| PS parse/encode | `crates/foundation/cheetah-codec/src/ps.rs` | 仅基础骨架，不足以承载生产路径 |
| RTP-TS Sans-I/O ingest | `crates/protocols/ts/core/src/rtp_ts.rs` | 只支持 TS，PS 当前拒绝 |
| RTSP RTP packetize/depacketize | `crates/protocols/rtsp/module/src/media/` | codec-specific RTP 能力丰富，但未抽为独立 RTP 协议 |
| 控制面 API | `crates/system/cheetah-control/` | 框架已在，可挂模块 REST |
| GB28181 crate | 无 | 完全缺失 |
| 独立 RTP crate | 无 | 完全缺失 |

## 必须补齐的实现缺口

1. 独立 `cheetah-rtp-core`
2. 独立 `cheetah-rtp-driver-tokio`
3. 独立 `cheetah-rtp-module`
4. 独立 `cheetah-gb28181-core`
5. 独立 `cheetah-gb28181-driver-tokio`
6. 独立 `cheetah-gb28181-module`
7. PS mux/demux 升级为生产级
8. RTP PS/TS/ES/Ehome payload 统一 decode/encode
9. RTP over TCP 2-byte framing
10. 独立 RTCP 路径和 session timeout
11. SMS 风格 REST API
12. GB28181 标准 SIP 控制面
13. 主动拉流模式
14. 双向语音对讲
15. 真实设备/SMS/Ehome 互操作样例

## 编码矩阵

| 编码 | RTP ingest | RTP egress | PS/TS/ES | 备注 |
|------|------------|------------|----------|------|
| H264 | 支持 | 支持 | PS/TS/ES | 主路径 |
| H265 | 支持 | 支持 | PS/TS/ES | 主路径 |
| AAC | 支持 | 支持 | PS/TS/ES | 主路径 |
| G711A/U | 支持 | 支持 | PS/ES | 厂商兼容重点 |
| Opus | 支持 | 支持 | ES | 跨协议桥接重点 |
| MP3 | 支持 | 支持 | PS/ES | 弱支持播放器多 |
| VP8 | 支持 | 支持 | ES/TS | 容器正确性优先 |
| VP9 | 支持 | 支持 | ES/TS | 容器正确性优先 |
| AV1 | 支持 | 支持 | ES/TS | 容器正确性优先 |

## 互操作风险

- PS 是 GB28181 主路径，但厂商常在 PES、PTS/DTS、系统头和 program stream map 上不规范
- RTP over TCP 2-byte framing 不是 RTSP interleaved RTP，不能复用 `$` framing
- TS 多轨较成熟，PS/ES 和 Ehome 才是主风险区
- GB28181 设备常出现 Keepalive 不规范、Contact/From/To 不一致、SSRC 与 SDP 不一致
- 语音对讲通常只走音频单轨，但设备经常要求特定 payload type、sample rate 和通道数
