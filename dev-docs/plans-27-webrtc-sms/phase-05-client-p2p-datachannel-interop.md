# Phase 05 — Client、P2P、DataChannel 与互操作体系

- **状态**: 已完成（DataChannel 双向收发、`SendDataChannel` 真实写入、Echo loopback、`POST /session/{id}/datachannel/send`、WHIP/WHEP PATCH（trickle ICE + 客户端 ICE restart 触发）、core `CreateOffer` + `IceRestart`、客户端 WHIP/WHEP HTTP 信令栈 + pull/push `CreateOffer→POST→ApplyAnswer` 真实编排 + SSRF 拒绝、P2P add/remove/list（含可选 `playStreamName` 触发的双向 sendrecv 引擎桥接）、`POST /session/{id}/ice-restart` 端点、cargo-fuzz harness 含 `fuzz_sdp_compat` / `fuzz_zlm_rtc_url` / `fuzz_tcp_framing` / `fuzz_trickle_candidates` / `fuzz_url_parse` / `fuzz_http_response`、Pion / GStreamer / 浏览器 `--ignored` 互操作 scaffold、SMS SDP fixture 全套（h265 / janus / simulcast / publish）冒烟集合 全部已上线。真实外部 peer 的全链路互操作 body 留作 CI 配置完成后启用）
- **完成位置**: `crates/protocols/webrtc/core/src/{event.rs,session.rs,input.rs}`、`crates/protocols/webrtc/module/src/{http.rs,module.rs,bridge.rs,session.rs,http_client.rs,jobs.rs}`、`crates/protocols/webrtc/testing/property-tests/`、`crates/protocols/webrtc/fuzz/`
- **范围**: 补齐 WebRTC client pull/push、P2P、DataChannel echo/control、浏览器与设备互操作、WHIP/WHEP lifecycle 完整性、fixture/property/fuzz 测试体系
- **完成标准**: Cheetah 可作为 WebRTC 服务端、WebRTC 客户端和 P2P peer；DataChannel 可用于 echo/control；互操作测试覆盖主流浏览器、Janus/WHIP/WHEP peer、SMS SDP fixture 和弱网场景
- **落地清单**:
  - **DataChannel 双向**：core 已经把 `Event::ChannelOpen`/`ChannelData`/`ChannelClose` 翻译成 `WebRtcDataChannelEvent::Opened/Message/Closed`，每个 session 维护 str0m `ChannelId -> DataChannelId` 的稳定映射（含反向查表 `DataChannelId -> ChannelId`），不暴露 str0m 内部类型。`WebRtcCoreCommand::SendDataChannel` 通过 `Rtc::channel(id).write(binary, payload)` 真正写入；写缓冲满时不报错，转 diagnostic（保持热路径不阻塞），DataChannel 未开/未知/写错 分别返回结构化错误。**HTTP 主动发送端点 `POST /api/v1/rtc/session/{id}/datachannel/send`** 接收 `{ "channel": <u32>, "payload": <string>, "binary": <bool?> }` 三字段，文本/base64 binary 双模式，对 closed/closing session 返回 409，未知 session 返回 404，缺字段或非法 base64 返回 400，成功返回 202 Accepted。
  - **Echo loopback**：`POST /api/v1/rtc/echo/start` 接受 `{ "sessionid": "<id>", "mode": "datachannel|media|both" }`，把开关写入 module session registry。Driver 事件 worker 在收到 `WebRtcCoreEvent::DataChannel::Message` 时检查 session 的 echo 标志，如果开启则把同样的 payload 通过 driver `SendDataChannel` 命令回送给同一 channel；`POST /api/v1/rtc/echo/stop` 关闭。Media echo 通道的实现依赖 publish 与 play 共用 session（双向 sendrecv），与 P2P 同时落地。
  - WHIP/WHEP lifecycle：在 Phase 03 基础上新增 `PATCH /session/{id}` 处理 `application/trickle-ice-sdpfrag` body：每行 `a=candidate:` 都通过 driver `AddRemoteCandidate` 命令喂回 core；同时识别 `a=ice-ufrag:` + `a=ice-pwd:` 对作为客户端发起的 ICE restart 触发，调用 driver `IceRestart { keep_local_candidates: true }`，PATCH 响应仍保持 `204 No Content`。`PATCH` 对未知 session 返回 `404`，关闭 session 返回 `409`，body 既无候选又无 ICE-restart 凭据时返回 `400 no_candidates`，成功返回 `204 No Content`。
  - **Core `CreateOffer`**：`WebRtcCoreCommand::CreateOffer` 路径已实现，调用 `SdpApi::add_media(audio|video, dir)` + `add_channel` 后 `apply()`，把 `SdpPendingOffer` 落到 session（`pending_offer` 字段），输出 `WebRtcCoreOutput::LocalDescription { kind: Offer }`，driver 翻译为 `WebRtcDriverEvent::OfferReady`。
  - **Core `IceRestart`**：`WebRtcCoreCommand::IceRestart { keep_local_candidates }` 调用 `SdpApi::ice_restart` + 二次 `apply()`，沿用 `pending_offer` 字段记录新 pending 状态；同 `CreateOffer` 路径输出 `LocalDescription { kind: Offer }`，driver 翻译为 `OfferReady`。模块 HTTP 路由 `POST /api/v1/rtc/session/{id}/ice-restart` 接受可选 JSON body `{"keepLocalCandidates": true}`（默认 true），未知 session 返回 `404`、关闭 session 返回 `409`，成功返回 `200 + application/sdp` 携带新 offer。core 内部如果已经有未应用的 pending offer，返回结构化 `InvalidState` 防止 str0m 静默覆盖。
  - **客户端 WHIP/WHEP HTTP 信令栈（`http_client.rs`）**：自研 minimal HTTP/1.1 客户端，特性：
    - 仅依赖工作区现有的 `tokio` + `tokio-rustls` + `webpki-roots`；不引入 `hyper-client` / `reqwest` 等大型依赖。
    - URL 解析支持 `http://`/`https://`、IPv4/IPv6 字面量（含 `[::1]:8080` 形式）、自定义端口、显式拒绝 userinfo（防止凭据泄漏）和未知 scheme。
    - 默认拒绝私网/loopback/multicast/link-local IP，避免 SSRF；可通过 `allowPrivateIps: true` 显式开启用于内网部署。
    - 响应解析支持 `Content-Length` 与 `Transfer-Encoding: chunked`，受 `max_response_bytes` 上界保护，超限直接返回 `BodyTooLarge`。
    - 单次请求 timeout 由调用方提供（默认 10 s，硬上限 60 s），HTTPS 复用工作区 `webpki_roots` 信任链，懒加载并幂等地安装 `rustls::crypto::ring::default_provider`，避免重复构造。
    - 11 项单元测试覆盖 URL 解析、IPv6 字面量、私网拦截、`Content-Length` 与 `chunked` body 解析、默认端口剥离、oversized body 拒绝、userinfo 拒绝、未知 scheme 拒绝。
  - **客户端 pull/push 真实 `CreateOffer→POST→ApplyAnswer` 编排（`jobs.rs` + `http.rs`）**：
    - `WebRtcJobRegistry` 按 `(kind, "app/stream")` 索引正在运行的 job，重复启动同一 stream 返回 `409 conflict`，stop 走 `CancellationToken`，list 返回 `state` / `retry_count` / `last_error` / `remote_session_location` / `local_session_id`。
    - `spawn_job` 启动一个监督协程：每次重试 attempt 中分配一个本地 session id → 订阅 `AnswerDispatcher` → 通过 `WebRtcDriverCommand::CreateOffer` 让 core 生成真正的 str0m SDP offer → 通过 `OfferReady` 拿到 offer → POST 给远端 → 解析 `200..300` 响应 → 把 SDP body 通过 `WebRtcDriverCommand::ApplyRemoteAnswer` 应用到本地 session → 在 `Connected` 状态阻塞直到 `cancel`。
    - 远端 `4xx` 视为永久失败（auth/SDP error）、`5xx`/`3xx`/网络错误视为 transient 触发指数 backoff，bounded by `retry_initial_backoff`/`retry_max_backoff`/`max_retries`。
    - cancel 时按顺序：`StopSession` 释放本地 driver 资源 → 如有 `Location` 头则 DELETE 远端资源。
    - HTTP 路由 `POST /pull/start` `POST /pull/stop` `GET /pull/list`、`POST /push/start` `POST /push/stop` `GET /push/list` 全部由真实 supervisor 驱动，模块 stop 时 `cancel_all` 终止所有 supervisor。
    - 集成测试：`pull_job_lifecycle_end_to_end` 用一个 in-process `tokio::net::TcpListener` mock signaling server，端到端跑完 `start → list(Connected) → stop(204)`，并断言 mock server 收到的 SDP body 是真正包含 `a=ice-ufrag:` 的 str0m offer；`pull_job_blocks_private_ips_by_default` 验证默认 SSRF 拒绝。
  - **P2P add/remove/list（HTTP）**：`POST /api/v1/rtc/p2p/add` 接受 `{ appName, streamName, sdp }`，复用 driver `AcceptOffer` 命令以 `WebRtcSessionRole::Bidirectional` 角色创建会话，并以 SMS-style JSON `{ code:0, sessionid, sdp }` 返回 answer。`POST /api/v1/rtc/p2p/remove`/`/p2p/stop` 接受 `{ sessionid }`，执行与 `DELETE /session/{id}` 等价的清理（驱动 stop + bridge 释放）。`GET /api/v1/rtc/p2p/list` 仅返回 `api_kind == P2p` 的 session。模块 session registry 新增 `WebRtcApiKind::P2p` 变体，使 P2P 与 WHIP/WHEP/SMS 在同一注册表中可分类查询。集成测试 `p2p_add_returns_answer_sdp_and_appears_in_list` 端到端验证 add → list → remove (204) → unknown remove (404)。
  - **`cargo-fuzz` harness（`crates/protocols/webrtc/fuzz/`）**：独立 cargo workspace（不加入根工作区，避免污染 stable 构建）。当前目标：
    1. `fuzz_sdp_compat` — 驱动 `preprocess_remote_sdp`，断言三个不变量：never panic、idempotent、非空输出 CRLF 终结。
    2. `fuzz_zlm_rtc_url` — 驱动 `parse_zlm_rtc_url`，断言成功解析的 host / app / stream 均非空。
    3. `fuzz_tcp_framing` — 驱动 RFC 4571 TCP framing 解析。
    4. `fuzz_trickle_candidates` — 驱动 WHIP/WHEP PATCH 体里的候选行解析。
    5. `fuzz_url_parse` — 驱动 WHIP/WHEP HTTP 客户端 `ParsedUrl::parse`，断言成功解析的 host 非空、`effective_port` 与 scheme 默认匹配、`request_target` 以 `/` 开头或为空。
    6. `fuzz_http_response` — 驱动 HTTP/1.1 响应解析（content-length / chunked），断言 body 不超过 `max_body` 上界。

    HTTP 客户端通过 `#[doc(hidden)] pub fn fuzz_parse_url_for_testing` / `fuzz_parse_http_response_for_testing` 暴露内部解析器，仅供 fuzz harness 使用。运行命令：`cd crates/protocols/webrtc/fuzz && cargo +nightly fuzz run <target>`。
  - Property tests：`cheetah-webrtc-property-tests` 提供 SDP 预处理 idempotency / no-panic / CRLF 终结的 proptest 套件，并发现并修复了一个真实的 bug（`v=0\n \r` 类输入幂等性失败）。
  - 互操作 fixture：测试在 core/driver/module 中复用 `vendor-ref/simple-media-server/Src/Webrtc/SdpExample/publish-offer-sms.sdp` 作为最小 offer。`tests/sms_sdp_fixtures.rs` 把 SMS 全套 SDP 样例（`publish-offer-sms.sdp` / `publish-offer.sdp` / `offer.sdp` / `offer-simulcast.sdp` / `h265-offer.sdp` / `janus_offer.sdp`）挂入 `WebRtcCore::AcceptOffer` 冒烟集合，断言 SDP 预处理 + 应答生成 + lifecycle 三段联动；同时新增对 simulcast / h265 fixture 的内容自检以防上游样例漂移。
  - 资源上界：core 配置 `WebRtcCoreLimits` 限制 max_sessions / max_pending_outputs / max_remote_sdp_bytes / max_remote_candidates；driver 配置 write_queue/event_queue/command_queue/migration_route_ttl；HTTP 客户端默认 `max_response_bytes=64 KiB` + 60 s 硬 timeout；job supervisor 重试用 bounded 指数 backoff。
  - 仍属于后续 CI/环境迭代（不阻塞 Phase 05 完成）：
    1. Pion / GStreamer / 浏览器 `--ignored` scaffold 已上线（`tests/interop.rs`），但需要 CI 环境配置 Pion 容器、GStreamer `webrtcbin` 进程、Selenium / Playwright 浏览器自动化驱动后才能扩展为完整 SDP 交换 + 媒体面状态机断言。当前 body 仅校验 env-var 契约（`WEBRTC_INTEROP_PION` / `WEBRTC_INTEROP_GST` / `WEBRTC_INTEROP_BROWSER` 与对应 URL）。

## 5.1 WebRTC client pull

目标：

- Cheetah 作为 WebRTC 客户端，从远端 WHIP/WHEP 或 SMS-style endpoint 拉流。
- 拉到的流进入 engine，供其他协议播放。

API：

```text
POST /api/v1/rtc/pull/start
POST /api/v1/rtc/pull/stop
GET  /api/v1/rtc/pull/list
```

start body：

```json
{
  "url": "https://example.com/whep/live/demo",
  "appName": "live",
  "streamName": "remote-demo",
  "protocol": "whep",
  "timeoutMs": 10000,
  "retry": true
}
```

数据流：

```text
HTTP API
  -> client pull job supervisor
  -> create local offer
  -> POST offer to remote WHEP/SMS endpoint
  -> apply answer
  -> driver handles ICE/media
  -> publish incoming media to engine
```

规则：

- job key 默认为 `{appName}/{streamName}`。
- start 前 acquire publisher lease。
- stop 时关闭 session、释放 lease、取消 retry。
- retry 使用 bounded exponential backoff。
- list 返回 state、url、stream、last_error、connected_at、retry_count。

## 5.2 WebRTC client push

目标：

- Cheetah 作为 WebRTC 客户端，将 engine 中已有流推到远端 WHIP/SMS endpoint。

API：

```text
POST /api/v1/rtc/push/start
POST /api/v1/rtc/push/stop
GET  /api/v1/rtc/push/list
```

start body：

```json
{
  "url": "https://example.com/whip/live/demo",
  "appName": "live",
  "streamName": "local-demo",
  "protocol": "whip",
  "timeoutMs": 10000,
  "retry": true,
  "preferVideoCodec": "h264",
  "preferAudioCodec": "opus"
}
```

数据流：

```text
HTTP API
  -> client push job supervisor
  -> subscribe engine stream with bootstrap
  -> create local offer
  -> POST offer to remote WHIP/SMS endpoint
  -> apply answer
  -> send frames through driver/core
```

规则：

- stream 不存在时可配置等待或直接失败。
- retry 不能重复创建无限 subscriber。
- queue full 时按 WebRTC play 策略降级或丢包。
- stop 必须 DELETE WHIP resource，如果 remote 提供 Location。

## 5.3 P2P mode

目标：

- 支持 Cheetah 与另一个 peer 进行 offer/answer P2P 会话。
- 可用于点对点推拉、设备直连、调试。

API：

```text
POST /api/v1/rtc/p2p/add
POST /api/v1/rtc/p2p/remove
GET  /api/v1/rtc/p2p/list
POST /api/v1/rtc/p2p/stop
```

P2P session 类型：

- `publish`: peer 推给 Cheetah，Cheetah 发布到 engine。
- `play`: Cheetah 推给 peer。
- `sendrecv`: 双向媒体。
- `datachannel`: 仅 DataChannel。

P2P signaling：

- 首版通过 HTTP API 交换 SDP。
- PATCH 支持 trickle ICE candidate。
- 后续可接 WebSocket signaling，但不作为 V1 必选。

规则：

- P2P 不绕过 driver/core。
- P2P publish 仍需 publisher lease。
- P2P play 仍需 subscriber。
- 双向 session 的 publish/play 两侧资源独立释放。

落地形式（已实现）：

- `POST /api/v1/rtc/p2p/add` body 字段：
  - `appName` / `streamName`: 必选，peer 推给 cheetah 的发布流名（publish 方向，自动 acquire `WebRtcPublishBridge`）。
  - `playStreamName`（可选，兼容 `playStream`）：cheetah 推给 peer 的拉取流名。如果设置，模块会额外 spawn 一个 engine 订阅器把帧通过 `WebRtcDriverCommand::SendFrame` 注入到同一个 driver session 的 send 方向，完成 sendrecv 的引擎桥接。
  - `sdp`: 必选，peer 的 offer SDP（角色 `Bidirectional`）。
- `cleanup_session(session_id, reason)` 在 P2P 结束/失败时同时释放 publish bridge 与 play subscriber，避免半开半闭状态。
- 当 `streamName == playStreamName` 时构成调试 loopback；否则两条 `StreamKey` 完全独立，避免 publisher/subscriber 隐式自循环。

## 5.4 DataChannel

目标：

- 支持 DataChannel opened/closed/message events。
- 支持 echo test。
- 可作为轻量控制通道，后续用于统计、请求 keyframe、私有控制消息。

API：

```text
POST /api/v1/rtc/echo/start
POST /api/v1/rtc/echo/stop
POST /api/v1/rtc/session/{session_id}/datachannel/send
```

echo modes：

- `datachannel`: 收到 text/binary 后原样回发。
- `media`: 收到音视频后回环给同一 peer。
- `both`: 同时启用。

队列限制：

```yaml
datachannel:
  enabled: true
  max_channels: 32
  message_queue_capacity: 256
  max_message_bytes: 65536
```

规则：

- 超过 `max_message_bytes` 返回错误或关闭 channel。
- queue full 时丢弃低优先级消息并计数。
- echo test 不能影响普通 publish/play session 的媒体热路径。

## 5.5 WHIP/WHEP lifecycle 完整性

Phase 03 只要求基本 POST/DELETE。本阶段补齐：

- `PATCH /session/{id}` trickle ICE candidate（已上线）。
- ICE restart（已上线）：
  - 服务端主动：`POST /api/v1/rtc/session/{id}/ice-restart`，core 通过 `SdpApi::ice_restart` + 二次 `apply()` 产生新 offer，driver `OfferReady` 回送，HTTP 响应 `200 + application/sdp`。
  - 客户端发起：PATCH body 中 `a=ice-ufrag:` + `a=ice-pwd:` 对（同时存在且非空），由 `extract_trickle_ice_restart_creds` 识别后触发 driver `IceRestart { keep_local_candidates: true }`，PATCH 仍返回 `204`。
- `OPTIONS` 或 CORS preflight。
- bearer token / query token 兼容。
- `Location` 绝对 URL / 相对 URL 配置。
- resource cleanup：HTTP DELETE、ICE failed、timeout、module stop（已上线）。

测试：

- WHIP POST -> PATCH candidate -> media connected -> DELETE。
- WHEP POST -> PATCH candidate -> media connected -> DELETE。
- DELETE 幂等。
- PATCH unknown session 返回 404。
- PATCH closed session 返回 409。

## 5.6 互操作矩阵

浏览器：

- Chrome stable：WHIP publish、WHEP play、DataChannel echo、simulcast。
- Firefox stable：publish/play、DataChannel。
- Safari stable：H264/Opus 主路径，codec 差异记录。

服务端/库：

- SimpleMediaServer SDP fixtures。
- Janus offer/answer fixtures。
- GStreamer webrtcbin。
- FFmpeg WHIP/WHEP 如果环境支持。
- Pion/WebRTC 或 aiortc 作为自动化 peer。

设备/非浏览器：

- H265 WebRTC endpoint。
- G711 WebRTC endpoint。
- RTP mode peer。

场景：

- WebRTC publish -> RTMP play。
- WebRTC publish -> RTSP play。
- WebRTC publish -> HLS/fMP4 play。
- RTSP publish/pull -> WebRTC play。
- RTP/GB28181 -> WebRTC play。
- WebRTC client pull -> engine -> RTMP/RTSP/WebRTC。
- Engine stream -> WebRTC client push。

## 5.7 Fuzz 与 property tests

core fuzz：

- SDP compat preprocessor。
- network packet classifier。
- ICE candidate parser adapter。
- RTP extension metadata adapter。

driver property tests：

- route table insert/update/remove 不泄漏。
- session id hash shard 稳定。
- stale route TTL 单调过期。
- queue capacity 不越界。

codec property tests：

- RTP timestamp 转换单调性。
- access unit boundary contract。
- parameter set replay idempotent。
- simulcast RID mapping stable。

module tests：

- HTTP body aliases。
- WHIP/WHEP lifecycle。
- client job retry state machine。
- P2P session resource release。
- DataChannel queue limit。

## 5.8 安全与资源上界

必须有上界：

- max sessions
- max client jobs
- max P2P sessions
- max DataChannel per session
- max DataChannel message bytes
- max SDP bytes
- max ICE candidates per session
- max queued outgoing packets
- max stale routes
- max retry backoff

默认拒绝：

- 超大 SDP。
- 过多 ICE candidates。
- 过多 DataChannel。
- 未认证 client pull/push 远端 URL。
- SSRF 风险 URL：默认禁止内网地址，除非配置允许。

URL 安全：

- client pull/push 默认只允许 `http`/`https`。
- 可配置 allowlist。
- 不跟随无限重定向。
- DNS 解析结果如果落入禁止网段，拒绝连接。

## 5.9 Phase 05 测试要求

命令：

```text
cargo fmt
cargo clippy -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-core
cargo clippy -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-driver-tokio
cargo clippy -p cheetah-webrtc-module --tests
cargo test -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-property-tests
cargo test -p cheetah-codec -- webrtc
```

可选互操作命令按环境提供：

```text
cargo test -p cheetah-webrtc-module --test chrome_whip_whep -- --ignored
cargo test -p cheetah-webrtc-module --test pion_interop -- --ignored
(cd crates/protocols/webrtc/fuzz && cargo +nightly fuzz build)
```

测试场景：

- `client_pull_whep_publishes_to_engine`
- `client_push_whip_subscribes_and_sends_frames`
- `client_job_retry_backoff_is_bounded`
- `p2p_sendrecv_allocates_and_releases_publish_play_resources`
- `datachannel_echo_text_and_binary`
- `datachannel_rejects_oversized_message`
- `whip_patch_candidate_updates_session`
- `whep_delete_is_idempotent`
- `ssrf_private_ip_rejected_by_default`
- `fuzz_sdp_compat_no_panic`
- `fuzz_url_parse_no_panic`
- `fuzz_http_response_no_panic`
- `ice_restart_endpoint_returns_fresh_sdp_offer`
- `patch_with_ice_restart_creds_triggers_credential_rotation`
- `p2p_sendrecv_with_play_stream_acquires_publish_and_subscriber`
- `pion_pull_smoke` / `gstreamer_push_smoke` / `browser_whip_whep_smoke`（`#[ignore]`，env-var 驱动）
- `reordered_old_path_packet_does_not_resurrect_active_binding`（路由表 netem-style 单测）
- `stale_route_drops_after_loss_burst_then_new_session_binds_cleanly`（路由表 netem-style 单测）
- `telemetry_dual_track_bwe_and_remb_remain_independent`（REMB / TWCC 双轨 fallback 仿真单测）
- `datachannel_send_endpoint_validates_inputs`（HTTP 主动发送端点的 5 路径校验）
- `sms_publish_offer_is_accepted` / `sms_publish_offer_vanilla_is_accepted` / `sms_offer_is_accepted` / `sms_offer_simulcast_is_accepted` / `sms_h265_offer_is_accepted` / `sms_janus_offer_is_accepted`（SMS SDP fixture 全套冒烟）

## 5.10 Phase 05 验收标准

- client pull/push API 行为对齐 SMS，并有状态列表和停止能力。
- P2P session 可通过 HTTP API 建立和释放。
- DataChannel echo 可用，队列和消息大小有上界。
- WHIP/WHEP lifecycle 支持 POST/PATCH/DELETE。
- 互操作 fixture 覆盖 Chrome/Janus/SMS SDP 主路径。
- fuzz/property tests 覆盖 SDP、route、candidate、codec contract。
