# Phase 05 — Client、P2P、DataChannel、互操作与 Fuzz

- **状态**: 部分完成（Phase 05 第一+二+三轮：`property-tests` 增加 ZLM `rtc://` URL parser、TCP RFC 4571 framing、trickle-ICE candidate 三套属性测试矩阵；`fuzz/` 增加 `fuzz_zlm_rtc_url`、`fuzz_tcp_framing`、`fuzz_trickle_candidates` 三个 cargo-fuzz harness；首轮 `cargo fuzz run` 发现并修复 `rtc://:port/...` 空 host 误判，新增对应回归单元测试；DataChannel 最大消息长度上界 `WebRtcCoreLimits::max_data_channel_message_bytes` 落地，超长 payload 在 core 内被丢弃并发出 `PendingOutputDropped` 诊断。Client pull/push、P2P 信令、DataChannel 兼容矩阵、浏览器/ZLMediaKit 互操作 `--ignored` 矩阵留作后续小步迭代）

## 实现概览

本阶段补齐 WebRTC 主动拉流/推流、P2P、自定义信令、DataChannel 生产兼容，以及浏览器和 ZLMediaKit 互操作闭环。

## 已完成（Phase 05 第一轮）

- `crates/protocols/webrtc/testing/property-tests/tests/property_zlm_rtc_url.rs`：4 条 proptest，覆盖 `parse_zlm_rtc_url` 不 panic、合法 URL 字段级 round-trip、`signaling_protocols` 始终是无符号整数、未知 query keys 全部进入 `extra_params`。
- `crates/protocols/webrtc/testing/property-tests/tests/property_tcp_framing.rs`：4 条 proptest，覆盖 `Tcp4571Decoder` 在任意字节流下不 panic、encode/decode round-trip、连续帧在任意分片下解码结果一致、整片 vs 单字节流式喂入产出相同。
- `cheetah-webrtc-fuzz` 新增 `fuzz_zlm_rtc_url` 与 `fuzz_tcp_framing` 两个 cargo-fuzz harness；首轮各跑 10 秒，URL fuzzer 第一次执行立刻发现 `rtc://:77/...` 空 host 误判（host=""，port=Some(77)），现已在 `parse_zlm_rtc_url` 加入 `host.is_empty() → MissingHost` 判定；20 秒重测覆盖 1.2M 输入仍然清洁。
- `crates/protocols/webrtc/fuzz/README.md` 同步更新，列出三个 harness 与运行命令。
- `crates/protocols/webrtc/module/src/compat.rs::rejects_empty_host_with_port` 单元回归测试，固化 `rtc://:77/live/demo` 必须返回 `ZlmRtcUrlError::MissingHost`。

## 已完成（Phase 05 第二轮）

- `crates/protocols/webrtc/testing/property-tests/tests/property_trickle_candidates.rs`：4 条 proptest，覆盖 `extract_trickle_candidates` 不 panic、所有输出行以 `candidate:` 前缀且非空、非候选行被丢弃、合法候选数量 round-trip。
- `cheetah-webrtc-fuzz` 新增 `fuzz_trickle_candidates` cargo-fuzz harness：libfuzzer 11 秒 / 921k 输入清洁。
- `crates/protocols/webrtc/fuzz/README.md` 同步更新到四个 harness。

## 已完成（Phase 05 第三轮）

- `WebRtcCoreLimits::max_data_channel_message_bytes`（默认 256 KiB，对齐 ZLM `data_channel_message_max`）作为新的核心配置上界。`WebRtcCore::send_data_channel` 在调用 `str0m` 之前 short-circuit 检查载荷长度，超出阈值时发出 `PendingOutputDropped` 诊断（携带通道 id、payload 长度、配置上界），不再进入 SCTP 缓冲。
- `WebRtcModuleConfig::datachannel_max_message_bytes` wire 字段（默认 256 KiB）通过 `to_driver_config` 写入 `core.limits`。`validate` 拒绝零值。
- 单元测试：`send_data_channel_oversized_payload_emits_diagnostic_and_drops` 在 8 字节 cap 下喂 32 字节 payload，断言 `PendingOutputDropped` 出现且 session 仍存活；`config::tests::rejects_zero_datachannel_max_message_bytes` 与 `datachannel_max_message_bytes_propagates_to_driver_config` 固化 wire / driver 透传契约。

## 已完成（Phase 05 第四轮）

- `fuzz_rtp_ext_mappings` cargo-fuzz harness：对 `extract_rtp_extension_mappings` 进行 coverage-guided fuzzing，断言不 panic、id > 0、uri 非空、`ext_type` 与 `from_uri(uri)` 一致。
- fuzz Cargo.toml 注册新 target。

## 已完成（Phase 05 第五轮）

- DataChannel close-after-write 诊断：`WebRtcCore::send_data_channel` 在 channel id 未知或已关闭时，从硬 `Err` 改为发出 `PendingOutputDropped` 诊断并优雅丢弃，对齐 ZLM 行为；session 仍存活，后续写入不受影响。
- 单元测试 `send_data_channel_unknown_channel_emits_diagnostic_not_error` 验证 close-after-write 路径。
- `fuzz_ice_candidate` cargo-fuzz harness：对 `extract_trickle_candidates` + `extract_trickle_ice_restart_creds` 进行 coverage-guided fuzzing，断言不 panic、candidate 行非空且以 `candidate:` 开头、ICE restart credentials 字段非空。

## 已完成（Phase 05 第六轮）

- `fuzz_sdp_ssrc_sim` cargo-fuzz harness：对 `inject_rid_from_ssrc_group_sim` 进行 coverage-guided fuzzing，断言不 panic、UTF-8 保持有效、第二轮 idempotent。

## 已完成（Phase 05 第七轮）

- 互操作 ignored 测试矩阵扩展：从 3 条扩展到 9 条，涵盖 ZLMediaKit / ZLMRTCClient.js / RTSP→WebRTC / RTMP→WebRTC / GB28181→WebRTC / 弱网 NACK 恢复，每条测试在 docstring 中记录可复现命令（docker / ffmpeg / `tc netem` 命令行）。
- 每个互操作测试明确环境变量约定（`WEBRTC_INTEROP_ZLM` / `WEBRTC_INTEROP_ZLMRTCCLIENT` / `WEBRTC_INTEROP_RTSP_TO_WEBRTC` / `WEBRTC_INTEROP_RTMP_TO_WEBRTC` / `WEBRTC_INTEROP_GB28181_TO_WEBRTC` / `WEBRTC_INTEROP_WEAK_NETWORK`）和验证矩阵（首帧时延 / 关键帧 / 弱网恢复指标）。

## 后续小步迭代

- RTCP feedback / DataChannel control message 的 fuzz harness（依赖外部 RTP/RTCP 解析器）。
- `WebRtcSignaling*` 等价的 P2P WebSocket 信令栈（check-in / candidate / bye / 房间 keeper）。
- DataChannel PPID、ordered/unordered（DataChannel 消息 cap、close-after-write、每通道 outbound queue 上界已落地）。
- 互操作 ignored 测试的实际 SDP 交换 + 媒体面验证体（当前仅落地 scaffold，等待 CI 资源支持）。


## 5.1 WebRTC client pull

支持：

- `webrtc://host:port/app/stream?signaling_protocols=0`：WHEP/SFU。
- `webrtcs://host:port/app/stream?signaling_protocols=0`：HTTPS WHEP。
- `webrtc://signaling-host:port/app/stream?signaling_protocols=1&peer_room_id=room`：WebSocket P2P。

流程：

1. 解析 URL，执行 SSRF 防护。
2. 创建 local offer。
3. WHIP/WHEP 模式 POST offer，应用 answer。
4. P2P 模式连接信令服务器，check-in，交换 offer/answer/candidate。
5. ICE/DTLS/SRTP 成功后把远端媒体作为本地 publisher 推入 engine。
6. stop 时发送 DELETE 或 P2P bye。

## 5.2 WebRTC client push

支持：

- 将本地 engine stream 推到远端 WHIP endpoint。
- 将本地 engine stream 推到 P2P 房间。

流程：

1. 校验本地 stream 存在。
2. 创建 local offer，协商 sendonly/sendrecv。
3. 订阅本地 stream，按 codec profile 发送。
4. 远端 PLI/FIR 时请求本地上游关键帧。
5. BWE 下降时执行降层或丢 delta frame。
6. stop 时释放 subscriber 和远端 session。

## 5.3 P2P 信令

ZLM P2P 行为：

- 有 room keeper / room list。
- 通过 WebSocket 交换 SDP 和 candidate。
- 支持 check-in、candidate、bye、query info。

本项目实现：

- `p2p/add`：创建 room keeper。
- `p2p/remove`：删除 room keeper。
- `p2p/list`：列出 room 和 peer 状态。
- P2P session 仍然走 `cheetah-webrtc-core` 和 driver，不绕过资源上界。
- 自定义信令消息必须限长、鉴权、校验 room id。

## 5.4 DataChannel

能力：

- text/binary message。
- ordered/unordered。
- stream id 和 label 观测。
- PPID 兼容。
- max message size。
- open/close/error event。
- echo mode。
- control message mode。

约束：

- DataChannel-only session 不要求 audio/video。
- 每 session DataChannel 数量有上界。
- 每 channel outbound queue 有上界。
- 超长消息拒绝并输出 diagnostic。
- close 后发送返回明确错误。

## 5.5 互操作矩阵

浏览器：

- Chrome: WHIP publish、WHEP play、echo、DataChannel、simulcast、TCP candidate。
- Firefox: simulcast RID/SSRC、DataChannel、WHEP play。
- Safari: H264/Opus 基础播放、WHIP/WHEP、candidate trickle。

服务端/库：

- ZLMediaKit：WHIP/WHEP、ZLMRTCClient、P2P signaling、WebRTC over TCP。
- Pion：WHIP/WHEP、DataChannel、NACK/RTX。
- GStreamer：webrtcbin publish/play。
- Janus：SDP fixture、simulcast、RTCP feedback。

协议互转：

- RTSP -> WebRTC。
- RTMP -> WebRTC。
- RTP/GB28181 -> WebRTC。
- WebRTC -> RTSP/RTMP/HLS/fMP4/HTTP-FLV。

弱网：

- `tc netem` 或 Windows 等价工具记录 1/5/10/20% loss。
- burst loss、reorder、jitter、bandwidth cap。
- 记录首帧耗时、freeze 次数、恢复时间、RTX hit/miss。

## 5.6 Fixture 与 corpus

新增 corpus：

- ZLM SDP：offer、simulcast、Janus。
- 浏览器 SDP：Chrome、Firefox、Safari。
- WHIP/WHEP HTTP requests：POST、PATCH、DELETE、错误 content-type。
- ICE candidate：host、srflx、relay、tcp passive、malformed。
- RTP：H264/H265/VP8/Opus/G711、RID、repaired RID、TWCC。
- RTCP：NACK、TWCC、REMB、PLI、FIR、SR、RR、BYE、unknown。
- DataChannel：text、binary、large message、invalid close order。

## 5.7 Fuzz 与 property tests

Fuzz target：

- SDP preprocess。
- ICE candidate fragment parser。
- WHIP/WHEP URL parser。
- RTP extension parser。
- RTCP feedback parser。
- TCP framing parser。
- DataChannel control message parser。

Property tests：

- RTP seq unwrap/reorder。
- NACK bitmask encode/decode。
- TWCC seq/time delta encode/decode。
- RID fallback stable mapping。
- URL parse/render roundtrip。
- session registry start/stop/list idempotency。

## 5.8 安全与资源上界

- Client URL 默认拒绝 private/loopback/link-local。
- P2P signaling host 也走 SSRF 防护。
- HTTP response `Location` 只能跟随同源或显式允许的远端。
- SDP/candidate/DataChannel/body 限长。
- 重连退避有上限。
- P2P room keeper 数量有上限。
- client job 终态允许覆盖，活跃态返回冲突。

## 5.9 测试要求

运行：

```powershell
cargo test -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-property-tests
```

手工/ignored 矩阵记录：

- Chrome WHIP/WHEP。
- ZLMediaKit WHIP/WHEP 互通。
- ZLMRTCClient publish/play/echo。
- Pion DataChannel echo。
- GStreamer publish/play。
- 10% loss 下 NACK/RTX 恢复。

## 完成后检查

- 文档补齐互操作命令和实际结果。
- fuzz corpus 纳入仓库。
- client/P2P/DataChannel 关闭路径释放所有资源。

