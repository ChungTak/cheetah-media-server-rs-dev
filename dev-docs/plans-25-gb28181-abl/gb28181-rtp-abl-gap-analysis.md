# GB28181 / RTP 与 ABLMediaServer 差距分析

- **状态**: 已完成
- **范围**: 记录 ABLMediaServer 在 RTP/GB28181 媒体面上的真实落地行为、本地当前状态和必须补齐的缺口
- **完成标准**: 后续阶段能按本文逐项补齐 RTP/RTCP、GB28181、JTT1078、voice talk 和互操作验证

## ABL 关键行为

| 领域 | ABL 文件 | 观察到的行为 |
|------|----------|--------------|
| RTP server | `NetGB28181RtpServer.*` | 支持 UDP/TCP active/passive、收发双工、PS/ES/XHB/JTT1078、RTCP |
| RTP client | `NetGB28181RtpClient.*` | 支持 ES/PS/JTT1078 发送、TCP 2-byte/4-byte framing、同会话接收 |
| generic RTP ingest | `NetServerRecvRtpTS_PS.*` | 默认 schema 为 `rtp`，按 payload 判断 PS/TS |
| PS input | `RtpPSStreamInput.*` | 双 demux 实现、真实帧率、AAC/G711 提取 |
| TS input | `RtpTSStreamInput.*` | 188 字节对齐、AAC ADTS、H264/H265 直推 |
| RTCP | `RtcpPacket.*` | SR/RR 基础结构、按 SSRC 维护 report block |
| SIP parse | `ABLSipParse.*` | `\r\n`/`\n`/`\r` 宽松解析、重复字段重命名入表、`;` `,` 参数拆分 |
| REST | `NetServerHTTP.cpp` | `openRtpServer`、`closeRtpServer`、`startSendRtp`、`stopSendRtp`、`sendJtt1078Talk` |

## 当前本地状态

| 能力 | 当前状态 | 备注 |
|------|----------|------|
| 独立 RTP crate | 已开始 | `crates/protocols/rtp/` 已存在，但还不是完整可用链路 |
| 独立 GB28181 crate | 已开始 | `crates/protocols/gb28181/` 已存在，但还未形成完整控制面 |
| RTP header/reorder | 已有基础 | `cheetah-codec` 有基础 RTP/重排能力 |
| TS demux/mux | 较完整 | `ts/core` 能力可复用 |
| PS mux/demux | 不足 | 仍需完成为生产可用实现 |
| RTCP 独立抽象 | 无 | RTSP 内部有局部实现，未形成通用模块 |
| JTT1078 | 无 | |
| REST RTP/GB API | 无闭环 | 控制面还未形成 ABL 风格 send/recv 模型 |

## ABL 标准行为

- RTP v2 包头校验、seq/ssrc/source address 管理
- UDP/TCP 双传输
- TCP 模式同时兼容 2-byte 大端长度头和 4-byte interleaved 头
- RTCP SR/RR 和基本 report block
- PS/TS/raw RTP 进入统一 process
- 主动/被动发送与接收都具备

## ABL 落地兼容行为

1. **海康风格 TCP 切包兼容**  
   `版本信息.txt` 从 2026-02 到 2026-05 多次调整 RTP 接收缓冲区切割，说明现实问题集中在 TCP 粘包、半包和错误头识别。本地必须把这段逻辑做成共享 deframer。

2. **动态最大 RTP 长度学习**  
   ABL 会根据 I 帧长度或实际 TCP 包长持续更新 `nMaxRtpLength`。本地也要有动态学习，但必须附带上界、超时和诊断事件。

3. **单端口 RTP PS/TS 自动分流**  
   `NetServerRecvRtpTS_PS` 通过 RTP 负载内容自动决定交给 PS 还是 TS pipeline。本地不能只假设单一承载格式。

4. **双工会话模型**  
   ABL 的 send client 和 recv server 都支持 `recv_app/recv_stream` 或 `send_app/send_stream_id` 的双工路径。本地 RTP module 应直接建模 `send_recv`。

5. **真实帧率而非固定 25fps**  
   RTP/GB/JTT1078 都会根据 timestamp 或 `frame_interval` 更新帧率。本地时间系统必须支持动态 video fps 学习和更新。

6. **IDR/SPS/PPS 强制补发**  
   ABL 提供 `ForceSendingIFrame`，并在参数集缺失时补发 SPS/PPS。本地需要把这部分放进 `cheetah-codec` 缓存与发送视图。

7. **JTT1078 常开端口与多模式命名**  
   live、playback、talk、sub 通过 keep-open mode 区分。本地若不显式建模，后续对讲和回放会混淆路径。

8. **宽松 SIP 解析**  
   `ABLSipParse` 会接受多种换行、重复字段和 header 参数写法。本地若只按理想 SIP 报文处理，设备兼容会明显受限。

## 必须补齐的缺口

1. `[x]` 独立 `cheetah-rtp-core`
2. `[x]` 独立 `cheetah-rtp-driver-tokio`
3. `[x]` 独立 `cheetah-rtp-module`
4. `[x]` 独立 `cheetah-gb28181-core`
5. `[x]` 独立 `cheetah-gb28181-driver-tokio`
6. `[x]` 独立 `cheetah-gb28181-module`
7. `[x]` TCP 2-byte/4-byte framing 与异常恢复（`RtpTcpFraming::AutoDetect` + 4 KiB 有界 SSRC/PS pack-start 扫描恢复）
8. `[x]` 单端口 RTP PS/TS auto probe（`probe_rtp_payload` + `RtpCore` 自动建会话）
9. `[x]` RTCP SR/RR 抽象（5 秒 tick 自动生成、`RtcpPacket` 路由 + `last_rr_received_ms` 用于 sender shutdown）
10. `[x]` JTT1078 2013/2016/2019 parser、sender、talk（`Jtt1078Header::parse`/`parse_v2019`、`Jtt1078Packetizer`、`Jtt1078KeepOpenMode::Talk`）
11. `[x]` publish auth 前 bounded frame cache（`ActiveIngressSession::pending_frames` + `publish_frame_cache_frames`）
12. `[x]` 单元测试、集成测试、fixture 和 fuzz

## 编码矩阵

| 编码 | RTP ingest | RTP egress | 载体 | 备注 |
|------|------------|------------|------|------|
| H264 | 支持 | 支持 | PS/TS/ES/JTT1078 | 主路径 |
| H265 | 支持 | 支持 | PS/TS/ES/JTT1078 | 主路径 |
| AAC | 支持 | 支持 | PS/TS/ES/JTT1078 | 主路径 |
| G711A/U | 支持 | 支持 | PS/ES/JTT1078 | 国标兼容重点 |
| Opus | 支持 | 支持 | ES/TS | 明确 payload 映射 |
| MP3 | 支持 | 支持 | PS/ES | 弱支持播放器多 |
| VP8 | 支持 | 支持 | ES/TS | 容器正确性优先 |
| VP9 | 支持 | 支持 | ES/TS | 容器正确性优先 |
| AV1 | 支持 | 支持 | ES/TS | 容器正确性优先 |

## 互操作风险

- 真实设备常把国标 PS、TCP framing、私有变体混在一起
- RTP timestamp 会回绕、乱序、异常跳变，不是理想单调输入
- 语音对讲通常只走音频单轨，但设备要求的 PT、packet duration、sample rate 经常不一致
- JTT1078 拼帧和国标 TCP 切包都需要严格的有界缓存，否则容易被坏流拖死
