# Phase 03 — `cheetah-rtp-module`、REST API 与协议桥接

- **状态**: 已完成
- **范围**: 把 RTP server/client 接入 engine、控制面与桥接路径，形成 ABL 风格的 `openRtpServer/startSendRtp` 能力
- **完成标准**: 本地流可稳定转推为 RTP，RTP 收流可稳定转成 RTSP/RTMP/HLS，支持主动/被动和双工

---

## 3.1 `cheetah-rtp-module`

module 负责：

- `[x]` RTP session service（`RtpHttpService` + `RtpDriverHandle`）
- `[x]` 端口、会话、流名、租约分配（`acquire_publisher` + `client_targets` map）
- `[x]` 与 `StreamManager` 的发布订阅桥接（`run_ingress_worker` + `run_egress_session`）
- `[x]` 权限校验、配置应用、资源释放（`apply_config` -> `ModuleRestartRequired`，`stop` -> cancel token）

要求：

- `[x]` 支持显式 `app/stream` 和默认 `/live/{ssrc}`（HTTP handler + RTP core auto-create）
- `[x]` 支持 `disableVideo`、`disableAudio`（`disableVideo` -> `RtpTrackFilter::OnlyAudio`，`disableAudio` -> `OnlyVideo`，否则透传 `onlyAudio`）
- `[x]` 支持 `recv_app/recv_stream` 双工接收路径（aliases 在 `/server/create` 与 `/server/stop` 都接受）
- `[x]` 同一接收目标默认禁止重复占用（`PublisherOptions` 走默认独占租约）

---

## 3.2 REST API

固定以下语义：

- `[x]` `POST /api/v1/rtp/server/create`
- `[x]` `POST /api/v1/rtp/server/stop`
- `[x]` `POST /api/v1/rtp/client/start`
- `[x]` `POST /api/v1/rtp/client/stop`
- `[x]` `POST /api/v1/rtp/client/create`（额外，对齐 SMS 多目标 `senderInfos`）

请求字段要兼容 ABL：

- `[x]` `transport` 或兼容 `enable_tcp`/`is_udp`（透传到 `socketType`）
- `[x]` `payload`（`payloadType` 中 ps/ts/es/ehome/xhb/jtt1078 全覆盖）
- `[x]` `payloadType`
- `[x]` `tcpHeaderType`（解析为 `RtpTcpFraming` 后注入到 driver）
- `[x]` `app`、`stream_id` 或 `stream`（`extract_app_alias` + `extract_stream_alias` 在 GB28181 module；RTP module 接受 `appName/app/recv_app/recvApp` + `streamName/recvStreamId/recv_stream/recvStream/ssrc`）
- `[x]` `dst_url`、`dst_port`、`src_port`（`peerIp/dst_url`、`peerPort/dst_port` 别名都接受；`src_port` 由本地 driver 持有）
- `[x]` `recv_app`、`recv_stream`
- `[x]` `enable_hls`、`enable_mp4`（advisory，作为响应字段回显）

---

## 3.3 协议桥接

本阶段完成：

- `[x]` RTSP -> RTP（订阅方通过 engine 拉 RTSP 流，再 mux 后从 RTP 出口发送）
- `[x]` RTMP -> RTP
- `[x]` HLS/内部流 -> RTP
- `[x]` RTP -> RTSP（RTP ingress 进 engine，RTSP module 订阅播放）
- `[x]` RTP -> RTMP
- `[x]` RTP -> HLS

要求：

- `[x]` 桥接前后统一通过 `AVFrame + TrackInfo`
- `[x]` 不因协议桥接复制一套私有时间戳逻辑（统一走 `cheetah-codec` 时间归一化）
- `[x]` 发送端支持 `ForceSendingIFrame` 风格的关键帧启动优化（`ParameterSetCache::prepend_to_annexb_keyframe` + `BootstrapPolicy::live_tail`）

---

## 3.4 生产化细节

需要显式实现：

- `[x]` 发布前 bounded frame cache（`ActiveIngressSession::pending_frames` + `publish_frame_cache_frames` 上界）
- `[x]` 空闲超时与主动清理（`session_idle_timeout_ms` + `RR timeout` for senders）
- `[x]` per-session 诊断和统计（`RtpCoreDiagnostic`：`SequenceGap` / `SourceAddressChanged` / `OversizedPayload` 等）
- `[x]` `save_gb28181_rtp` 调试落盘开关（`RtpModuleConfig::save_debug_payload`）

---

## 3.5 测试

需要补齐：

- `[x]` module 集成测试：创建/停止 RTP recv/send、端口分配、重复流冲突、双工收发（`test_rtp_module_factory` 等 10 用例）
- `[x]` REST 测试：字段兼容、非法组合、资源回收（`test_socket_type_numeric_compat`、`test_transport_mode_aliases`、`test_payload_mode_case_insensitive`、`test_parse_connection_type_string_and_numeric`、`test_parse_only_audio_filter_modes`、`test_parse_payload_mode_includes_jtt1078_and_xhb`）
- `[x]` E2E：RTSP/RTMP/HLS -> RTP 与 RTP -> RTSP/RTMP/HLS（通过 engine `StreamManager` 完成；`cheetah-server --features rtp,gb28181` 集成构建测试）

完成后检查：

```bash
cargo clippy -p cheetah-rtp-module --tests
cargo test -p cheetah-rtp-module
```
