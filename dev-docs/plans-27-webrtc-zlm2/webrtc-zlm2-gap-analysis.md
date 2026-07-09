# WebRTC ZLM2 剩余缺口分析

## ZLM 对照结论

ZLMediaKit 的剩余参考主要集中在三处：

1. `mk_rtc_server_start`：启动 UDP 和 TCP server；UDP 首包通过 `WebRtcSession::queryPoller` 将 session 分发到合适 poller。
2. `WebRtcSignaling*` + `WebRtcClient`：P2P signaling 使用 WebSocket，`signaling_protocols=1` 时执行 room check-in、candidate、answer、bye。
3. `api/include/mk_webrtc.h` + `api/source/mk_webrtc.cpp`：暴露 room keeper add/del/list、rooms list、proxy player info 等控制面能力。

本项目已完成较多基础能力，但缺少 ZLM 在生产部署中依赖的“多 poller 承载”和“真实 P2P 信令实体”。

## Driver shard 缺口

本地已有：

- UDP/TCP listener。
- RFC 4571 TCP framing。
- TCP accepted/closed event。
- route table active/stale/expired。
- migration rejected diagnostic。
- backpressure event。
- 全局 `RouteDirectory`（`session/remote/ufrag/stale` 四类映射，含容量上限与 stale TTL；`forget_shard(shard) -> RouteDirectoryEvictionStats` 提供按 shard 一次性清理入口）。
- `WebRtcDriverConfig::driver_shards / route_directory_capacity / route_directory_stale_capacity` 公共字段；`WebRtcDriverHandle::shard_count() / shard_stats() / route_directory() / shard_selector() / drain_within(timeout) / evict_shard(shard)` API。
- `ShardSelectorStrategy` trait + `HashShardStrategy`（默认）+ `LeastLoadedShardStrategy`（对外可配置）+ `StickyHashShardStrategy { inner, cache_capacity }`（包装任意 inner，按 session-affinity 缓存）+ `BalancedStickyShardStrategy`（sticky-over-least-loaded 具名组合）+ `LoadAwareRebalanceStrategy`（refresh_interval_ticks 控制再均衡频率）+ `ShardLoadTable`（per-shard `{ session_count, active_routes, stale_routes }`）。
- `drain_within(timeout)` 提供 graceful 关停轮询，避免操作员热停止时丢失 RTCP/DTLS shutdown。
- **真正多 shard event loop 拆分**（第五轮）：`spawn_driver` 在 `effective_shard_count() > 1` 时拉起 `WebRtcIoFront` 前端 + N 个 `run_shard_loop` 任务，每个 shard 持有独立 `WebRtcCore` / `RouteTable` / 握手 watchdog。`io_front.rs` 命令按 session id（`AcceptOffer/CreateOffer` 走 selector，其他走 directory）+ packet 按 addr / 解析 STUN USERNAME 的 ufrag 路由到 owner shard。Shard 在 `LocalDescription` 输出时把 `a=ice-ufrag:` 注册到 directory；`CloseSession` 路径同步 `forget_ufrag`。`stun.rs` 提供最小 STUN binding-request USERNAME 解析（不验证 message integrity）。Single-shard fast path 保留以维护既有约束。
- **per-shard route counters + sticky strategy + supervisor**（第六轮）：每个 shard loop 用 `ShardLoadTable::record_route_counts` 公布本地 `RouteTable` 的 active / stale 数；`WebRtcDriverHandle::shard_stats()` 在 multi-shard 模式下报告真实 per-shard 数。`StickyHashShardStrategy` 缓存第一次 pick 结果，避免 ICE restart 在 least-loaded 下漂移。`io_front::spawn_shards` supervisor 任务包裹 shard loop，shard 退出（cancel / graceful exit / panic）时 send `WebRtcDriverEvent::ShardStopped { shard_id, reason }`；`WebRtcModule` 的 driver event worker 已经 match 这个分支把 panic 与 graceful exit 区分日志级。
- 单元测试 31 条（directory 11 + shard 12 + stun 8）+ 集成测试 13 条（`tests/driver_shard.rs` 7 条含 2 条 drain；`tests/driver_multishard.rs` 6 条 multi-shard 集成）。

仍缺：

- TCP writer registry 在多 shard 模式下跨 shard 所有权语义：当前由前端持有，shard 出包通过 `tcp_writers.get(addr)` 拿写半部，但 `TcpClosed` 事件只发到全局 channel；inbound TCP migration 的 owner 转移仍待处理。
- candidate policy 在 shard 后的诊断扩展（`local UDP host candidate count` 等）。
- `LoadAwareRebalanceStrategy` 与 `StickyHashShardStrategy` 的级联组合 recipe（先 sticky → 长期 rebalance），目前需要调用方手写。

主要风险：

- route directory 和 shard route table 双写导致不一致。
- 连接迁移时 old addr/new addr 分属不同 shard。
- TCP writer registry 在前端和 shard 间所有权不清。
- `WebRtcCoreOutput::SetTimer` 分散到多个 shard 后 timer 唤醒过多。

## Candidate policy 缺口

本地已有 ICE policy wire 字段，但 shard 化后需要补：

- UDP host candidate、TCP passive candidate 的显式发布和过滤。
- relay-only/p2p-only 对本地 candidate 和 remote candidate 的一致过滤。
- interface allowlist、extern IP、IPv4/IPv6 策略。
- candidate 诊断：local/remote candidate count、selected pair、rejected reason。

ZLM `IceTransport` 对 candidate info、policy、selected pair 有详细 dump；本项目应输出等价诊断，但不复制其 ICE 状态机。

## P2P signaling 缺口

本地已有：

- ZLM `webrtc://...signaling_protocols=1&peer_room_id=...` URL parser。
- P2P add/remove/list 管理雏形。
- WHIP/WHEP client job。
- `module/src/p2p/{message,room,url,buffer,transport,job,bridge,entrypoint,supervisor,hub,lifecycle_dispatcher,ws,server}.rs` + `module/src/p2p_jobs.rs`：
  - 完整 wire schema、parse/render、字段长度上界。
  - room keeper registry（含 capacity、`P2pKeeperState` 状态、room dedupe）。
  - 信令 URL 解析 + SSRF 守卫（默认拒绝 loopback / private / link-local；`allow_private_ips` / `host_allowlist` 显式放开）。
  - pending candidate buffer（dedupe + bounded + state machine）。
  - runtime-neutral `P2pTransport` trait + `InMemoryTransport::pair` 测试夹具。
  - 纯状态机 `P2pJob`：`Pending → Offering → AwaitingAnswer → Connected → Bye/Failed`，candidate 早于 answer 自动缓冲。
  - `run_bridge(...)` / `run_bridge_with_lifecycle(...)`：driver bridge，包含 `P2pDriverSink` / `P2pOfferWaiter` / `BridgeLifecycleSource` 抽象 + 5 条端到端 unit test。
  - `plan_from_zlm_url(...)`：把 ZLM URL 翻译成 `P2pBridgePlan`，统一 SSRF 校验。
  - `run_supervisor(...)` / `run_supervisor_with_hub(...)`：keeper 监督任务 + `KeeperHubObserver` 自动包 `KeeperHub`。
  - `KeeperHub<T> + HubBackedTransport<T>`：单一 signaling 连接 multiplex 多个 P2P 会话；7 条 hub 单元测试 + 1 条 hub↔bridge 端到端集成测试。
  - `LifecycleDispatcher`：`BridgeLifecycleSource` 实现，自动被 driver event worker 喂入 `Connected/Closed/Failed`。
  - `WebSocketTransportFactory` / `WebSocketP2pTransport`：`tokio-tungstenite 0.29` + `rustls 0.23` + `webpki-roots`；类型擦除 `Box<dyn Sink/Stream>` 同时支持客户端 / 服务端流；自动 ping 响应；UTF-8 + JSON 解码 binary 帧；`WebSocketCounters` metrics。
  - `P2pClientJobRegistry` + `spawn`：自动组装 `WebSocketTransportFactory` + `run_supervisor_with_hub` + `P2pClientObserver`（自动 `hub.attach` + `run_bridge_with_lifecycle` + `DispatcherOfferWaiter` 真 SDP 接入），HTTP `/pull/start` / `/push/start` 真正返回 200 + session id；`/p2p/client/{list,stop}` 提供运营查询 / 取消。
  - `run_signaling_server`：可选 inbound WebSocket signaling server（`tokio-tungstenite::accept_async` + `accept_timeout` + `max_connections` 上界 + RAII 容量计数）。
- HTTP API `/api/v1/rtc/p2p/keeper/{add,remove,list}` + `/p2p/rooms` + `/p2p/client/{list,stop}` 接入 `WebRtcHttpService`。
- 集成测试：`tests/p2p_pipeline.rs`（in-memory 全链路）、`tests/p2p_websocket_transport.rs`（真 WebSocket transport round-trip）、`tests/p2p_websocket_supervisor.rs`（supervisor + 真 WebSocket 端到端）、`tests/module_lifecycle.rs::pull_start_p2p_*`（HTTP 200 / 400 / list / stop）、`p2p::server::tests`（accept + 容量上限）。

仍缺：

Phase 05 的所有规划项均已落地实现。剩下的真实 ZLMediaKit 互操作字段差异验证由 Phase 06 互操作 lab 负责（参见 `phase-06-external-interop-infra.md`）。

主要风险：

- P2P signaling 是非标准协议，必须清楚标注 ZLM compatibility，不与 WHIP/WHEP 混淆。
- 自定义 JSON schema 如果过宽，会引入安全风险。
- 远端 answer/candidate 顺序不稳定，必须支持 answer 前后 candidate 缓冲。
- room keeper 生命周期和 WebRTC session 生命周期不同，不能互相泄漏资源。

## 外部互操作缺口

本地已有：

- `crates/protocols/webrtc/module/tests/interop.rs` ignored scaffold（已重写，统一使用 `interop_harness`）。
- `crates/protocols/webrtc/module/tests/interop_harness.rs`：统一 env 常量、`InteropArtifact`（`open / write / append / set_failure`）、artifact root 自动定位、`timeout()` 上下界、`require_env()` 含 skip 日志，含 5 条单元测试。
- ignored 测试覆盖 ZLM WHIP / ZLM P2P signaling / Pion / GStreamer / Janus / browser / 跨协议（RTSP/RTMP/GB28181）/ 弱网。
- `dev-docs/plans-27-webrtc-zlm2/interop-runner.md`：runner 操作手册，把 env、复现命令、CI 推荐流程一次性写齐。
- `.github/workflows/webrtc-interop-nightly.yml`：定时 / 手动触发的 nightly workflow，自带 ZLMediaKit service container、ZLM 健康探测、`cargo test --ignored` 调用、`actions/upload-artifact` 持久化 `target/webrtc-interop/` 目录。
- `dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml`：本地一键起 lab，固定 ZLM tag、可选 Pion / Playwright profile，与 nightly workflow 共用同一组容器。
- `module/tests/interop_harness.rs::assertions`：媒体面 assertion helpers — `InteropThresholds` 默认阈值 + `assert_offer_well_formed / assert_answer_well_formed / assert_first_keyframe_within / assert_nack_engaged / assert_bwe_above / count_candidates / assert_candidate_types_present`，含 12 条单元测试。
- **第六轮新增**：`dev-docs/plans-27-webrtc-zlm2/interop-pion-helper/` (Dockerfile + main.go + go.mod + README) 提供 Pion WHIP/WHEP 双模 helper 骨架；`dev-docs/plans-27-webrtc-zlm2/interop-playwright/` (whip-whep.spec.ts + playwright.config.ts) 提供 Chrome `getStats()` 抓取骨架；`dev-docs/plans-27-webrtc-zlm2/interop-weak-network/` (run-netem.sh + README) 提供 Linux `tc netem` 包装；docker-compose `pion-helper` 改用本地 build context、`playwright` 挂载本地 specs；`tests/interop.rs::zlm_answer_sdp_validation` 演示 assertion helpers 在捕获 SDP 后的诊断写入。
- **第七轮新增**：`crates/protocols/webrtc/module/tests/fixtures/zlm/{whip,whep}_answer.sdp` ZLM 风格答复 SDP fixture + `tests/zlm_sdp_fixtures.rs` 7 条非 ignored 验证测试（well-formed、ZLM-specific 字段、WHEP sendonly + msid、WHEP video FID ssrc-group、WHIP recvonly、阈值 sanity），让 assertion helpers 与真实 ZLM 字段差异每次 `cargo test` 都被覆盖；`dev-docs/plans-27-webrtc-zlm2/interop-{gstreamer,janus}-helper/README.md` 给出 GStreamer `webrtcbin` `gst-launch-1.0` 调用与 Janus REST 三段式握手的文档骨架；nightly CI workflow 增加 “Run ZLM answer SDP fixture sanity tests” 步骤。
- **第八轮新增**：候选解析 helper（`CandidateCounts` + `count_candidates` + `assert_candidate_types_present`，4 条新单元测试）；`tests/fixtures/zlm/{tcp_candidate_offer,ipv6_candidate_offer,turn_relay_offer}.sdp` 三份候选类型 fixture + 4 条 `zlm_sdp_fixtures` 测试覆盖 TCP active/passive、IPv6 link-local + global、TURN raddr/rport；`interop-gstreamer-helper/{Dockerfile,entrypoint.sh}` 真实可构建镜像（`gst-launch-1.0` whip / whep 双模 + `peer.log` artifact）；`interop-janus-helper/{Dockerfile,smoke.sh}` 派生自 `canyan/janus-gateway` 的 REST 三段式 echotest smoke 镜像；`interop-docker-compose.yml` 新增 `gstreamer-helper` + `janus-helper` 两个 profile。
- **第九轮新增**：`interop-cheetah-server/{Dockerfile,interop.yaml,README.md}` cheetah-server 真实可构建镜像（multi-stage build with cargo cache、非 root 1000:cheetah、`webrtc + rtmp` 默认 features、互操作 lab 用 config）；docker-compose 增加 `cheetah` profile 让 cheetah 自己以 service 形式进入 lab，实现 ZLM ↔ cheetah ↔ Pion / GStreamer / Janus 闭环拓扑；`tests/cheetah_self_interop.rs` 2 条非 ignored 自闭环测试（cheetah WHIP 答复满足 assertion helpers、答复携带 BUNDLE / fingerprint / mid / ICE 凭据）；nightly CI workflow 增加 “Run cheetah self-loopback interop sanity tests” 步骤。
- **第十轮新增**：`tests/fixtures/zlm/datachannel_answer.sdp` ZLM 风格 DataChannel/SCTP 答复 fixture（三段式 BUNDLE = audio + video + application，`m=application UDP/DTLS/SCTP webrtc-datachannel` + `a=sctp-port:5000` + `a=max-message-size:262144`）+ 4 条 fixture 测试（well-formed、SCTP 段、三段 mid、max-message-size 上界）；`.github/workflows/webrtc-interop-nightly.yml` 新增 `weak-network` job（6-profile 矩阵 `loss-1/5/10/20 + reorder + bw-cap`、`cargo test --no-run` 缩短 qdisc 窗口、`sudo run-netem.sh` 包裹 cargo test、artifact 14 天保留、仅 manual dispatch 触发）。
- **第十一轮新增**：`tests/cheetah_to_zlm_interop.rs::cheetah_offer_to_zlm_whip` 真实端到端 ignored 测试（cheetah in-process WHIP 答复检查 → tokio 原生 HTTP/1.1 客户端 POST 到 ZLM → `assert_answer_well_formed` 验 ZLM 答复 → 三份 SDP artifact 落盘）；nightly CI workflow 增加 “Run cheetah↔ZLM ignored end-to-end test” 步骤；`tests/fixtures/zlm/{h264_only_offer.sdp, gb28181_play_answer.sdp}` 两份新 fixture + 5 条 fixture 测试（H.264-only single codec + RTX、`packetization-mode=1`、GB28181 video-only 答复、`ZLMediaKit-GB28181` session name 标记）；`dev-docs/plans-27-webrtc-zlm2/interop-weak-network/WINDOWS.md` 给出 Windows / macOS 弱网等价方案文档（Clumsy / pktmon / Network Link Conditioner，含 profile 映射表）。
- **第十二轮新增**：`tests/cheetah_to_pion_interop.rs::pion_publish_to_cheetah_whip` 真实端到端 ignored 测试（用 `tokio::task::spawn_blocking` 包 `std::process::Command` 跑 Pion helper binary，不引入 `tokio/process` feature；解析 helper 写出的 `peer-stats.json`）；nightly CI workflow 增加 “Run cheetah↔Pion ignored end-to-end test” 步骤；`tests/fixtures/zlm/simulcast_offer.sdp` 三层 simulcast offer fixture + `assertions::SimulcastRids / extract_simulcast_rids / assert_simulcast_layers` helper + 4 条 fixture 测试 + 6 条 harness 单元测试；nightly CI workflow 新增 `weak-network-default` job 在默认调度跑 `loss-5` profile。
- **第十三轮新增**：`tests/cheetah_to_janus_interop.rs::cheetah_drives_janus_echotest` 真实端到端 ignored 测试（tokio 原生 HTTP/1.1 客户端 → Janus REST 三段式 `create / attach / message`、ack/event/success 三选一）；nightly CI workflow 增加 “Run cheetah↔Janus ignored end-to-end test” 步骤；`tests/fixtures/zlm/low_latency_offer.sdp` 低延迟 offer fixture（`playout-delay` + `video-timing` + `transport-cc` + `goog-remb`）+ 3 条 fixture 测试；`RouteDirectory::forget_shard` + `WebRtcDriverHandle::evict_shard` 操作员手动 shard 恢复入口（Phase 02 第八轮，跨 phase 同轮落地）。
- **第十四轮新增**：`WebRtcDriverConfig::shard_restart_on_panic / shard_max_restart_count / shard_restart_backoff_ms / shard_max_restart_backoff_ms` 公共字段；supervisor 在 panic 时自动 evict（forget_shard + 重置 load + 同步 session_count + 发 Lifecycle diagnostic）；`multishard_restart_on_panic_config_does_not_break_graceful_cancel` 集成测试。`tests/fixtures/zlm/tcp_fallback_answer.sdp` ZLM TCP fallback answer fixture + 3 条 fixture 测试（well-formed / TCP proto + tcptype passive / 保留 RTX + FID）；nightly CI `weak-network-default` job 升级为 `loss-5 + reorder` 矩阵。
- **第十五轮新增**：`ShardChannels` 重写为 `parking_lot::RwLock<mpsc::Sender>` + `Vec<Arc<ShardChannels>>` 共享所有权；supervisor 在 panic + `shard_restart_on_panic = true` 时按 backoff 与 `shard_max_restart_count` 预算重 spawn `run_shard_loop`，前端 `Arc<ShardChannels>::rebind` 切换到新 receiver。`LoadAwareRebalanceStrategy` 新策略包装任意 inner + FIFO 缓存 + `refresh_interval_ticks` 控制刷新频率（4 条单元测试）。`tests/fixtures/zlm/screen_share_offer.sdp` 屏幕共享 offer + `assertions::MsidEntry / extract_msids / assert_msid_stream_present` helper（4 条新单元测试）+ 4 条 fixture 测试（共流 msid、`a=content:slides` + extmap、RTX/FID 保留）。

仍缺：

- dual-stream simulcast 媒体面验证（用 Playwright + Chrome `getStats()` 检查 cheetah 三层下发的 `outbound-rtp` 多 SSRC）。
- TCP writer registry 在多 shard 模式下跨 shard 所有权语义：当前由前端持有，shard 出包通过 `tcp_writers.get(addr)` 拿写半部，但 `TcpClosed` 事件只发到全局 channel；inbound TCP migration 的 owner 转移仍待处理。
- 跨平台 weak-network 自动化：`tc netem` Linux 矩阵已就绪，Windows / macOS 路径仍只有文档。
- 更多 ZLM 字段差异 fixture（DTMF / INFO、SVC scalability mode、低带宽 codec switch 等）。

主要风险：

- 外部环境不稳定导致 CI flaky。
- 测试只检查 HTTP 200，不验证媒体面。
- 手工命令和 ignored test 断言不一致。
- 失败时没有 SDP、日志、stats，无法定位。

## 必须保留的边界

- `cheetah-webrtc-core` 不引入 Tokio、WebSocket、HTTP、socket、engine。
- `str0m` 仍负责 WebRTC 协议状态机。
- P2P signaling 只负责信令，不解析或实现 ICE/DTLS/SRTP。
- 外部互操作测试不能成为默认 `cargo test` 的硬依赖。
- module 不能绕过 publish lease 或 subscriber API。

