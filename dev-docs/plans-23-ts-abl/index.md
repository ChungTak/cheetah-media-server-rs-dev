# TS 协议完善计划（对标 ABLMediaServer）

- **状态**: 计划中
- **目标**: 在当前已实现 TS 协议基础上，对照 `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer` 补齐 ABL 在真实设备、国标/RTP、HTTP-TS、WS-TS、编码兼容和多轨道场景中的工程能力。
- **版本信息输入**: 必须参考 `vendor-ref/ABLMediaServer-src-2026-05-09/版本信息.txt`，其中 2026-03-21 至 2026-05-08 的高频主题是 RTP 接收缓冲区切割、海康下级平台兼容、真实帧率估计、G711 时间戳、MP4 文件循环读取、FFmpeg 日志开关和国标发送/接收修复。
- **本轮边界**: 不推翻已完成的 `dev-docs/plans-23-ts-zlm` 方案；本轮只补 ABL 对照后发现的缺口，并把非标准兼容逻辑集中命名。

---

## 计划文件清单

| 文件 | 范围 |
|------|------|
| [ts-abl-gap-analysis.md](ts-abl-gap-analysis.md) | ABL 版本信息、参考源码和本地实现的逐项差距 |
| [ts-abl-architecture.md](ts-abl-architecture.md) | ABL 兼容增强后的分层边界、数据流和兼容层命名 |
| [phase-01-codec-ts-compat.md](phase-01-codec-ts-compat.md) | `cheetah-codec` TS demux/mux、非标准 stream_type、时间戳、帧率估计、容错 |
| [phase-02-rtp-ts-ingest.md](phase-02-rtp-ts-ingest.md) | RTP-over-TS 输入、SSRC 分流、RTP 切包兼容、GB/海康场景 |
| [phase-03-http-ws-ts-live.md](phase-03-http-ws-ts-live.md) | HTTP(S)-TS 与 WS(S)-TS 直播输出/拉流细节，对齐 ABL 行为 |
| [phase-04-codec-multitrack-interop.md](phase-04-codec-multitrack-interop.md) | H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1/MP2、多轨道、互操作与性能验收 |

---

## 本地现状摘要

当前本地 TS 已具备：

1. `cheetah-codec` 中共享 `MpegTsMuxer` / `MpegTsDemuxer`
2. `.ts` / `.live.ts` HTTP 请求解析
3. HTTP/HTTPS 与 WS/WSS server driver
4. HTTP(S)/WS(S)-TS pull client
5. H264/H265/H266/AAC/G711/OPUS/MP3/MP2/VP8/VP9/AV1 stream_type 映射
6. PAT/PMT 初始发送与周期补发
7. demux 前导垃圾重同步、continuity diagnostic、PES 重组上限和 flush
8. module 播放会话、pull job、publisher lease、subscriber 有界队列

因此本轮重点不是“从零实现 TS”，而是补：

1. ABL 风格 RTP-TS 输入和 SSRC 分流
2. 真实设备脏 RTP/TS 输入的切包、重同步和诊断
3. ABL 的真实视频帧率估计与 G711 时间戳推导
4. HTTP/WS 输出批量缓冲、错误计数和慢客户端隔离
5. 多轨道动态更新、超限诊断和 ABL/libmpeg 私有编码互操作样例

---

## ABL 参考入口

| 功能 | ABL 文件 |
|------|----------|
| HTTP-TS 播放输出 | `NetServerHTTP_TS.cpp` / `NetServerHTTP_TS.h` |
| WS-TS 播放输出 | `NetServerWS_TS.cpp` / `NetServerWS_TS.h` |
| RTP-TS 输入 | `RtpTSStreamInput.cpp` / `RtpTSStreamInput.h` |
| RTP PS/TS 入口分流 | `NetServerRecvRtpTS_PS.cpp` / `NetServerRecvRtpTS_PS.h` |
| TS 录制 | `StreamRecordTS.cpp` / `StreamRecordTS.h` |
| 通用 URL、帧率、AAC、G711 helper | `NetRecvBase.cpp` / `NetRecvBase.h` |
| 媒体源和客户端管理 | `MediaStreamSource.cpp` / `MediaStreamSource.h` |

---

## 阶段顺序

1. **Phase 01**: 先补 codec 层兼容能力，保证所有协议复用统一 TS 容器。
2. **Phase 02**: 实现 RTP-TS 输入计划，解决 ABL 版本信息中最重要的 RTP 接收切割和海康兼容问题。
3. **Phase 03**: 强化 HTTP(S)-TS / WS(S)-TS live 输出和 pull，保证真实客户端可持续播放。
4. **Phase 04**: 用编码矩阵、多轨道、ABL/ZLM/ffmpeg/VLC 样例和故障输入收口。

---

## 验收总标准

1. HTTP(S)-TS 直播播放可用。
2. WS(S)-TS 直播播放可用，TS bytes 必须用 WebSocket binary frame 承载。
3. RTP-TS 输入能从 RTP payload 中解析 TS，支持 SSRC 分流、PS/TS 识别和切包容错。
4. 支持 H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1/MP2 的 mux/demux 与播放输出。
5. 支持多 video/audio 轨道，PMT PID 稳定，pull 发现新轨后不覆盖旧轨。
6. 脏数据、前导垃圾、半包、粘包、continuity gap、空 body、慢客户端不会 panic、无限缓存或拖垮其它连接。
