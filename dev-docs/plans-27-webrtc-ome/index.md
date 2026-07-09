# WebRTC OME 对标增强计划索引

> **状态口径**: 本目录是设计与开发计划文档，并同步记录阶段实现进度。`已有/复用` 表示本地已有同类基础；`部分具备` 表示已有骨架但未达到 OME 行为；`进行中` 表示已有代码落地但阶段未完成；`未开始` 表示后续开发任务。

## 背景

本计划参考 `vendor-ref/OvenMediaEngine` 的 WebRTC provider、publisher、signalling、ICE、RTP/RTCP 与公开文档，对比本地 `crates/protocols/webrtc/{core,driver-tokio,module}` 的现状，规划下一阶段 WebRTC 协议增强。

本项目继续遵守仓库约束：

- WebRTC 仍采用 `core + driver + module` 三段式。
- `core` 保持 Sans-I/O，不直接依赖 runtime、socket、HTTP 或引擎。
- `driver` 负责 UDP/TCP、timer、route、候选收发、连接迁移。
- `module` 负责 HTTP/WebSocket 信令、引擎接入、配置、兼容层、会话生命周期。
- 媒体时间戳、参数集、封装视图优先沉淀到 `cheetah-codec`。

## OME 参考结论

OME 的 WebRTC 价值不在于其 DTLS/SRTP/ICE 底层实现本身，而在于一组完整的工程行为：

- 自定义 WebSocket 信令与 WHIP 并存。
- `?direction=` 和 `?transport=` 驱动 publish/play 与 UDP/TCP/relay 候选策略。
- 直接 TCP ICE、TURN relay、`iceServers` 下发与 `TcpRelayForce` 兼容。
- 播放侧 playlist/rendition、`WebRtcAutoAbr`、TransportCC/REMB 驱动 ABR 切换。
- `JitterBuffer`、`PlayoutDelay`、周期性 FIR、RTCP-SR 时间戳模式等真实部署配置。
- RED/RTX/ULPFEC、H264/H265/VP8/Opus、simulcast、RID、非标准 SDP 兼容。

本地已经具备较完整的 WebRTC 三段式框架、WHIP/WHEP、ZLM/SMS/ABL 兼容层、BWE/RTCP/Simulcast 指标、P2P 与互操作脚手架。OME 对标的目标是把这些能力补到更接近主流流媒体服务器的行为面，而不是迁移其底层协议栈。

## 文档清单

| 文档 | 状态 | 说明 |
| --- | --- | --- |
| [webrtc-ome-architecture.md](webrtc-ome-architecture.md) | 已完成 | OME 行为抽象、本地分层落点、能力边界 |
| [webrtc-ome-gap-analysis.md](webrtc-ome-gap-analysis.md) | 已完成 | OME 与本地实现差距、优先级、风险 |
| [phase-01-signaling-transport-ome-compat.md](phase-01-signaling-transport-ome-compat.md) | 已完成 | 已落地 OME URL/`direction` HTTP 入口、`transport`/`DefaultTransport`、每请求 SDP candidate 过滤、`CreateOffer` candidate policy、`TcpRelayForce` relay-only 覆盖、`iceServers` 输出兼容，以及 OME WebSocket schema/action/session plan/handler/transport/listener/server loop/module 配置接入、服务端会话 id 校验和 publish/play 媒体桥接；真实客户端互操作回归归入 Phase 05 |
| [phase-02-ingest-sdp-simulcast-timestamp.md](phase-02-ingest-sdp-simulcast-timestamp.md) | 已完成 | 已补 OME publish simulcast/H265 SDP fixtures、core 兼容诊断入口、OME/ZLM RID 质量序，以及 `rtcp_based_timestamp` ingest 时间戳模式 |
| [phase-03-playback-abr-jitter-playout.md](phase-03-playback-abr-jitter-playout.md) | 已完成 | 已补 `webrtc_auto_abr` 闭环、Jitter/Playout 配置与发送平滑、playout-delay extmap 注入、`fir_interval_ms` 周期 FIR 任务（publish 远端 FIR + play 上游关键帧请求），以及 session GET 的播放时延观测 |
| [phase-04-loss-control-bwe-fec-observability.md](phase-04-loss-control-bwe-fec-observability.md) | 已完成 | 已补 `enable_red_ulpfec` 协商开关、本地 SDP RED/ULPFEC 过滤、RTP extmap operator surface、RR/SR/NACK 细粒度遥测输出与弱网恢复回归入口 |
| [phase-05-interop-fixtures-fuzz-hardening.md](phase-05-interop-fixtures-fuzz-hardening.md) | 已完成 | 已补 OME play fixtures、OME interop ignored 入口（WS/OvenRtcTester）、property/fuzz OME 补强 |
| [tasks.md](tasks.md) | 已完成 | 跨 phase 总任务清单与状态跟踪 |

## 总任务状态

| 阶段 | 任务 | 状态 |
| --- | --- | --- |
| Phase 01 | 补齐 OME 风格信令与 transport 策略兼容 | 已完成 |
| Phase 02 | 补齐发布侧 SDP、simulcast 与时间戳兼容 | 已完成 |
| Phase 03 | 补齐播放侧 playlist/ABR/jitter/playout 兼容 | 已完成 |
| Phase 04 | 补齐弱网恢复、FEC/BWE/RTCP 与观测面 | 已完成 |
| Phase 05 | 补齐 OME 样例、互操作与回归体系 | 已完成 |

## 建议执行顺序

1. 先做 Phase 01，固定入口协议和候选策略，否则后续互操作样例没有稳定入口。
2. 再做 Phase 02，先把 publish/ingest 的 SDP、RID、timestamp 行为补齐。
3. 接着做 Phase 03，把播放侧 OME 特有的 playlist/ABR/jitter/playout 行为落到 module/codec。
4. 然后做 Phase 04，补齐 RTCP/BWE/FEC 与指标闭环。
5. 最后做 Phase 05，把 OME 文档样例、测试程序和真实 SDP/弱网回归接入。

## 最低验证命令

后续实现阶段每个受影响 crate 至少执行：

```powershell
cargo fmt
cargo clippy -p cheetah-webrtc-core
cargo clippy -p cheetah-webrtc-driver-tokio
cargo clippy -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-module
```

涉及 `cheetah-codec`、property tests、互操作样例或 fuzz 时，继续运行对应测试集。
