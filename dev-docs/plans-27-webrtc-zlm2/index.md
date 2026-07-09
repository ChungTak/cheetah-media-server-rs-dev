# WebRTC 剩余架构实现计划（对标 ZLMediaKit，第二阶段）

- **状态**: 第十三轮已落地（Phase 02 `RouteUpdated` 携带 `RouteCandidateDiff`；`ShardCandidateTable` + `shard_candidate_stats()` 按 shard 维度持久化 candidate 计数；module 层 8 个 Prometheus counter 接入运营 dashboard）。下一轮聚焦 TCP writer 跨 shard 物理迁移评估 + `RouteCandidateDiff` 业务层消费（candidate churn 告警）。
- **目标**: 在 `dev-docs/plans-27-webrtc-zlm/` 已完成和部分完成的基础上，继续规划剩余的大架构工作：Phase 02 多线程 shard、Phase 05 P2P signaling、外部互操作实体测试。
- **方法**: 继续使用 `str0m` 承担 WebRTC Sans-I/O 协议状态机；参考 `vendor-ref/ZLMediaKit/webrtc/` 的 transport、client、signaling、room keeper 设计，并参考 `vendor-ref/ZLMediaKit/src/`、`api/` 的 server 启动、HTTP、RTCP 和 C API 行为。
- **完成标准**: WebRTC driver 能按 shard 横向扩展，P2P signaling 可与 ZLM 风格房间/peer 模型互通，外部实体互操作测试能在本地或 CI 环境稳定复现。

---

## 本轮剩余范围

本目录只补 `plans-27-webrtc-zlm` 中仍需要更大架构或外部基础设施支持的工作：

1. **Phase 02 多线程 shard**
   - 前端 UDP/TCP acceptor 与多个 session owner shard 分离。
   - 全局 route directory 支持 remote addr、STUN ufrag、session id 到 shard 的查找。
   - UDP/TCP 共用路由、连接迁移、candidate policy 和 backpressure 在 shard 模式下保持一致。
2. **Phase 05 P2P signaling**
   - 实现 ZLMediaKit 风格 WebSocket P2P 信令：room keeper、check-in、candidate、answer、bye、list。
   - client pull/push 的 `signaling_protocols=1` 路径走 P2P signaling，而不是 WHIP/WHEP。
   - P2P session 仍走 `cheetah-webrtc-core` + driver + `str0m`，不绕过资源上界。
3. **外部互操作实体测试**
   - 把现有 ignored scaffold 升级为可运行实体测试。
   - 覆盖 ZLMediaKit、ZLMRTCClient/浏览器、Pion、GStreamer、Janus、跨协议源。
   - 固化环境变量、docker/local runner、日志、SDP、packet stats 和失败 artifact。

本轮不重新展开：

1. Phase 01 SDP/codec/RTP extension 已落地内容。
2. Phase 03 WHIP/WHEP、ZLM URL、publish/play、echo、GOP 已落地内容。
3. Phase 04 simulcast/BWE/NACK/TWCC 已落地内容；只在互操作实体测试里验证。

---

## ZLMediaKit 关键参考

| 领域 | ZLM 文件 | 参考行为 |
|------|----------|----------|
| RTC server 启动 | `api/source/mk_common.cpp` | `mk_rtc_server_start` 同时启动 UDP/TCP，UDP 首包通过 `WebRtcSession::queryPoller` 选择 poller |
| TCP/UDP session | `webrtc/WebRtcSession.*`、`webrtc/IceSession.*` | 单端口收包、TCP HTTP splitter/RFC4571 风格 framing、ICE transport 交给 transport |
| P2P signaling server | `api/source/mk_common.cpp`、`webrtc/WebRtcSignalingSession.*` | `mk_signaling_server_start` 启动 WebSocket signaling server |
| P2P signaling peer | `webrtc/WebRtcSignalingPeer.*` | room keeper、check-in、candidate、checkout、远端 answer/candidate 回调 |
| WebRTC client | `webrtc/WebRtcClient.*` | `signaling_protocols=0` 走 WHIP/WHEP，`1` 走 WebSocket P2P |
| C API | `api/include/mk_webrtc.h`、`api/source/mk_webrtc.cpp` | add/del/list room keeper、list rooms、proxy player info |
| ICE candidate | `webrtc/IceTransport.*`、`webrtc/Sdp.*` | candidate policy、relay/p2p-only、candidate info dump、local/remote candidate 观测 |
| DataChannel / SCTP | `webrtc/SctpAssociation.*`、`src/Common/config.*` | message size、SCTP state broadcast、send/recv hooks |
| RTCP 互操作 | `src/Rtcp/*` | REMB/NACK/TWCC/SR/RR 解析构造，可作为实体测试 oracle |

---

## 当前本地状态摘录

- `cheetah-webrtc-driver-tokio` 已有 UDP 单端口、RFC 4571 TCP framing、TCP listener、idle timeout、keepalive、route stale/expired、migration reject、backpressure 事件。
- 多线程 shard 第七轮已落地（`BalancedStickyShardStrategy` 真实类型）：把 “sticky over least-loaded” 配方从 doc recipe 升级成具名类型，避免每个调用方手写 `Arc<StickyHashShardStrategy::new(Arc<LeastLoadedShardStrategy>, cap)>`。提供 `new(cache_capacity)` / `with_default_capacity()`（16k）/ `forget(session_id)` 三个入口，pick 透传给内部 sticky+least-loaded 组合。Re-export 加入 `lib.rs`。新增 2 条单元测试覆盖 “初次落到空闲 shard、之后 pin” 与 “forget 后重新 pick” 行为。
- 多线程 shard 第六轮已落地（per-shard route counters + sticky strategy + shard supervisor）：`ShardLoad` 升级为 `{ session_count, active_routes, stale_routes }` 三元组，`ShardLoadTable::record_route_counts(shard, active, stale)` 让 shard 在每轮事件循环结束发布本地 RouteTable 计数；`WebRtcShardStats` 现在按 shard 真实分布展示 active/stale routes（multi-shard 模式）。新增 `StickyHashShardStrategy { inner, cache_capacity }`：第一次 pick 时委托 inner（默认 hash），之后稳定返回缓存值，避免 ICE restart 在 least-loaded 策略下漂移；FIFO 缓存 + `forget(session_id)` 释放。新增 `WebRtcDriverEvent::ShardStopped { shard_id, reason }` 事件 + supervisor 任务包裹 `run_shard_loop`，`reason` 为 `cancelled / exited / panic: ... / join error: ...`，让操作员区分 graceful drain 与 panic。
- 多线程 shard 第五轮已落地（真正多 shard event loop）：`spawn_driver` 在 `effective_shard_count() > 1` 时拉起 `WebRtcIoFront` 前端 + N 个独立的 `run_shard_loop` 任务，每个 shard 持有独立的 `WebRtcCore` / `RouteTable` / 握手 watchdog；`io_front.rs` 把命令按 session id（`AcceptOffer/CreateOffer` 走 `ShardSelector`，其他命令走 `RouteDirectory::lookup_session`）和把 packet 按 `RouteDirectory::lookup_remote` / 解析 STUN USERNAME 后的 `lookup_ufrag` 分发到 owner shard。Shard 在每次 `LocalDescription` 输出时把 `a=ice-ufrag:` 注册到 directory，使初始 STUN binding 请求能直接路由到正确 shard。`stun.rs` 提供最小 STUN binding-request USERNAME 解析（不验证 message integrity，那是 str0m 的事）。Single-shard fast path（`run_driver_core`）保留以维护既有测试约束。
- 此前轮次已落地：`RouteDirectory`、`ShardSelectorStrategy` trait（`HashShardStrategy` / `LeastLoadedShardStrategy`）、`ShardLoadTable`、`WebRtcDriverHandle::shard_count() / shard_stats() / route_directory() / shard_selector() / drain_within(timeout)`。`run_driver_core` 在 accept 路径用 selector 选 owner，migrate / close 路径读 `RouteDirectory::lookup_session(...)` 保持一致；`shard_stats()` 返回真实的 per-shard 计数。Event loop 仍是单 shard，但 selector 抽象、负载视图、graceful drain 已就位。
- `cheetah-webrtc-module` 已有 ZLM URL parser、WHIP/WHEP client job、P2P add/remove/list 雏形、DataChannel 上界、interop ignored scaffold、Phase 06 互操作 harness、runner 操作手册与 nightly CI workflow。
- P2P signaling 第十轮已落地（全部规划项完成）：`module/src/p2p/{message,room,url,buffer,transport,job,bridge,entrypoint,supervisor,hub,lifecycle_dispatcher,ws,server}.rs` + `module/src/p2p_jobs.rs`：完整 wire schema、room registry、SSRF URL 守卫、pending candidate 缓冲、transport trait + in-memory pair、纯状态机 P2P job、`run_bridge` / `run_bridge_with_lifecycle`、`plan_from_zlm_url`、`run_supervisor` / `run_supervisor_with_hub`、`KeeperHub` 多路复用、`LifecycleDispatcher` + driver event worker 自动分发、`tokio-tungstenite` WebSocket transport（`new` / `from_server_stream` 双入口、类型擦除 `Box<dyn>` sink/stream）、`P2pClientJobRegistry` + `spawn` 后台 supervisor 任务（`AnswerDispatcher` 真 SDP 接入）、可选 inbound signaling server (`run_signaling_server`)；HTTP API `/api/v1/rtc/p2p/keeper/{add,remove,list}` + `/p2p/rooms` + `/p2p/client/{list,stop}` 全可用，`/pull/start` / `/push/start` 收到 `signaling_protocols=1` URL 在 driver 就绪时返回 200 + session id（spawn P2P client job），未就绪时回退 501 + 结构化 extras，URL 校验失败 400 + `p2p_invalid_url`。集成测试覆盖：`tests/p2p_pipeline.rs`（in-memory 全链路）、`tests/p2p_websocket_transport.rs`（真 WebSocket transport round-trip）、`tests/p2p_websocket_supervisor.rs`（supervisor + 真 WebSocket 端到端）、`tests/module_lifecycle.rs::pull_start_p2p_*`（HTTP 200 / 400 / list / stop / bad json / unknown id）、`p2p::server::tests`（accept ↔ handler ↔ 容量上限）。剩真实 ZLM 互操作字段差异由 Phase 06 互操作 lab 验证。
- 外部互操作 harness `module/tests/interop_harness.rs` 已落地，含 `assertions` 模块（`InteropThresholds`、`assert_offer_well_formed`、`assert_answer_well_formed`、`assert_first_keyframe_within`、`assert_nack_engaged`、`assert_bwe_above`、`count_candidates`、`assert_candidate_types_present`，12 条单元测试）；`dev-docs/plans-27-webrtc-zlm2/interop-runner.md` 给出完整复现命令；`.github/workflows/webrtc-interop-nightly.yml` 提供 nightly CI 触发与 artifact 上传；`dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml` 提供本地一键起 ZLM + 可选 cheetah-server / Pion / Playwright / GStreamer / Janus 的 lab。Phase 06 第六轮新增 Pion helper Dockerfile + main.go + go.mod、Playwright `whip-whep.spec.ts` + `playwright.config.ts`、`tc netem` Linux 包装脚本 `run-netem.sh` 与 README，并在 docker-compose 里换成本地 build context 与 specs 挂载。Phase 06 第七轮新增 `tests/fixtures/zlm/{whip,whep}_answer.sdp` ZLM 风格答复 SDP fixture + 非 ignored 验证测试 `tests/zlm_sdp_fixtures.rs`（7 条 fixture 测试 + 默认阈值 sanity）。Phase 06 第八轮新增 `tests/fixtures/zlm/{tcp_candidate_offer,ipv6_candidate_offer,turn_relay_offer}.sdp` 三份候选类型 fixture + 候选数器 helper（`count_candidates`、`assert_candidate_types_present`）+ 4 条新 fixture 测试 + 4 条 helper 单元测试；`interop-gstreamer-helper/{Dockerfile,entrypoint.sh}` + `interop-janus-helper/{Dockerfile,smoke.sh}` 真实可构建镜像；docker-compose 增加 `gstreamer / janus` 两个 profile。Phase 06 第九轮新增 `interop-cheetah-server/{Dockerfile,interop.yaml,README.md}` cheetah-server 真实可构建镜像（multi-stage build、cargo cache、非 root 1000:cheetah、`webrtc + rtmp` 默认 features），docker-compose 增加 `cheetah` profile，让 cheetah 自己也以 service 形式进入 lab；新增 `tests/cheetah_self_interop.rs` 2 条非 ignored 自闭环测试（cheetah WHIP 答复满足 `assert_answer_well_formed`、答复携带 BUNDLE / fingerprint / mid / ICE 凭据），nightly CI workflow 增加 “Run cheetah self-loopback interop sanity tests” 步骤。
- 多 shard 第六轮新增集成测试：`multishard_cancel_emits_shard_stopped_for_each_shard`（cancel 后每 shard 收到一条 `ShardStopped { reason: cancelled / exited }`）、`multishard_route_counts_track_per_shard`（per-shard route counters 起始为 0、session_count 准确）；shard.rs 新增 5 条单元测试（sticky 缓存、forget、capacity eviction、single-shard 透传、route counts）。`StickyHashShardStrategy` 文档新增 “Recipe: balanced-sticky” 段，把 `LeastLoadedShardStrategy` + sticky 组合成一行配方。
- Phase 06 第十一轮新增：`tests/cheetah_to_zlm_interop.rs::cheetah_offer_to_zlm_whip` 真实端到端 ignored 测试（cheetah in-process WHIP 答复检查 → tokio 原生 HTTP/1.1 客户端 POST 到 ZLM → `assert_answer_well_formed` 验 ZLM 答复 → 三份 SDP artifact 落盘）；nightly CI workflow 增加 “Run cheetah↔ZLM ignored end-to-end test” 步骤；`tests/fixtures/zlm/{h264_only_offer.sdp, gb28181_play_answer.sdp}` 两份新 fixture + 5 条 fixture 测试覆盖 H.264-only 单 codec offer + RTX、`packetization-mode=1`、GB28181 video-only 答复、`ZLMediaKit-GB28181` session name 标记；`dev-docs/plans-27-webrtc-zlm2/interop-weak-network/WINDOWS.md` 给出 Windows / macOS 弱网等价方案文档（Clumsy / pktmon / Network Link Conditioner，含 profile 映射表）。
- Phase 06 第十二轮新增：`tests/cheetah_to_pion_interop.rs::pion_publish_to_cheetah_whip` 真实端到端 ignored 测试（用 `tokio::task::spawn_blocking` 包 `std::process::Command` 跑 Pion helper binary，超时杀进程，把 `peer-stats.json` 反序列化校验关键字段）；nightly CI workflow 增加 “Run cheetah↔Pion ignored end-to-end test” 步骤；`tests/fixtures/zlm/simulcast_offer.sdp` 三层 simulcast offer fixture（hi/mid/lo + RID + repaired-rtp-stream-id 扩展）+ 4 条 fixture 测试 + 6 条 harness simulcast 助手测试（`extract_simulcast_rids`、`assert_simulcast_layers`，含两层 / 三层 / 缺失情况）；nightly CI workflow 新增 `weak-network-default` job 在默认调度跑 `loss-5` profile，让弱网回归走每日 nightly 而不只是 manual dispatch。
- Phase 06 第十三轮新增：`tests/cheetah_to_janus_interop.rs::cheetah_drives_janus_echotest` 真实端到端 ignored 测试（tokio 原生 HTTP/1.1 客户端 → Janus REST 三段式 `create / attach / message`、按 `data.id` 拆解 session/handle、ack/event/success kind 三选一）；nightly CI workflow 增加 “Run cheetah↔Janus ignored end-to-end test” 步骤；`tests/fixtures/zlm/low_latency_offer.sdp` 低延迟 offer fixture（含 `playout-delay` / `video-timing` / `transport-cc` / `goog-remb`）+ 3 条 fixture 测试。
- Phase 02 第八轮新增：`RouteDirectory::forget_shard(shard) -> RouteDirectoryEvictionStats` 把指定 shard 拥有的所有 session / address / ufrag / stale 一次性清掉，并把删除数量按四个维度返回；`WebRtcDriverHandle::evict_shard(shard)` 在 handle 层包装 directory + load 表 + session_count 三处更新，让操作员观察到非 graceful `ShardStopped` 后能一行清理孤立映射；`tests/driver_multishard.rs::multishard_evict_shard_drops_directory_and_load_counters` 端到端验证（4 shard、evict 1 个、目标 shard count 归零、其他 shard 不受影响、aggregate session_count 减小）；2 条 directory 单元测试覆盖跨 shard 清理与未知 shard 返 0。
- Phase 02 第九轮新增：`WebRtcDriverConfig::shard_restart_on_panic / shard_max_restart_count / shard_restart_backoff_ms / shard_max_restart_backoff_ms` 四个公共字段（默认 false + 3 + 250ms + 30s），supervisor 在 `shard_restart_on_panic = true` 时对 panicked shard 自动调用 `forget_shard` + 重置 load 表 + 同步 aggregate session_count + 发 `Lifecycle` diagnostic（含四维度清理量），graceful exit / cancel 路径不受影响；`tests/driver_multishard.rs::multishard_restart_on_panic_config_does_not_break_graceful_cancel` 验证 cancel 路径仍发 ShardStopped 但不发 auto-evict diagnostic。
- Phase 06 第十四轮新增：`tests/fixtures/zlm/tcp_fallback_answer.sdp` ZLM TCP fallback answer fixture（`m=video TCP/TLS/RTP/SAVPF`、tcptype passive 候选、保留 RTX + FID ssrc-group）+ 3 条 fixture 测试（well-formed、TCP proto + passive 候选 + 无 UDP 候选、RTX 与 FID 保留）；nightly CI workflow `weak-network-default` job 从单 profile 升级为 `loss-5 + reorder` 矩阵（仍只在默认调度跑），让 nightly 同时覆盖丢包与重排两种弱网场景。
- Phase 02 第十轮 / Phase 06 第十五轮新增：
  - **真正的 shard task 自动 respawn**：`ShardChannels` 重写为 `parking_lot::RwLock<mpsc::Sender>` + `cmd_sender()` / `packet_sender()` / `rebind(...)` 三件套，`spawn_shards` 输出 `Vec<Arc<ShardChannels>>`；supervisor 在 panic + `shard_restart_on_panic = true` 时按 `shard_max_restart_count` 预算重 spawn `run_shard_loop`，前端 `Arc<ShardChannels>` 被 rebind 到新 receiver，运营 dashboard 看到 `shard {N} respawning attempt {i}/{max} (backoff next {ms}ms)` 诊断；超过 `shard_max_restart_count` 后发 `reached max_restart_count` 诊断并停止 supervisor。Backoff 从 `shard_restart_backoff_ms` 起始翻倍封顶 `shard_max_restart_backoff_ms`。
  - **`LoadAwareRebalanceStrategy`**：包装任意 inner（推荐 `LeastLoadedShardStrategy`）+ FIFO 缓存 + `refresh_interval_ticks` 控制刷新频率，介于 sticky 与无状态之间。`with_least_loaded_default()` 提供 8k 缓存 + 256 ticks 默认；`forget(session_id)` 释放绑定。4 条单元测试覆盖初次落到空闲 shard + 缓存命中、超过 refresh 间隔后再均衡、单 shard 透传、forget 释放。`lib.rs` re-export。
  - **screen-share fixture + msid helper**：`tests/fixtures/zlm/screen_share_offer.sdp` 屏幕共享 offer（audio + video 两段共用 `screen-share` 流 id、`a=content:slides`、`video-content-type` extmap、保留 RTX + FID）；`assertions::MsidEntry` + `extract_msids(sdp)` + `assert_msid_stream_present(sdp, stream_id)` helper（4 条新单元测试覆盖 happy path、malformed line skip、stream 匹配、空 SDP 报告）；4 条 fixture 测试（well-formed、audio + video msid 共流、`a=content:slides` + extmap、RTX/FID 保留）。
- Phase 02 第十一轮 / Phase 06 第十六轮新增：
  - **`StickyOverRebalanceStrategy`**：把 sticky 外层缓存 + load-aware rebalance 内层组合落成具名类型，`with_default_capacity()` 提供 16k sticky + 8k rebalance + 256-tick refresh 默认；`forget(session_id)` 同步清理外层 sticky 与内层 rebalance 缓存（避免 inner cache 残留导致 forget 后仍命中旧绑定）。3 条单元测试：sticky 永久绑定（saturate 后多次 pick 仍同 shard）、forget 双层清理、单 shard 透传。`lib.rs` re-export。
  - **`count_local_candidates(sdp) -> LocalCandidateCounts`**：driver-tokio 公共助手，按 `host / srflx / prflx / relay` × `udp / tcp` × `ipv4 / ipv6` 三维度统计 SDP 中的 `a=candidate:` 行；`std::net::IpAddr::parse` 区分 v4/v6（mDNS 主机名按 v4 计数），permissive 解析（malformed 行跳过）。4 条单元测试覆盖 canonical 行、prflx + mDNS、malformed skip、空 SDP 返 0。让运营在不引入 harness 测试依赖的情况下从 `LocalDescription` 输出 SDP 上拿到 candidate 分布。
  - **SVC + DTMF fixtures**：`tests/fixtures/zlm/svc_offer.sdp`（VP9 + `a=scalability-mode:L3T3` + RTX + FID）+ 3 条 fixture 测试（well-formed、scalability-mode + VP9、RTX/FID 保留）；`tests/fixtures/zlm/dtmf_audio_offer.sdp`（opus + `telephone-event/48000` + `telephone-event/8000` 双速率 + `0-16` fmtp）+ 3 条 fixture 测试（well-formed、双速率 telephone-event、opus 仍是首选 PT）。

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [webrtc-zlm2-remaining-architecture.md](webrtc-zlm2-remaining-architecture.md) | 草案 | 剩余架构：driver shard、P2P signaling、外部互操作基础设施 |
| [webrtc-zlm2-gap-analysis.md](webrtc-zlm2-gap-analysis.md) | 第四轮已更新 | ZLM 对照、本地剩余缺口、风险 |
| [phase-02-driver-multithread-shard.md](phase-02-driver-multithread-shard.md) | 第十三轮已落地（RouteCandidateDiff + ShardCandidateTable + shard_candidate_stats + module Prometheus 指标） | shard plumbing + selector strategy + load table + drain + 真正多 shard event loop（front-end + N shard 任务） + sticky / balanced-sticky / load-aware rebalance / sticky-over-rebalance 四种策略 + shard 退出监管 + 操作员手动 shard 恢复 + 自动 panic 清理 + 自动 respawn 循环 + local candidate 诊断助手 + TCP writer registry shard 归属与批量清理 + LocalCandidateSnapshot 按 owner shard 暴露 candidate 分布 + RouteCandidateDiff + shard_candidate_stats + local candidate Prometheus 指标 |
| [phase-05-p2p-signaling.md](phase-05-p2p-signaling.md) | 第十轮已落地（全部规划项完成） | 全 P2P 栈：bridge + URL → plan + supervisor + signaling hub + lifecycle 分发 + WebSocket transport + P2P client job runner + `AnswerDispatcher` 真 SDP 接入 + 可选 inbound signaling server。剩真实 ZLM 互操作字段差异（Phase 06 范围） |
| [phase-06-external-interop-infra.md](phase-06-external-interop-infra.md) | 第十六轮已落地 | harness + runner 操作手册 + nightly CI workflow + docker-compose 一键起 lab + 媒体面 assertion helpers + Pion helper / Playwright spec / netem 脚本骨架 + ZLM 答复 SDP fixture + TCP/IPv6/TURN candidate fixture + DataChannel/SCTP fixture + GB28181 / H.264-only / simulcast / 低延迟 / TCP fallback / screen-share / SVC / DTMF fixture + 候选数器 helper + simulcast 解析 helper + msid helper + GStreamer / Janus 可构建镜像与 docker-compose profile + cheetah-server 可构建镜像 + cheetah_self_interop 自闭环测试 + cheetah_to_zlm_interop / cheetah_to_pion_interop / cheetah_to_janus_interop 真实端到端 ignored 测试 + nightly CI sanity / weak-network 默认矩阵 + Windows 弱网等价方案文档 |
| [interop-runner.md](interop-runner.md) | 已发布 | 互操作 runner 操作手册（env、复现命令、CI 推荐流程，含 docker-compose 一键起说明） |
| [interop-docker-compose.yml](interop-docker-compose.yml) | 已发布 | ZLMediaKit + Pion helper + Playwright runner 组合的一键起 lab 模板 |

---

## 渐进式执行顺序

1. **Phase 02 follow-up** — 先做 driver shard。P2P 和互操作实体测试都依赖稳定的多会话传输层。
2. **Phase 05 follow-up** — 再做 P2P signaling。先完成本地 signaling 协议和 client job，再做 ZLM 对接。
3. **Phase 06** — 最后补外部互操作基础设施。实体测试需要前两项能力稳定后才能给出可靠结果。

---

## 总体验收

每个实现阶段完成后运行：

```powershell
cargo fmt
cargo clippy -p cheetah-webrtc-core
cargo clippy -p cheetah-webrtc-driver-tokio
cargo clippy -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-property-tests
```

外部互操作阶段额外运行：

```powershell
cargo test -p cheetah-webrtc-module --test interop -- --ignored
```

每个 ignored test 必须在 docstring 中记录环境变量、启动命令、期望观测值和失败 artifact 路径。

