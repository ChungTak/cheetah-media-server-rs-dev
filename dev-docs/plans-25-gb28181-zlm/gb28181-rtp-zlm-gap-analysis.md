# GB28181 / RTP 与 ZLMediaKit 差距分析

- **状态**: 已完成
- **范围**: 记录 ZLMediaKit 在 RTP/GB28181 媒体面上的真实落地行为、本地当前状态和必须补齐的缺口
- **完成标准**: 后续阶段能按本文逐项补齐 RTP/RTCP、GB28181、Ehome、voice talk 和互操作验证

## ZLMediaKit 关键行为

| 领域 | ZLM 文件 | 观察到的行为 |
|------|----------|--------------|
| RTP server | `RtpServer.*` | 支持 UDP/TCP active/passive、单端口多流、显式 stream 与默认 SSRC 建流 |
| RTP session | `RtpSession.*` | 处理 TCP 2-byte framing、SSRC 校验、异常长度检测、按 SSRC/PS header 恢复上下文 |
| RTP splitter | `RtpSplitter.*` | 兼容 Ehome 私有头、RTSP 4-byte interleaved 和 2-byte 长度头 |
| RTP process | `RtpProcess.*` | 推流鉴权前缓存 frame、按超时清理、默认 schema 为 `rtp`、无 stream 时使用 SSRC |
| GB28181 process | `GB28181Process.*` | 按 payload type 决定 raw codec 或 PS/TS，动态判断 TS vs PS |
| RTP sender | `RtpSender.*` | ES/PS/TS 发送、TCP/UDP active/passive、voice talk、RTCP SR/timeout |
| RTCP | `RtcpContext.*` | SR/RR/XR、RTT、jitter、loss 统计 |
| Packet cache | `PacketCache.h`、`RtpCache.*` | 按一帧 RTP 聚合输出，低延迟时仍保留按 timestamp flush |
| Timestamp | `Stamp.cpp` | RTP timestamp 回绕、乱序、异常跳变兼容 |

## 当前本地状态

| 能力 | 当前状态 | 备注 |
|------|----------|------|
| 独立 RTP crate | 无 | 只有 `cheetah-codec` 基础 RTP 和 `ts/core` 的 RTP-TS ingress |
| 独立 GB28181 crate | 无 | 只有计划文档，没有实现 |
| RTP header/reorder | 已有基础 | `cheetah-codec/src/rtp.rs`、`rtp_reorder.rs` |
| TS demux/mux | 较完整 | `ts_demux.rs`、`ts_mux.rs` 可复用 |
| PS mux/demux | 不足 | `ps.rs` 仍是骨架级能力 |
| RTSP RTP codec | 较完整 | `rtsp/module/src/media/` 有 packetize/depacketize |
| 控制面 REST 框架 | 已有 | `cheetah-control` 可挂模块路由 |
| RTCP 独立抽象 | 无 | RTSP 内部有局部实现，未形成通用模块 |
| Voice talk | 无 | |
| Ehome | 无 | |

## ZLM 标准行为

- RTP v2 包头校验、seq/ssrc/source address 管理
- UDP/TCP 双传输
- TCP 模式默认 2-byte 大端长度前缀
- RTCP SR/RR/XR 和基本 RTT/loss/jitter 统计
- PS/TS/raw RTP 进入统一 process
- 主动/被动发送与接收都具备

## ZLM 落地兼容行为

1. **Ehome 自动识别**  
   `RtpSplitter` 先探测私有头，再决定是 2-byte 还是 RTSP-style 4-byte framing。本地需要把这段兼容逻辑做成共享路径。

2. **TCP 上下文恢复**  
   `RtpSession` 在帧长异常或坏包后，会按 SSRC 搜索，失败后再按 PS system header 搜索恢复。本地必须补这一类有界恢复，否则面对真实设备的坏流只能断开。

3. **默认按 SSRC 建流**  
   未指定 stream id 时，ZLM 会直接把 SSRC 转成本地 stream。这个行为对被动收流非常重要，应兼容。

4. **鉴权前缓存 frame**  
   `RtpProcess` 在 publish auth 返回前缓存最多 10 秒 frame，避免首屏丢包。本地若无同类缓冲，hook 型鉴权会导致前几帧丢失。

5. **G711 RTP 包时长可配置**  
   ZLM 把国标 G711 RTP 包时长单独做成配置项，默认 100ms。本地应在 RTP encode 层提供同等控制。

6. **Voice talk 使用已有上行链路回发**  
   `kVoiceTalk` 模式下，发送端直接拿目标流现有 `RtpProcess` 的 socket 回写 RTP。本地应规划对应的 talk session service。

7. **RTCP timeout 影响 sender 生命周期**  
   ZLM 可在 UDP 发送时检查 RR timeout，超时主动关闭 sender。本地也应保留这个生产化行为。

8. **payload type 历史映射**  
   ZLM 对 PT<96、H264/H265/Opus/PS 有默认映射，并允许未知 PT 退回 `mpeg-ps or mpeg-ts` 猜测。本地需要更明确的兼容策略和 diagnostic。

## 必须补齐的缺口

1. 独立 `cheetah-rtp-core`
2. 独立 `cheetah-rtp-driver-tokio`
3. 独立 `cheetah-rtp-module`
4. 独立 `cheetah-gb28181-core`
5. 独立 `cheetah-gb28181-driver-tokio`
6. 独立 `cheetah-gb28181-module`
7. RTP TCP 2-byte framing 与异常恢复
8. Ehome probe 与私有头剥离
9. RTCP SR/RR/XR 抽象
10. G711/Opus/PT 映射和 packet duration
11. ZLM 风格主动/被动 send/recv/voice talk
12. publish auth 前 bounded frame cache
13. 单元测试、集成测试、fixture 和 fuzz

## 编码矩阵

| 编码 | RTP ingest | RTP egress | 载体 | 备注 |
|------|------------|------------|------|------|
| H264 | 支持 | 支持 | PS/TS/ES | 主路径 |
| H265 | 支持 | 支持 | PS/TS/ES | 主路径 |
| AAC | 支持 | 支持 | PS/TS/ES | 主路径 |
| G711A/U | 支持 | 支持 | PS/ES | 国标兼容重点 |
| Opus | 支持 | 支持 | ES | 需显式 PT 配置 |
| MP3 | 支持 | 支持 | PS/ES | 弱支持播放器多 |
| VP8 | 支持 | 支持 | ES/TS | 容器正确性优先 |
| VP9 | 支持 | 支持 | ES/TS | 容器正确性优先 |
| AV1 | 支持 | 支持 | ES/TS | 容器正确性优先 |

## 互操作风险

- 真实设备常把国标 PS、TCP framing、Ehome 私有头混在一起
- RTP timestamp 会回绕、乱序、异常跳变，不是理想单调输入
- 语音对讲通常只走音频单轨，但设备要求的 PT、packet duration、sample rate 经常不一致
- UDP passive 依赖对端打洞和 peer 地址锁定，错误处理要足够保守
