# WebRTC OME 对标增强 — 任务清单

本任务清单对应 `dev-docs/plans-27-webrtc-ome` 下的 5 个阶段文档，用于跟踪后续实现进度。

## 任务清单

- [x] 1. 编写 OME 对标设计文档
  - [x] 1.1 编写总索引 `index.md`
  - [x] 1.2 编写架构文档 `webrtc-ome-architecture.md`
  - [x] 1.3 编写差距分析 `webrtc-ome-gap-analysis.md`

- [x] 2. Phase 01: 信令与 transport 策略兼容
  - [x] 2.1 OME URL 与 `direction` query 兼容
  - [x] 2.2 WebSocket 自定义信令 request-offer/answer/candidate/stop（已完成 JSON schema、decoder、action 映射、`request_offer` session plan、抽象 handler、WebSocket text transport、offer response 渲染、独立 WebSocket listener/server loop、`ome_ws_listen` 配置校验、module start 接入、publish/play 媒体桥接，以及 driver `CreateOffer` per-session candidate policy；真实客户端互操作归入 Phase 05）
  - [x] 2.3 `transport=udp|tcp|relay|udptcp|all` 与 `DefaultTransport`
  - [x] 2.4 `TcpRelayForce` / `iceServers` 输出策略（已完成 `TcpRelayForce` 对 OME session candidate 策略的 relay-only 覆盖、OME `ome_ice_servers` 配置校验、WHIP/WHEP `Link: rel="ice-server"` 输出，以及 OME JSON `iceServers`/`ice_servers` 双字段渲染）

- [x] 3. Phase 02: ingest SDP、simulcast 与时间戳
  - [x] 3.1 OME SDP fixtures 与 compat 诊断（已新增 core OME publish simulcast offer、H265 payload descriptor fixtures；simulcast offer 可被 core 接受，H265 descriptor 进入预处理/诊断入口）
  - [x] 3.2 RID/simulcast 行为对齐（已将 `q/h/f` 等 OME/ZLM RID 质量序固定为 `q < h < f`，同步校准 Highest/Lowest/Adaptive/REMB/NACK storm 选择测试；`MultiStream` 仍保持全层透传）
  - [x] 3.3 CompositionTime / CTS / RTCP-SR timestamp 模式（已新增 `rtcp_based_timestamp` 配置；默认 fast-start 将每个 WebRTC ingress track 的首个 RTP timestamp 归零，开启后保留 RTP epoch，为 RTCP-SR 对齐模式预留）

- [x] 4. Phase 03: playback、ABR、jitter、playout
  - [x] 4.1 playlist/rendition 映射（已新增 publish bridge rendition snapshot，按 MID 暴露当前 RID 与已见 RID，并在 session GET 返回 `renditions` 观测字段）
  - [x] 4.2 `WebRtcAutoAbr` 与本地层选择闭环（已新增 `webrtc_auto_abr` 开关；BWE/REMB 对 publish 层选择的驱动可按会话配置启停）
  - [x] 4.3 JitterBuffer / PlayoutDelay 策略（已新增 `play_jitter_buffer_ms`、`playout_delay_{min,max}_ms` 配置；播放发送链路支持平滑延迟；本地 SDP 可按配置注入 playout-delay extmap；session GET 暴露延迟观测字段）
  - [x] 4.4 `FIRInterval` 配置与关键帧恢复（已新增 `fir_interval_ms`；module 启动周期任务并通过 driver `RequestKeyframe(FIR)` 向 publish 会话发起关键帧请求，同时向 play 会话对应 `StreamKey` 请求上游关键帧）

- [x] 5. Phase 04: 弱网恢复、FEC、RTCP 与观测
  - [x] 5.1 RED/ULPFEC 协商与开关（已新增 `enable_red_ulpfec` 开关；默认对本地 SDP 按 media section 执行 RED/ULPFEC payload 过滤，并级联移除指向 RED/ULPFEC 的 RTX payload，避免未接通数据面时误协商）
  - [x] 5.2 RTX/NACK/TWCC/REMB/RTCP 指标增强（已补 `rtcp_sr/rtcp_rr/rtcp_nack` per-session 遥测，并在 `GET /session/{id}` 返回；保留 TWCC/REMB/RTX 既有观测；RTP extmap 通过 `telemetry.rtp_extensions` 暴露 operator surface）
  - [x] 5.3 弱网恢复回归矩阵（已补 `weak_network_nack_recovery` ignored 用例与 Phase 03/04 关键路径单元回归）

- [x] 6. Phase 05: fixtures、互操作、fuzz 加固
  - [x] 6.1 OME 专项 SDP/URL/config fixtures（已新增 play UDP/relay+RED-ULPFEC/H265 low-latency fixtures，并纳入 core fixture 回归）
  - [x] 6.2 OvenRtcTester / 浏览器互操作 ignored 测试（已新增 `ome_ws_request_offer_smoke` 与 `ome_oven_rtc_tester_smoke` ignored skeleton）
  - [x] 6.3 property-tests 与 fuzz 补强（已新增 `property_ome_compat.rs`，并补充 fuzz corpus OME seeds）

## 完成定义

- 文档层面：已完成。
- 代码层面：Phase 01~05 已全部完成。当前除已落地 OME URL/transport/WebSocket 信令与 publish/play 媒体桥接外，已补齐 `webrtc_auto_abr`、Jitter/Playout/FIRInterval、RED/ULPFEC SDP 策略、RR/SR/NACK 遥测字段、OME fixtures、OME interop ignored skeleton、property/fuzz OME 补强。
