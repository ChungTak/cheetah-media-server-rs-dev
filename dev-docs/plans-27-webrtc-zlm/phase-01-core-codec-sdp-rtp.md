# Phase 01 — Core、Codec、SDP 与 RTP Extension

- **状态**: 部分完成（Phase 01 第一+二轮：ZLM SDP fixtures 引入并通过端到端 `AcceptOffer` 集成测试，core 公共事件扩展为 `RtcpFeedback::Remb/Bye`、`SimulcastLayerObserved` 与 `WebRtcSimulcastRidSource` 完整 fallback 链；第二轮新增 `a=ssrc-group:SIM` 无 RID 时自动注入 `r0/r1/r2` + `a=simulcast` 的 SDP 预处理兼容、`RtpExtensionType` 全矩阵枚举与 `from_uri/uri` roundtrip、`RtpExtensionMapping` 结构体与 `extract_rtp_extension_mappings` 公共 API、`RtpExtensionObserved` core 事件在 `AcceptOffer` 后自动发出、`SdpCompatReport` 扩展 `ssrc_group_sim_rid_generated` / `extmap_allow_mixed_observed` 字段、`munged_ssrc_sim_no_rid.sdp` fixture 与集成测试；codec profile、Access Unit / 参数集策略与浏览器 SDP fixture 在后续小步迭代中继续）

## 实现概览

本阶段固定协议模型和媒体 contract。目标是让后续 driver/module 不再猜测 SDP、codec、RTP extension、RTCP event 的形状。

## 已完成（Phase 01 第一轮）

- `crates/protocols/webrtc/core/tests/fixtures/` 新增 ZLM 官方 fixture：`zlm_offer.sdp`、`zlm_offer_simulcast.sdp`、`zlm_janus_offer.sdp`、`zlm_janus_answer.sdp`。
- `crates/protocols/webrtc/core/tests/zlm_sdp_fixtures.rs` 集成测试覆盖 ZLM offer / simulcast offer / Janus offer 的 `preprocess_remote_sdp + AcceptOffer` 完整路径，确保 `Created` / `LocalDescriptionReady` 事件、Answer SDP 都正确产出。
- `WebRtcRtcpFeedback` 公共枚举扩展：新增 `Remb { mid, bitrate_bps }` 与 `Bye`，沿用既有 `Pli/Fir/Nack/Twcc/SenderReport/ReceiverReport` 命名。
- `WebRtcCoreEvent` 新增 `SimulcastLayerObserved { observation: WebRtcSimulcastLayerObservation }`，与 ZLM `RtpExtContext` 行为对齐：每个 RID 一条事件，标注来源（`SdpRid`、`RidExt`、`RepairedRidExt`、`SsrcSimGroup`、`Generated`）。
- `Str0mEvent::EgressBitrateEstimate(BweKind::Remb)` 现在同时映射为 `Bwe` 与 `RtcpFeedback::Remb` 事件，让 module 区分本地 BWE 估计与远端 REMB 反馈。
- `Str0mEvent::MediaAdded` 处理路径在 `MediaTrackAdded` 之后，按 simulcast layers 分别发出 `SimulcastLayerObserved`，标注 `WebRtcSimulcastRidSource::SdpRid`。

## 已完成（Phase 01 第二轮）

- `inject_rid_from_ssrc_group_sim` 公共函数：当 SDP 中某 m= section 存在 `a=ssrc-group:SIM <ssrc0> <ssrc1> [<ssrc2>]` 但没有 `a=rid:` 行时，自动注入 `a=rid:r0 send` / `a=rid:r1 send` / `a=rid:r2 send` 和 `a=simulcast:send r0;r1;r2`，对齐 ZLM `RtpExtContext` 的 SSRC 顺序生成稳定 RID 行为。
- `preprocess_remote_sdp` 在 whitespace 规范化之后自动调用 `inject_rid_from_ssrc_group_sim`，并在 `SdpCompatReport` 中标记 `ssrc_group_sim_rid_generated = true`。
- `SdpCompatReport` 新增 `ssrc_group_sim_rid_generated` 和 `extmap_allow_mixed_observed` 字段，前者参与 `is_modified()` 判定触发诊断输出。
- `RtpExtensionType` 公共枚举：覆盖 ZLM `RTP_EXT_MAP` 全矩阵（AudioLevel、AbsSendTime、TransportWideCc、Mid、Rid、RepairedRid、VideoOrientation、VideoTiming、PlayoutDelay、TransmissionOffset、VideoContentType、ColorSpace、FrameMarking、Av1DependencyDescriptor、Unknown），提供 `from_uri` / `uri` 双向映射。
- `RtpExtensionMapping` 结构体：携带 `id`、`ext_type`、`uri`、`direction`，表示单条 `a=extmap` 行的解析结果。
- `extract_rtp_extension_mappings(sdp) -> Vec<RtpExtensionMapping>` 公共 API：轻量行级解析器，从 SDP 文本中提取所有 extmap 映射，支持 direction qualifier（`/sendonly` 等）和 malformed 行跳过。
- `WebRtcCoreEvent::RtpExtensionObserved { session_id, mappings }` 新事件：在 `AcceptOffer` 成功后、`LocalDescriptionReady` 之后自动发出，让 module 无需重新解析 SDP 即可获得完整 extension 映射。
- `munged_ssrc_sim_no_rid.sdp` 新 fixture：模拟 SDP-munging 工具剥离 RID/simulcast 但保留 `a=ssrc-group:SIM` 的真实场景。
- 集成测试新增：`munged_ssrc_sim_no_rid_is_accepted`（fixture 经预处理后能被 str0m 接受）、`munged_ssrc_sim_no_rid_generates_rid_labels`（验证 r0/r1/r2 注入）、`zlm_simulcast_offer_reports_extmap_allow_mixed`（验证 extmap-allow-mixed 观测）、`accept_offer_emits_rtp_extension_observed`（验证 RtpExtensionObserved 事件携带正确映射）。
- 单元测试新增 12 条：`inject_rid_from_ssrc_group_sim` 各分支（生成 r0/r1/r2、已有 RID 不修改、两 SSRC 组、单 SSRC 忽略、多 m= section 独立处理）、`RtpExtensionType::from_uri` roundtrip、`extract_rtp_extension_mappings` 从 ZLM fixture 提取 / direction qualifier / malformed 跳过、`extmap_allow_mixed_observed` 检测。

## 已完成（Phase 01 第三轮）

- 浏览器 SDP fixture 化：`offer_from_chrome.sdp`、`offer_from_firefox.sdp`、`offer_from_safari.sdp` 三个真实浏览器 offer 样例。
- Chrome fixture 覆盖：VP8/VP9/H264/AV1 + RTX、`extmap-allow-mixed`、`rtcp-rsize`、`ssrc-group:FID`、audio-level / abs-send-time / transport-cc / mid / rid / repaired-rid / video-orientation / playout-delay / video-content-type / video-timing / color-space 全矩阵 extmap。
- Firefox fixture 覆盖：`a=rid:q send` + `a=rid:h send` + `a=simulcast:send q;h` 原生 simulcast、direction qualifier `/recvonly` extmap、Mozilla-style `o=` 行。
- Safari fixture 覆盖：H264 High Profile (`640c1f`) + Baseline (`42e01f`)、VP8/VP9、`ssrc-group:FID`、无 `extmap-allow-mixed`。
- `browser_sdp_fixtures.rs` 集成测试 8 条：三浏览器 `AcceptOffer` 成功、Firefox simulcast RID 保留、Chrome `extmap-allow-mixed` 观测、Safari 无 `extmap-allow-mixed`、Chrome RTP extension 提取、Firefox direction qualifier 提取。

## 已完成（Phase 01 第四轮）

- `crates/foundation/cheetah-codec/tests/webrtc_egress_contract.rs` 新增 14 条 WebRTC egress contract 测试矩阵：
  - H264 参数集缓存：从 Annex B keyframe 提取 SPS/PPS、从 extradata 提取、prepend 到 IDR payload。
  - H265 参数集缓存：从 Annex B keyframe 提取 VPS/SPS/PPS、从 extradata 提取、prepend 到 IDR payload。
  - WebRTC egress contract 验证：拒绝无 AU boundary 的 H264 视频帧、接受有 AU boundary 的 H264/H265 帧、音频帧不要求 AU boundary。
  - WebRTC egress contract view：keyframe 携带 `random_access=true`、delta frame 携带 `random_access=false`。
  - `ParameterSetRequirement`：空缓存报告 `RequiredMissing`、填充后报告 `RequiredPresent`。

## 后续小步迭代

Phase 01 所有计划项已完成。


## 1.1 SDP 兼容与 fixture

新增 fixtures：

- `vendor-ref/ZLMediaKit/webrtc/offer.sdp`
- `vendor-ref/ZLMediaKit/webrtc/offer-simulcast.sdp`
- `vendor-ref/ZLMediaKit/webrtc/janus_offer.sdp`
- `vendor-ref/ZLMediaKit/webrtc/janus_answer.sdp`
- ZLMRTCClient Chrome/Firefox/Safari offer 样例

实现要求：

- `cheetah-webrtc-core::sdp_compat` 只做输入规范化和兼容诊断，不做业务策略。
- 支持 CRLF/LF/CR 行尾、末尾空白、重复空行、未知 `rtcp-fb`、`extmap-allow-mixed`。
- 对缺失 `rtcp-mux`、缺失 fingerprint、candidate 格式非法、BUNDLE 不完整返回明确错误。
- `SdpCompatReport` 增加修改项列表，用于 module 观测。

测试：

- `minimal_offer.sdp`、ZLM offer、simulcast offer、Janus offer 都能预处理并进入 `str0m`。
- property test 覆盖行尾、空白、candidate 顺序和未知属性。
- fuzz target 保留 UTF-8/非 UTF-8 边界。

## 1.2 Codec profile

在 core/module 配置中明确 profile：

- `browser`：H264、VP8、VP9、AV1、Opus；AAC 默认拒绝；H265 按配置开启。
- `rtsp-compatible`：H264、H265、G711A、G711U、AAC、Opus；优先协议互转。
- `surveillance`：H264、H265、G711A/U 优先，允许非浏览器 WebRTC client。
- `datachannel-only`：不要求 media m-line。

约束：

- codec 策略在 module 选择，core 只接受明确的 offer/answer spec。
- 不做转码；协商失败必须返回 codec diagnostic。
- WebRTC 输出前必须通过 `cheetah-codec` egress contract，视频帧必须有 Access Unit 边界。

## 1.3 RTP extension 统一模型

新增或扩展 core 事件，让 module 能观测：

- extension uri、id、direction、track、mid、rid、repaired_rid。
- audio level、abs send time、transport-wide sequence、video orientation、video timing、playout delay、framemarking、AV1 dependency descriptor。
- RID fallback 来源：`rid-ext`、`repaired-rid-ext`、`ssrc-sim-map`、`generated`.

参考 ZLM `RtpExtContext`：

- 接收 RTP 时按 ext id 找 type。
- 发送 RTP 时按 type 找 ext id。
- RID 缺失时从 SSRC map 回填。
- `a=ssrc-group:SIM` 没有 RID 时生成 `r0/r1/r2`。

## 1.4 Core event 与 stats

Core 需要明确输出：

- `RtcpFeedback::Nack { mid, count }`
- `RtcpFeedback::Remb { bitrate_bps }`
- `RtcpFeedback::ReceiverReport`
- `RtcpFeedback::SenderReport`
- `RtcpFeedback::Bye`
- `RtpExtensionObserved`
- `SimulcastLayerObserved`

stats 至少包含：

- RTP packets/bytes in/out
- RTCP packets/bytes in/out
- NACK in/out、RTX sent/miss
- TWCC feedback in/out
- BWE bitrate、loss、rtt、jitter
- DataChannel messages/bytes in/out

## 1.5 Codec 媒体能力补强

`cheetah-codec` 负责：

- H264/H265 参数集解析、缓存、关键帧补发、STAP-A/single NALU 策略。
- RTP timestamp 与 `AVFrame` timebase 转换。
- B 帧检测或 frame reorder policy 的通用输出。
- Opus/G711/AAC track info 与 SDP fmtp 导出。
- WebRTC egress contract 校验：随机访问、AU 边界、timestamp 来源。

不允许 WebRTC module 私自复制 NALU、参数集或 timestamp 修正逻辑。

## 1.6 测试要求

运行：

```powershell
cargo test -p cheetah-webrtc-core
cargo test -p cheetah-codec future_protocol_adapter_contract
cargo test -p cheetah-webrtc-property-tests
```

新增测试场景：

- ZLM simulcast SDP 能识别 RID/SSRC。
- Janus SDP unknown attribute 不导致 panic。
- H264/H265/AAC/G711/Opus profile 决策稳定。
- WebRTC egress 缺 AU boundary 被拒绝。
- RTP extension id/type 映射 roundtrip。

## 完成后检查

- core 仍无 Tokio、socket、HTTP、engine 依赖。
- 所有新增公共类型使用 `module` 命名，不引入本项目禁用命名。
- 文档同步 `webrtc-zlm-architecture.md` 的事件和配置。

