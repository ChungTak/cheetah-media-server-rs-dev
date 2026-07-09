# Phase 05 Follow-up — P2P Signaling

- **状态**: 第十轮已落地（`AnswerDispatcher` ↔ `DispatcherOfferWaiter` 真接入 + 可选 inbound signaling server）。Phase 05 计划项全部落地，剩下的真实 ZLMediaKit 互操作字段差异由 Phase 06 互操作 lab 负责验证。

## 已完成（Phase 05 follow-up 第十轮）

- `crate::http::AnswerDispatcher::subscribe_p2p`：新 `pub(crate)` API，订阅一次 `OfferReady` 并通过适配器把内部 `AnswerOutcome` 映射为公共 `crate::p2p::DispatcherOfferOutcome`，避免泄漏 `pub(crate)` 类型到 P2P 模块。
- `P2pClientJobRuntime::answer_dispatcher`（`pub(crate)`）：HTTP 入口构造 runtime 时把 module 的 dispatcher 传进来；`P2pClientObserver` 用它替换早先的 `InlineOfferWaiter`，bridge 现在拿到真实 SDP。
- 旧 `InlineOfferWaiter` 删除。
- `module/src/p2p/server.rs` 新增 inbound signaling server adapter：
  - `run_server(listener, SignalingServerConfig, ConnectionHandler, cancel)`：基于 `tokio-tungstenite::accept_async` 的 WebSocket server，自带 `accept_timeout`、`max_connections` 上界（带 RAII drop guard 保证容量计数器可靠回落）。
  - 每条入站 WebSocket 自动包成 `WebSocketP2pTransport`，handler 通过 `Arc<dyn Fn(InboundConnection, WebSocketP2pTransport) -> BoxFuture<()>>` 异步消费。
  - `WebSocketP2pTransport` 改为类型擦除（`Box<dyn Sink/Stream>`）：同一结构体既能包客户端 `MaybeTlsStream<TcpStream>`（`new`），也能包服务端 `TcpStream`（`from_server_stream`）。
  - 2 条单元测试：accept ↔ handler ↔ ping 消息透传；`max_connections=1` 下第二条连接被 drop。
- 公共导出：`run_signaling_server / SignalingServerConfig / SignalingServerError / SignalingServerHandler / SignalingServerInbound`。

## 已完成（Phase 05 follow-up 第九轮）

- `module/src/p2p_jobs.rs` 新增 P2P client job runner：
  - `P2pClientJobRegistry`（`new / list / stop / stop_all`）+ `P2pClientJobRuntime`（registry / keepers / driver / lifecycle / engine / parent_cancel 一束打包）。
  - `P2pClientJobRequest`：URL、kind、`allow_private_ips`、可选 signaling URL override、connect / offer 超时、supervisor 重试旋钮。
  - `spawn(...)`：解析 URL、跑 SSRF + plan 校验、添加 keeper、注册 snapshot、`tokio::spawn` 调用 `run_supervisor_with_hub` + `WebSocketTransportFactory` + 内部 `P2pClientObserver`（自动 `hub.attach` + `run_bridge_with_lifecycle` + 把 `LifecycleDispatcher` 当 lifecycle 源）。任务结束后自动清理 keeper + registry。
  - `P2pClientJobSnapshot`：`session_id / kind / url / state(Pending|Running|Stopped|Failed) / last_error / signaling_url / peer_room_id / stream_key`。
  - 2 条单元测试：registry round-trip（注册 / 重复 / stop）、`stop_all` drain。
- `WebRtcModule` 新增 `p2p_jobs: Arc<P2pClientJobRegistry>`，`stop()` 调用 `stop_all()` 清理后台任务。
- `WebRtcHttpService::handle_job_start` P2P 分支升级：driver / engine 都已绑定时调用 `p2p_jobs::spawn` 返回 `200` + `session_id / kind / state / signaling_url / peer_room_id / stream_key`；URL 校验失败仍 `400 p2p_invalid_url`；冲突 `409`；spawn 失败 `503`；driver 未就绪保留 `501` + extras 兜底。
- 新 HTTP 路由 `/api/v1/rtc/p2p/client/list`（返回所有 in-flight P2P client jobs）和 `/api/v1/rtc/p2p/client/stop`（按 `session_id` 取消任务）。
- 集成测试：`pull_start_p2p_signaling_returns_200_with_session_id`（默认 host 跑 200 + session id）、`pull_start_p2p_with_loopback_host_returns_400`（SSRF 拒绝）、`pull_start_p2p_loopback_with_allow_private_ips_returns_200`（opt-in 后 200）、`p2p_client_list_and_stop_round_trip`（list、stop、bad json、missing field、unknown id 全覆盖）。

## 已完成（Phase 05 follow-up 第八轮）

- 工作区新增 `tokio-tungstenite 0.29` workspace dep（`rustls-tls-webpki-roots` 特性，复用 `rustls 0.23` + `webpki-roots 0.26`）。`module/Cargo.toml` 透传引用。
- `module/src/p2p/ws.rs` 落地 `WebSocketTransportFactory` + `WebSocketP2pTransport`：
  - 实现 `KeeperTransportFactory` 与 `P2pTransport`，把 schema 文本帧解码 / 编码完全藏在 transport 内。
  - `WebSocketTransportConfig`：`SignalingUrlPolicy` SSRF + `P2pDecoderConfig` 长度上界 + `connect_timeout` + 可选 `url_override`（让 keeper 配置无法表达的完整 URL 也能落到 transport）。
  - 自动响应 `Ping`、忽略 `Pong / Frame`、`Binary` 帧按 UTF-8 + JSON 解码（覆盖那些把文本当 binary 发的实现）。
  - `WebSocketCounters`（`messages_sent / messages_received / decode_errors`）+ `snapshot_websocket_counters` 暴露给测试与 metrics。
- 公共导出：`WebSocketTransportFactory / WebSocketTransportConfig / WebSocketP2pTransport / WebSocketCounters / WebSocketCounterSnapshot / WebSocketTransportError`，全在 `cheetah_webrtc_module::p2p::*`。
- 集成测试 `tests/p2p_websocket_transport.rs`：本地 `tokio-tungstenite::accept_async` 跑迷你 server，验证 transport `send / recv / close` round-trip 与 counters。
- 集成测试 `tests/p2p_websocket_supervisor.rs`：完整端到端 — 本地 ws server + `WebSocketTransportFactory` + `run_supervisor_with_hub` + `KeeperHubObserver` 跑 `run_bridge`，bridge 终态 `Bye`，supervisor 在 keeper remove 后退出（`KeeperRemoved` 或 `Stopped` 或 `GaveUp`）。
- `module/src/http.rs::handle_job_start` 重写 P2P 路径：调用 `plan_from_zlm_url` 跑 SSRF + 信令 URL 派生，成功时返回 `501 not_implemented` 但带结构化 extras（`signaling_url / peer_room_id / kind`）；失败时返回 `400 p2p_invalid_url` + 解析错误描述。`allowPrivateIps` 透传到 SSRF 策略。
- 新集成测试：`pull_start_rejects_p2p_signaling_with_501` 升级断言对照 extras；`pull_start_p2p_with_loopback_host_returns_400` 验证 SSRF 拒绝；`pull_start_p2p_loopback_with_allow_private_ips_returns_501` 验证 opt-in 后回到 501 + URL 命中 loopback。
- `http_json_status_with_extras` 新公共 helper：标准 `code/error/message` 字段 + 任意附加字段，避免 extras stomp 系统字段。

## 已完成（Phase 05 follow-up 第七轮）

- `module/src/p2p/lifecycle_dispatcher.rs` 新增 `LifecycleDispatcher`：实现 `BridgeLifecycleSource`，per-session mpsc channel（capacity 4），`deliver_connected / deliver_closed / forget` API；`deliver_closed` 自动移除 entry 防止泄漏。5 条单元测试。
- `WebRtcModule::lifecycle_dispatcher()` 公共 accessor + `WebRtcModule::run_driver_event_worker` 内部 `WebRtcCoreEvent::Lifecycle` 分发：`Connected → deliver_connected`，`Closed/Failed → deliver_closed`，中间状态 `Created/LocalDescriptionReady/Disconnected` 不向 bridge 透传。
- `module::tests::module_exposes_working_lifecycle_dispatcher` 集成测试：直接调用 module 的 dispatcher，订阅 + deliver Connected / Closed 都按预期工作。
- 公共导出：`LifecycleDispatcher`。

## 已完成（Phase 05 follow-up 第六轮）

- `module/src/p2p/bridge.rs` 新增 `BridgeLifecycleSource` async trait + `BridgeLifecycleEvent { Connected, Closed }`：
  - `run_bridge_with_lifecycle(...)`：在发 `CreateOffer` 之前订阅 lifecycle 通道，主循环按 `cancel / lifecycle / transport.recv()` 三路 `select!` 推进。`Connected` 触发 `P2pJobInput::DriverConnected`，job 从 `AwaitingAnswer` 走到 `Connected`；`Closed` 当作 transport error 让 job `Failed`。
  - `recv_lifecycle(rx: &mut Option<...>)`：源关闭后把 receiver 置空，select 该臂自动 pend 永不唤醒，避免 busy-loop。
  - `NoopLifecycleSource`：默认 `run_bridge` 用空源，行为与之前完全一致（向后兼容）。
- 新单元测试 `bridge_with_lifecycle_advances_job_to_connected`：模拟 server send answer → 注入 `Connected` lifecycle → send bye，断言 job 终态 `Bye`（而不是没看到 `Connected` 直接因 bye 退出）。
- 公共导出：`run_bridge_with_lifecycle` / `BridgeLifecycleSource` / `BridgeLifecycleEvent` / `NoopLifecycleSource`。

## 已完成（Phase 05 follow-up 第五轮）

- `module/src/p2p/supervisor.rs` 新增 `run_supervisor_with_hub(...)` 与 `KeeperHubObserver` async trait：
  - 把 `KeeperTransportFactory` 返回的 transport 自动包成 `KeeperHub<T>`，通过观察者回调 `on_hub_ready(snapshot, hub, hub_cancel)` 让上层 attach 多个 peer bridge。
  - 添加 registry 监视器：keeper 中途被移除时自动取消 hub，避免 `transport.recv()` 阻塞导致 supervisor 永远不返回。
  - dispatcher 与 observer 的生命周期：dispatcher 退出 → cancel hub → 等 observer 收尾 → close hub → 再走 reconnect / give-up 逻辑。
  - 与已有 `run_supervisor` 并存，保持向后兼容。
- 集成测试 `module/tests/p2p_pipeline.rs::supervisor_drives_hub_drives_bridge_pull_lifecycle`：完整跑通 supervisor → KeeperHub → run_bridge → RecordingDriverSink 的 pull lifecycle，最后 `registry.remove(key)` 触发 supervisor 返回 `KeeperRemoved`。
- `module/src/http.rs::handle_job_start`：识别 `webrtc://...?signaling_protocols=1` URL 并显式返回 `501 not_implemented`，附带跳转到 `plans-27-webrtc-zlm2/phase-05-p2p-signaling.md` 的指引，避免 P2P URL 静默走 WHIP/WHEP 的兼容路径并超时。
- 集成测试 `module/tests/module_lifecycle.rs::pull_start_rejects_p2p_signaling_with_501`：断言 P2P URL 走 `/pull/start` 时拿到 501 响应及包含 `signaling_protocols=1` / `WebSocket` 关键字的 message。

## 已完成（Phase 05 follow-up 第四轮）

- `module/src/p2p/hub.rs`：信令多路复用 hub。
  - `PeerKey { room_id, peer_id, transport_id }` + `from_header / from_message` 解构入站消息的 routing key。
  - `KeeperHub<T: P2pTransport>`：拥有底层 transport，把入站 `Message` 按 `PeerKey` 投递到 per-peer mpsc channel；`Closed / Error` 广播给所有已 attach 的桥接器。`run_dispatcher(cancel)` 单 task 跑读循环。
  - `HubBackedTransport<T>`：实现 `P2pTransport`，通过 hub 共享 transport `send`、独占 inbound mpsc。`close` 时自动 `detach` 释放 hub 槽位。
  - `KeeperHubConfig { peer_channel_capacity, max_peers }`，超容量返回 `KeeperHubError::CapacityExceeded`。
  - 7 条 hub 单元测试 + 1 条 hub↔bridge 端到端集成测试：单 hub、两个并发 bridge 完成完整 pull / push lifecycle，`hub.peer_count()` 在结束后归零。
- 公共导出：`KeeperHub / KeeperHubConfig / KeeperHubError / PeerKey / HubBackedTransport`。
- 修复 `bridge::run_bridge` 的 `tokio::select!` 加 `biased`，使 cancel 优先级稳定，避免 `transport closed` 与 cancel 的竞态；happy-path 测试改用 `bye/answer` 而非 `cancel` 作为终止信号，消除 paused-clock 下的不确定性。

## 已完成（Phase 05 follow-up 第一轮）

- `crates/protocols/webrtc/module/src/p2p/mod.rs`：模块入口，重新导出 `message` 与 `room` 子模块的核心类型。
- `crates/protocols/webrtc/module/src/p2p/message.rs`：完整 P2P wire schema：
  - `P2pMessage` 枚举：`CheckIn / CheckInOk / Offer / Answer / Candidate / Bye / Error / Ping / Pong / RoomList / Unknown`，每个变体附 `P2pMessageHeader`（`room_id / peer_id / transport_id`）。
  - `P2pStreamTuple { vhost, app, stream }` 严格序列化（默认 vhost = `__defaultVhost__`）。
  - `parse(raw, P2pDecoderConfig)` / `render(message)`，验证 message size、SDP size、candidate size、字段长度，错误返回 `P2pMessageError`（非 panic）。
  - 默认上界：`P2P_MAX_FIELD_BYTES=128`、`P2P_DEFAULT_MAX_MESSAGE_BYTES=1MB`、`P2P_DEFAULT_MAX_SDP_BYTES=64KB`、`P2P_DEFAULT_MAX_CANDIDATE_BYTES=1KB`。
  - 未知 `type` 解析为 `P2pMessage::Unknown { ty }`，不会 panic；`render` 拒绝重渲染未知 message，避免回放任意 type。
- `crates/protocols/webrtc/module/src/p2p/room.rs`：room keeper registry：
  - `P2pRoomKeeperRegistry::add / remove / list / list_rooms / set_status / len / is_empty`。
  - `P2pRoomKeeperConfig` 含 `server_host / server_port / room_id / vhost / app / stream / ssl`，自带 `validate()`（room_id 1..128、host 1..253、port != 0）。
  - `P2pKeeperState`: `Pending / Connecting / Registered / Reconnecting / Stopped / Failed`。
  - 每个 registry 默认 `P2P_DEFAULT_MAX_KEEPERS = 1024` 上限；超出返回 `LimitReached`。
  - `list_rooms()` 自动 dedupe，对齐 ZLM `mk_webrtc_list_rooms` 行为。
- 单元测试 14 条（message 9 + room 5）：覆盖 round-trip、未知 type、字段超限、SDP/candidate 超限、direction 校验、缺失 stream、registry 增删改查、capacity、room dedupe。

## 已完成（Phase 05 follow-up 第三轮）

- `module/src/p2p/bridge.rs`：将 `P2pJob` 与 `WebRtcDriverHandle` 串起的 runtime-aware glue。
  - `P2pDriverSink` async trait + 自动 `Arc<WebRtcDriverHandle>` 实现 + `Arc<T: P2pDriverSink>` 通用 blanket impl，让测试和生产共享一份接入。
  - `P2pOfferWaiter` async trait（含 `DispatcherOfferWaiter` 适配 `AnswerDispatcher`、测试用 `StaticOfferWaiter / FailingOfferWaiter`）。
  - `run_bridge(P2pBridgeConfig, transport, driver, waiter, cancel)` 异步函数：
    1. 发 `CreateOffer` 给 driver，等待 `OfferReady`；
    2. 把 SDP 交给 `P2pJob::apply(LocalOfferReady)` 并发出 `check_in`；
    3. 主循环把 transport 入站消息映射成 `P2pJobInput::*`，把 `P2pJobAction::*` 翻译成 transport `send` 或 driver 命令；
    4. cancel / remote bye / transport error 都走统一的 shutdown 路径，issue `StopSession` 并 `transport.close()`。
  - `P2pBridgeOutcome` 枚举返回最终状态（`Completed { final_state }` / `OfferFailed` / `TransportError` / `Encode`）。
  - 测试用 `RecordingDriverSink` + `InMemoryTransport::pair` 跑 4 条端到端用例：pull happy path（CreateOffer → check_in → answer → bye → StopSession）、offer 失败、远端 bye、cancel 关停。
- `module/src/p2p/entrypoint.rs`：纯函数把 `compat::ZlmRtcUrl` 翻译成 `P2pBridgePlan`：
  - `plan_from_zlm_url(P2pBridgePlanInput)`：拒绝 `signaling_protocols=0`、强制 `peer_room_id`、用 `SignalingUrlPolicy` 校验 SSRF（`webrtc://` → `ws://`，`webrtcs://` → `wss://`，端口默认 80/443，IPv6 host 自动加方括号）。
  - 输出 `P2pBridgePlan { bridge_config, signaling_url, kind }`，`pending_candidate_cap = 0` 自动落到 `PENDING_CANDIDATE_DEFAULT_CAP`。
  - 5 条单元测试：`signaling_protocols=0` 拒绝、缺 `peer_room_id` 拒绝、loopback 默认拒绝、happy path round-trip URL 字段、`allow_private_ips` 放开 loopback。
- `module/src/p2p/supervisor.rs`：keeper 监督任务：
  - `KeeperTransportFactory` async trait + `KeeperSupervisorConfig` retry 旋钮（`retry_initial_backoff / retry_max_backoff / max_attempts`）。
  - `run_supervisor(registry, key, config, factory, cancel) -> KeeperSupervisorOutcome`：
    - 状态机 `Pending → Connecting → Registered → Reconnecting → Failed/Stopped/KeeperRemoved`，每次 connect 失败按指数退避并写回 `P2pKeeperStatus`，到达 `max_attempts` 标记 `Failed` 并返回 `GaveUp`。
    - 检测 keeper 从 registry 删除后立即返回 `KeeperRemoved`。
  - 3 条 `start_paused = true` tokio 测试：first connect → Registered → 模拟断线 → 重连；总是失败 → 重试 → `Failed`；keeper 中途被移除 → `KeeperRemoved`。
- `WebRtcDriverHandle::drain_within(timeout)`（来自 Phase 02 第三轮）让 P2P bridge 关停时可以等驱动 session 真正落地，便于 keeper supervisor 与 bridge 共享 graceful shutdown 语义。

## 已完成（Phase 05 follow-up 第二轮）

- `crates/protocols/webrtc/module/src/p2p/url.rs`：信令 URL 解析 + SSRF 守卫：
  - `parse(input, &SignalingUrlPolicy) -> Result<SignalingUrl, _>`：识别 `ws://` / `wss://`、解析 IPv4 / IPv6 字面量 / 域名 + 默认端口（443/80）、解析 `path`。
  - 默认拒绝 loopback / private / link-local / multicast / `localhost` 字面量；`allow_private_ips` 或 `host_allowlist` 可显式放开。
  - `SignalingUrlError::TooLong / MissingScheme / InvalidScheme / MissingAuthority / InvalidAuthority / InvalidPort / Blocked`。
  - 长度上限 `SIGNALING_URL_MAX_BYTES = 2048`；render 后再 parse 实现 round trip。
  - 14 条单元测试覆盖正常解析、IPv6 字面量、私网 IP 拒绝、`allow_private_ips`、allowlist 覆盖默认拒绝、scheme 校验、render round-trip。
- `crates/protocols/webrtc/module/src/p2p/buffer.rs`：pending candidate 缓冲：
  - `PendingCandidateBuffer::new(cap)` + `push / flush / drain / close`，状态机 `AwaitingAnswer → Open → Closed`。
  - 严格 dedupe（按 `candidate` 字符串）、bounded（满时驱逐最老条目并返回 `PushOutcome::Evicted`）、close 后 `push` 返回 `Closed`。
  - 默认 `PENDING_CANDIDATE_DEFAULT_CAP = 32`；rejects `cap == 0`。
  - 5 条单元测试。
- `crates/protocols/webrtc/module/src/p2p/transport.rs`：runtime-neutral transport：
  - `P2pTransport` async trait（`send / recv / close`），`P2pTransportEvent::{Message, Closed, Error}`。
  - `InMemoryTransport::pair(capacity)` 返回相互连接的两端；自带 `recorder` 暴露已发送消息。
  - 3 条单元测试（双向 round-trip、close 传播、recorder）。
- `crates/protocols/webrtc/module/src/p2p/job.rs`：纯状态机的 P2P pull/push job：
  - `P2pJob::new(P2pJobConfig)` + `apply(P2pJobInput) -> Vec<P2pJobAction>`。
  - 状态：`Pending → Offering → AwaitingAnswer → Connected → Bye / Failed`。
  - 输入：`LocalOfferReady / RemoteAnswer / RemoteCandidate / DriverConnected / LocalBye / RemoteBye / TransportError`。
  - 输出：`SendCheckIn / ApplyRemoteAnswer / AddRemoteCandidate / SendBye / Diagnostic / Fatal`。
  - candidate 早于 answer 自动进 `PendingCandidateBuffer`，answer 落地时按到达顺序 flush；duplicate 仅发 `Diagnostic`。
  - `TransportError` 把 job 推入 `Failed` 并保留 `last_error()`。
  - 8 条单元测试覆盖 happy path、buffer flush 顺序、open 状态直通、duplicate、transport error、错误状态转换、双 bye 幂等、remote bye 关闭 buffer。
- HTTP API：`WebRtcHttpService` 接入 `P2pRoomKeeperRegistry`，新增 4 条路由：
  - `POST /api/v1/rtc/p2p/keeper/add`：body 含 `server_host / server_port / ssl / room_id / vhost / app / stream`。
  - `POST /api/v1/rtc/p2p/keeper/remove`：body `{ "key": "keeper-N" }`。
  - `GET /api/v1/rtc/p2p/keeper/list`：返回所有 keeper（含 `state / last_error / reconnect_attempts`）。
  - `GET /api/v1/rtc/p2p/rooms`：返回 distinct room_ids，对齐 `mk_webrtc_list_rooms`。
- 集成测试 `module/tests/module_lifecycle.rs::p2p_keeper_api_round_trip`：覆盖 list 空、add、list 一项、rooms 命中、bad json 400、missing room_id 400、unknown key 404、remove 200、再次 list 空 — 端到端串通 HTTP service 与 registry。

## 仍未落地（下一轮）

Phase 05 的所有规划项均已落地实现。剩下：

- 真实 ZLMediaKit 互操作字段差异：本地 `tests/p2p_websocket_supervisor.rs` 跑通的链路是 cheetah 自己的 schema；与 ZLMediaKit 的真实 P2P signaling 字段差异需要在互操作 lab 上验证。属于 Phase 06 范围（参见 `phase-06-external-interop-infra.md`）。

## 实现概览

本阶段实现 ZLMediaKit 风格 WebSocket P2P signaling。它只处理信令，不实现 WebRTC 协议状态；媒体和 ICE/DTLS/SRTP/SCTP 仍由 `str0m` 和现有 driver/core 负责。

## 5.1 模块结构

建议在 `cheetah-webrtc-module` 新增：

```text
src/p2p/
  mod.rs
  message.rs
  room.rs
  client.rs
  job.rs
  config.rs
```

职责：

- `message.rs`：严格定义 wire schema。
- `room.rs`：room keeper registry 和本地 room 状态。
- `client.rs`：WebSocket signaling client。
- `job.rs`：pull/push P2P job 状态机。
- `config.rs`：P2P signaling 配置与资源上界。

## 5.2 Wire schema

首版支持消息：

```text
check_in
check_in_ok
offer
answer
candidate
bye
error
ping
pong
room_list
```

字段约束：

- `room_id`: 1..128 chars。
- `peer_id`: 1..128 chars。
- `transport_id`: 1..128 chars。
- `sdp`: 不超过 `max_sdp_bytes`。
- `candidate`: 不超过 `max_candidate_bytes`。
- `direction`: `pull|push|p2p`。
- `stream`: `{ vhost, app, stream }`。

未知消息类型返回 `error`，不透传。

## 5.3 Room keeper

Room keeper 行为对齐 ZLM C API：

- `add_room_keeper(server_host, server_port, room_id, ssl)`
- `del_room_keeper(room_key)`
- `list_room_keepers()`
- `list_rooms()`

本项目 HTTP API：

- `POST /api/v1/rtc/p2p/keeper/add`
- `POST /api/v1/rtc/p2p/keeper/remove`
- `GET /api/v1/rtc/p2p/keeper/list`
- `GET /api/v1/rtc/p2p/rooms`

状态：

- `Connecting`
- `Registered`
- `Reconnecting`
- `Stopped`
- `Failed`

Keeper 断线后按配置退避重连；stop 后不重连。

## 5.4 P2P client pull

`webrtc://host:port/app/stream?signaling_protocols=1&peer_room_id=room` pull 流程：

1. 解析 URL。
2. SSRF 防护和鉴权。
3. 创建临时 local room id，例如 `ringing_<random>`.
4. 连接 signaling server。
5. 创建 WebRTC offer session。
6. 向 `peer_room_id` 发送 `check_in` 和 `offer`。
7. 收到 answer 后 `ApplyRemoteAnswer`。
8. 收到 candidate 后 `AddRemoteCandidate`。
9. ICE connected 后 remote media 进入 engine。
10. stop 时发送 `bye` 和关闭 session。

## 5.5 P2P client push

Push 流程：

1. 校验本地 stream 存在。
2. 订阅 engine。
3. 创建 WebRTC offer session。
4. check-in 目标 room。
5. 发送 offer。
6. 应用 answer/candidate。
7. 将本地 `AVFrame` 通过 WebRTC 发出。
8. 收到 PLI/FIR 时请求上游关键帧。
9. stop 时释放 subscriber、发送 bye。

## 5.6 Candidate 顺序与缓冲

需要处理：

- candidate 早于 answer 到达。
- answer 后补 candidate。
- ICE restart candidate fragment。
- 重复 candidate。
- malformed candidate。

策略：

- session 未 ready 前 candidate 进入 bounded pending queue。
- 应用 answer 后 flush pending candidates。
- queue 满时丢弃最旧 candidate 并记录 diagnostic。
- malformed candidate 不关闭整个 job，除非超过错误阈值。

## 5.7 安全与上界

配置：

```text
enable_p2p_signaling: bool
p2p_max_room_keepers: usize
p2p_max_peer_sessions: usize
p2p_max_message_bytes: usize
p2p_max_sdp_bytes: usize
p2p_max_candidate_bytes: usize
p2p_pending_candidate_limit: usize
p2p_reconnect_initial_ms: u64
p2p_reconnect_max_ms: u64
```

安全：

- signaling URL 默认拒绝 private/loopback/link-local，除非显式允许。
- WebSocket TLS 校验默认开启。
- room id、peer id、stream key 校验字符集。
- 不接受任意 JSON 扩展字段进入 core。

## 5.8 测试要求

单元测试：

- message parse/render roundtrip。
- unknown message rejected。
- oversized SDP/candidate rejected。
- room keeper add/remove/list。
- active job conflict returns conflict。
- terminal job can be replaced。
- pending candidate before answer flushes after answer。
- malformed candidate diagnostic。

集成测试：

- fake signaling server：pull 成功交换 offer/answer/candidate。
- fake signaling server：push 成功并收到 bye。
- reconnect：server 断开后 keeper 进入 reconnecting。
- SSRF：private IP 默认拒绝。

运行：

```powershell
cargo test -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-driver-tokio
```

## 完成后检查

- P2P signaling 不进入 `cheetah-webrtc-core`。
- client pull/push 的 WHIP/WHEP 和 P2P 路径状态机分清。
- stop/cancel 会释放 room keeper、session、subscriber、publish lease、pending candidates。

