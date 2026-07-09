# Phase 02 Follow-up — Driver 多线程 Shard

- **状态**: 第十三轮已落地（`RouteCandidateDiff` 让 `RouteUpdated` 携带迁移路径上的 candidate 差量；`ShardCandidateTable` + `shard_candidate_stats()` 按 shard 维度持久化最新 candidate 计数；module 层 8 个 Prometheus counter 把 `LocalCandidateSnapshot` 接入运营 dashboard）。第十四轮聚焦 TCP writer 跨 shard 物理迁移评估 + `RouteCandidateDiff` 业务层消费（candidate churn 告警）。

## 已完成（Phase 02 follow-up 第十二轮）

本轮把第十一轮列出的两项 “仍未落地” 同时落地：(1) `TcpWriterRegistry` 升级为 shard-aware，让 `evict_shard` / supervisor auto-evict 不再只清 directory 而把属于该 shard 的 TCP 连接也释放；(2) 新增 `WebRtcDriverEvent::LocalCandidateSnapshot`，把 `count_local_candidates` 在每条 `LocalDescription` 输出时按 owner shard 暴露给 module / 运营 dashboard。

### Section 1：TcpWriterRegistry shard-aware

- `cheetah-webrtc-driver-tokio/src/runner.rs` 中 `TcpWriterRegistry` 在原有 `addr → writer` map 旁新增 `addr → ShardId` 索引，并维护两份结构在 insert / remove 时的一致性。
- 新增 / 升级 API：
  - `TcpWriterRegistry::insert(addr, ShardId, writer)`：插入 writer 并记录 owner shard。
  - `TcpWriterRegistry::reassign_shard(addr, ShardId)`：把已存在的 writer owner 改写到新 shard，未知 addr 时为 no-op。
  - `TcpWriterRegistry::forget_shard(ShardId) -> usize`：一次性清理 owner == 目标 shard 的全部 writer，返回清理条目数。
  - `TcpWriterRegistry::len()`：返回当前 writer 总数（用于测试 / 诊断）。
- `tcp_accept_loop` 接受新连接时按 `remote_addr` 走 `ShardSelector` 选 owner shard（用 `remote_addr` 而不是 `WebRtcSessionId` 做 key，避免和 session 选择冲突），随后由 shard 在收到第一条 STUN binding-request 解析出真正的 ufrag 后通过 `reassign_shard` 把 owner 改写到正确的 shard，避免 owner 长期错位。
- 单元测试覆盖：insert/remove 同步 index、`forget_shard` 只清目标 shard 不影响其他 shard、`reassign_shard` 改写 owner、未知 addr `reassign_shard` / `forget_shard` 为 no-op。

### Section 2：evict_shard / supervisor auto-evict TCP cleanup

- `WebRtcDriverHandle::evict_shard(shard)` 在原有的 `route_directory.forget_shard(...)` 流程之外，再调用 `tcp_writers.forget_shard(shard)`，让该 shard 名下的 TCP 连接同时释放。
- `RouteDirectoryEvictionStats` 扩展新字段 `tcp_writers: usize`，把 TCP writer 清理数和原有 `sessions / addresses / ufrags / stale` 一起返回（向后兼容扩展，无需新增并行结构体；运营 dashboard 已经在消费这个结构体）。
- `io_front::spawn_shards` 的 supervisor auto-evict 分支（`shard_restart_on_panic == true` 命中 panic 时）同样调用 `tcp_writers.forget_shard(shard_id)`，并把数量写进 lifecycle diagnostic 文案：`tcp_writers={N}`，与 `sessions / addresses / ufrags / stale` 字段并列，方便运营按字段追踪清理量。
- 集成测试 `multishard_evict_shard_drops_tcp_writers`（既有，本轮验证仍通过）覆盖 evict 后 `tcp_writers.len() == 0`、aggregate session / route counter 与既有断言一致。

### Section 3：LocalCandidateSnapshot 事件

- `WebRtcDriverEvent` 枚举新增变体：

  ```rust
  WebRtcDriverEvent::LocalCandidateSnapshot {
      shard_id: ShardId,
      session_id: WebRtcSessionId,
      counts: LocalCandidateCounts,
  }
  ```

  保留既有的 `Debug, Clone` derive；`LocalCandidateCounts` 在第十一轮已经从 `cheetah_webrtc_driver_tokio` re-export，无需额外改动。
- 在 `run_shard_loop`（multi-shard 路径）和 `run_driver_core`（single-shard fast path）同样的位置，每条 `WebRtcCoreOutput::LocalDescription` 输出时复用 SDP 字符串调用 `count_local_candidates(&sdp)`，在对应的 `AnswerReady` / `OfferReady` 之前通过 `try_send` 投出一条 `LocalCandidateSnapshot`：1:1 与 `LocalDescription` 对齐；`try_send` 失败（背压）时丢弃，不阻塞主路径。Single-shard fast path 用 `ShardId(0)` 发，保持两路语义一致。
- `WebRtcModule::run_driver_event_worker`（module 层）新增 `LocalCandidateSnapshot` 分支：通过 `tracing::debug!(target: "webrtc.driver", shard_id, session_id, host, srflx, prflx, relay, udp, tcp, ipv4, ipv6, …)` 把字段结构化落到日志，不接入业务逻辑（keep module 层薄）。

### 测试

- `cheetah-webrtc-driver-tokio/tests/driver_smoke.rs::driver_emits_local_candidate_snapshot_after_answer`：单 shard，AcceptOffer 后断言收到一条 `LocalCandidateSnapshot { shard_id: ShardId(0), .. }`，并断言 `counts.total()` 与 answer SDP 中 `a=candidate:` 行数一致。
- `cheetah-webrtc-driver-tokio/tests/driver_multishard.rs::multishard_local_candidate_snapshot_carries_owner_shard`：4 shard、8 个并发 session，断言每个 session 收到的 `LocalCandidateSnapshot.shard_id` 等于 `RouteDirectory::lookup_session(session_id).unwrap()`。
- `TcpWriterRegistry` 新增单元测试覆盖 insert/remove index 同步、`forget_shard` 仅清目标 shard、`reassign_shard` 改写 owner、未知 addr no-op。
- 全量回归：`cargo fmt`、`cargo clippy -p cheetah-webrtc-driver-tokio`、`cargo test -p cheetah-webrtc-driver-tokio`（103 passed）、`cargo clippy -p cheetah-webrtc-module`、`cargo test -p cheetah-webrtc-module`（456 passed）全部通过。

## 已完成（Phase 02 follow-up 第十三轮）

本轮把第十二轮列出的 "仍未落地" 中三项同时落地：(1) `RouteUpdated` 事件携带 `RouteCandidateDiff`，让运营侧直接看到迁移路径上新增 / 移除 / 转 stale 的 remote candidate；(2) `ShardCandidateTable` + `shard_candidate_stats()` 按 shard 维度持久化最新 candidate 计数，运营 dashboard 不必自己累加事件；(3) module 层 8 个 Prometheus counter 把 `LocalCandidateSnapshot` 接入运营 dashboard 数据源。

### Section 1：`RouteCandidateDiff`

- `cheetah-webrtc-driver-tokio/src/migration.rs` 新增 `pub struct RouteCandidateDiff { pub added: Vec<SocketAddr>, pub removed: Vec<SocketAddr>, pub stale: Vec<SocketAddr> }`，derive `Debug, Clone, Default, PartialEq, Eq`。
- `WebRtcRouteUpdate` 新增 `pub diff: RouteCandidateDiff` 字段（向后兼容追加，不删 `previous_addr` / `new_addr`）。
- `route.rs` 的 `RouteTable` 三条路径升级为返回 `RouteCandidateDiff`：
  - `bind(addr, session, now)`：首次 bind 返回 `added: [addr]`；覆盖旧 session 时同时填 `removed` / `stale`。
  - `try_bind_migration(addr, session, now)`：成功路径返回 diff；rejected 路径返回 `RouteCandidateDiff::default()`。
  - `unbind_address(addr, now)`：返回 `stale: [addr], removed: [addr]`；未命中时返回空 diff。
  - `forget_session(session)`：返回 diff 列出本次清理的 active / stale 地址。
- `runner.rs` 的迁移成功分支（`handle_command_packet` 与 `handle_datagram`）构造 `WebRtcRouteUpdate` 时填充真实 diff。
- `lib.rs` re-export `RouteCandidateDiff`，与既有 `WebRtcRouteUpdate` 并列。
- 单元测试 5 条（`route::tests`）：`bind_first_time_returns_diff_with_only_added`、`bind_overwrite_same_addr_different_session_returns_added_removed_stale`、`unbind_address_returns_removed_and_stale_for_target_addr`、`try_bind_migration_success_returns_diff_added_for_new_addr`、`forget_session_returns_diff_listing_active_and_stale_addresses`。
- 单元测试 2 条（`migration::tests`）：`route_candidate_diff_default_is_all_empty`、`webrtc_route_update_with_default_diff_round_trips_via_clone`。
- 集成测试：`driver_route_updated_carries_candidate_diff_on_migration`（单 shard）、`multishard_route_updated_carries_candidate_diff_on_migration`（4 shard）。

### Section 2：`ShardCandidateTable` / `shard_candidate_stats()`

- `cheetah-webrtc-driver-tokio/src/directory.rs` 新增 `pub struct ShardCandidateTable`（`parking_lot::RwLock<Vec<LocalCandidateCounts>>`），按 `shard_count` 预分配。公共 API：
  - `record_snapshot(shard: ShardId, counts: LocalCandidateCounts)`：last-writer-wins 写入对应 shard 的最新计数。
  - `snapshot() -> Vec<WebRtcShardCandidateStats>`：按 shard id 顺序返回。
  - `clear_shard(shard: ShardId)`：重置目标 shard 为 `LocalCandidateCounts::default()`，supervisor auto-evict 时调用。
- `pub struct WebRtcShardCandidateStats { pub shard_id: ShardId, pub counts: LocalCandidateCounts }`，derive `Debug, Clone, Copy, PartialEq, Eq`。
- `runner.rs` 在 `run_shard_loop` / `run_driver_core` emit `LocalCandidateSnapshot` 的同一位置先调用 `shard_candidates.record_snapshot(shard_id, counts)`。
- `WebRtcDriverHandle::shard_candidate_stats(&self) -> Vec<WebRtcShardCandidateStats>`：薄包装 `Arc<ShardCandidateTable>::snapshot()`。
- `io_front::spawn_shards` 的 supervisor auto-evict 分支调用 `shard_candidates.clear_shard(shard_id)`，让目标 shard 候选计数归零。
- `lib.rs` re-export `ShardCandidateTable`、`WebRtcShardCandidateStats`。
- 单元测试 5 条（`directory::tests`）：`shard_candidate_table_default_is_zero`、`record_snapshot_updates_only_target_shard`、`record_snapshot_is_last_writer_wins`、`clear_shard_resets_only_target`、`snapshot_returns_entries_in_shard_id_order`。
- 集成测试：`driver_shard_candidate_stats_reflects_latest_snapshot`（单 shard）、`multishard_shard_candidate_stats_per_shard`（4 shard）、`multishard_shard_candidate_stats_clear_on_auto_evict`（auto-evict 后目标 shard 归零）。

### Section 3：Module Prometheus 指标

- `cheetah-webrtc-module/src/metrics.rs` 给 `WebRtcModuleMetrics` 新增 8 个 `AtomicU64` monotonic counter：
  - `webrtc_local_candidate_host_total`
  - `webrtc_local_candidate_srflx_total`
  - `webrtc_local_candidate_prflx_total`
  - `webrtc_local_candidate_relay_total`
  - `webrtc_local_candidate_udp_total`
  - `webrtc_local_candidate_tcp_total`
  - `webrtc_local_candidate_ipv4_total`
  - `webrtc_local_candidate_ipv6_total`
- `WebRtcModuleMetrics::record_local_candidate_snapshot(&self, counts: LocalCandidateCounts)`：把 `counts` 各字段累加到对应 counter（`Ordering::Relaxed`，与 packet counter 模式一致）。
- `WebRtcModuleMetricsSnapshot` 同步新增 8 个 `local_candidate_*_total` 字段；`assemble` 方法同步更新。
- `module.rs::run_driver_event_worker` 的 `LocalCandidateSnapshot` 分支在既有 `tracing::debug!` 之前调用 `metrics.record_local_candidate_snapshot(counts)`（log + metric 双写）。
- 单元测试 4 条（`metrics::tests`）：`record_local_candidate_snapshot_accumulates_per_type`、`record_local_candidate_snapshot_is_monotonic`、`local_candidate_counters_default_to_zero`、`assemble_includes_local_candidate_fields`。

### 新增公共 API

- `RouteCandidateDiff`（re-exported from `lib.rs`）
- `ShardCandidateTable`（re-exported from `lib.rs`）
- `WebRtcShardCandidateStats`（re-exported from `lib.rs`）
- `WebRtcDriverHandle::shard_candidate_stats(&self) -> Vec<WebRtcShardCandidateStats>`
- `WebRtcModuleMetrics::record_local_candidate_snapshot(&self, counts: LocalCandidateCounts)`
- `WebRtcModuleMetricsSnapshot::local_candidate_{host,srflx,prflx,relay,udp,tcp,ipv4,ipv6}_total` 字段

### 测试

- `cargo fmt`、`cargo clippy -p cheetah-webrtc-driver-tokio`、`cargo test -p cheetah-webrtc-driver-tokio`、`cargo clippy -p cheetah-webrtc-module`、`cargo test -p cheetah-webrtc-module` 全部通过。

## 已完成（Phase 02 follow-up 第十一轮）

- `cheetah-webrtc-driver-tokio/src/shard.rs` 新增 `StickyOverRebalanceStrategy`：
  - 把 “sticky 外层 + load-aware rebalance 内层” 落成具名类型，避免每个调用方手写 `StickyHashShardStrategy::new(Arc<LoadAwareRebalanceStrategy>, cap)`。
  - 内部持有两份引用：`inner_sticky: StickyHashShardStrategy` 与 `inner_rebalance: Arc<LoadAwareRebalanceStrategy>`。pick 走外层 sticky，forget 双层清理（外层 sticky + 内层 rebalance），避免 inner cache 残留导致 forget 后仍命中旧绑定的边界 case。
  - `with_default_capacity()` 提供 16k sticky + 8k rebalance + 256-tick refresh 三件套默认；`new(sticky_capacity, rebalance_capacity, refresh_interval_ticks)` 暴露细粒度调参。
  - 3 条单元测试：sticky 永久绑定（即使内层 rebalance 跨过 refresh 阈值也保持原 shard）、forget 双层清理、单 shard 透传。
- `lib.rs` re-export `StickyOverRebalanceStrategy`，与 `HashShardStrategy / LeastLoadedShardStrategy / StickyHashShardStrategy / BalancedStickyShardStrategy / LoadAwareRebalanceStrategy` 并列。
- 新增 `cheetah-webrtc-driver-tokio/src/sdp.rs` 模块：
  - `LocalCandidateCounts { host, srflx, prflx, relay, udp, tcp, ipv4, ipv6 }` 公共结构体 + `total()` 方法。
  - `count_local_candidates(sdp: &str) -> LocalCandidateCounts`：解析 `a=candidate:` 行的 7 字段位（foundation + component + transport + priority + addr + port + "typ" + type），按候选类型与传输与地址族三维度计数；malformed 行跳过；mDNS 主机名按 v4 计数。
  - 4 条单元测试：canonical 行 happy path、prflx + mDNS、malformed skip、空 SDP 返 0。
  - re-export 加入 `lib.rs`，方便上层 module / 运营代码从 `LocalDescription` 输出 SDP 直接拿到 candidate 分布做 dashboard / 诊断，不必拉测试 harness 进 dep 图。

## 已完成（Phase 02 follow-up 第十轮）

- `ShardChannels` 重写为 `parking_lot::RwLock<mpsc::Sender>`：`cmd_tx` / `packet_tx` 改为读写锁包装；新增 `cmd_sender()` / `packet_sender()` / `rebind(cmd_tx, packet_tx)` 三件套。前端 dispatch 路径改用 `cmd_sender()` / `packet_sender()` 拿到一份 sender 克隆，对 RW 锁的读争用极低（只有 supervisor rebind 时才写）。
- `IoFrontConfig::shards` 与 `spawn_shards` 返回值改为 `Vec<Arc<ShardChannels>>`，让 supervisor 与前端共享同一份 `Arc`，rebind 后前端立即看到新的 sender。
- `spawn_shards` 改造为 supervisor 循环：
  - 第一次 spawn 时使用初始的 `(cmd_rx, pkt_rx)`。
  - shard task 退出后发 `ShardStopped`，再决定是否 respawn：`cancelled || !auto_evict || !panicked || restarts >= max_restarts` 时停。
  - 命中 panic 时调 `forget_shard` + 重置 load + 同步 session_count + 发 `auto-evicted after panic` diagnostic。
  - 重 spawn 前 sleep `backoff_ms`（从 `shard_restart_backoff_ms` 起始翻倍封顶 `shard_max_restart_backoff_ms`），新建 `(cmd_rx, pkt_rx)` + 调 `Arc<ShardChannels>::rebind(...)` 把前端的 sender 切到新 receiver。
  - 进入下一轮循环 spawn `run_shard_loop`，发 `respawning attempt {i}/{max} (backoff next {ms}ms)` 诊断。
  - 预算耗尽后发 `reached max_restart_count` 诊断并停止 supervisor。
- 新增 `LoadAwareRebalanceStrategy`：
  - `new(inner, cache_capacity, refresh_interval_ticks)`：包装任意 inner，FIFO 缓存 session 选择，每 `refresh_interval_ticks` 次 pick 重新评估 inner（基于最新 `ShardLoadTable` 快照）。
  - `with_least_loaded_default()`：8k 缓存 + 256 ticks 默认。
  - `forget(session_id)`：释放绑定，让下一次 pick 重新决策。
  - 介于 `StickyHashShardStrategy`（永久绑定）与无状态 `LeastLoadedShardStrategy`（每次重新决策）之间，提供受控再均衡：sticky over short window，rebalance over long window。
  - 4 条单元测试：初次落到空闲 shard、缓存命中、超过 refresh 间隔后再均衡到新 least-loaded、单 shard 透传、forget 释放。
- `lib.rs` 新增 `LoadAwareRebalanceStrategy` re-export。

## 已完成（Phase 02 follow-up 第九轮）

- `WebRtcDriverConfig` 新增四个公共字段控制 shard 退出行为：
  - `shard_restart_on_panic: bool`（默认 `false`）：开启后 supervisor 在 shard panic 时自动跑 `evict_shard` 等价的清理流程。
  - `shard_max_restart_count: u32`（默认 `3`）：未来 respawn 的预算上限，目前自动清理路径不消耗。
  - `shard_restart_backoff_ms: u64`（默认 `250`）：未来 respawn 的初始退避，目前未生效。
  - `shard_max_restart_backoff_ms: u64`（默认 `30_000`）：未来 respawn 退避的上界。
- `io_front::spawn_shards` supervisor 在 join 完 shard task 后判断 `panicked && !cancelled && config.shard_restart_on_panic == true`，命中时执行：
  1. `route_directory.forget_shard(shard_id)` 清理 directory 条目。
  2. `shard_loads.record_route_counts(shard_id, 0, 0)` 重置 active/stale。
  3. 按 `evicted.sessions` 次数 `record_session_removed(shard_id)` 减回 shard 计数。
  4. 全局 `session_count` saturating_sub `evicted.sessions`。
  5. 发 `WebRtcDriverDiagnosticKind::Lifecycle` 诊断（消息含四维度清理量）。
- 真正的 shard task 自动 respawn 推迟到下一轮：当前 supervisor 不会重新 spawn `run_shard_loop`；自动重启需要前端 `ShardChannels` 持有的 sender 能 rebind 到新 receiver，这是侵入性更强的设计变化。第十轮起用 `shard_restart_on_panic` 已经能让运营在 panic 时自动看到清理状态而不是手动调 `evict_shard`。
- `cheetah-webrtc-module/src/config.rs` 新增四个字段的默认值（`false / 3 / 250 / 30_000`），保持模块层默认行为兼容。
- 新增集成测试 `multishard_restart_on_panic_config_does_not_break_graceful_cancel`：开启 `shard_restart_on_panic = true` 后，graceful cancel 路径仍按原样收 `ShardStopped { reason: cancelled }`，且不发 auto-evict diagnostic。

## 已完成（Phase 02 follow-up 第八轮）

- `cheetah-webrtc-driver-tokio/src/directory.rs` 新增 `RouteDirectoryEvictionStats { sessions, addresses, ufrags, stale }` 公共结构体，专门用于报告 `forget_shard` 的清理量。
- `RouteDirectory::forget_shard(shard) -> RouteDirectoryEvictionStats`：一次锁内部把目标 shard 拥有的所有 `session_to_shard / remote_to_entry / ufrag_to_shard / stale` 条目清理掉，并把删除计数按四个维度返回。`remote_to_entry` 与 `stale` 的清理同时考虑 `entry.shard == shard` 与 `session_set.contains(&session)`，因为迁移路径下 entry.shard 可能被 directory 改写但 session 仍属于原 shard。
- `WebRtcDriverHandle::evict_shard(shard)`：handle 层 thin wrapper，按顺序更新 `route_directory.forget_shard(...)` → 调用 `shard_loads.record_route_counts(shard, 0, 0)` 重置 active/stale → 按 evicted.sessions 次 `record_session_removed` 重置 session_count → 全局 `session_count` saturating_sub 同步。返回 `RouteDirectoryEvictionStats` 给运营侧记录。
- `lib.rs` re-export `RouteDirectoryEvictionStats`。
- 单元测试 2 条（directory）：`forget_shard_drops_all_bindings_owned_by_shard`（多 shard 隔离，目标 shard 全清，其他 shard 不动）、`forget_shard_returns_zero_for_unknown_shard`。
- 集成测试 1 条（multishard）：`multishard_evict_shard_drops_directory_and_load_counters`（4 shard 各 1 session、evict 一个、目标 shard `session_count == 0`、其他 shard `session_count == 1`、aggregate `session_count` 从 2 → 1）。

## 已完成（Phase 02 follow-up 第七轮）

- `cheetah-webrtc-driver-tokio/src/shard.rs` 新增 `BalancedStickyShardStrategy`：
  - `BalancedStickyShardStrategy::new(cache_capacity)` 构造一个 `StickyHashShardStrategy::new(Arc<LeastLoadedShardStrategy>, cache_capacity)` 内部委托。
  - `BalancedStickyShardStrategy::with_default_capacity()` 提供 16k cache 默认入口。
  - `BalancedStickyShardStrategy::forget(session_id)` 透传给 inner sticky，方便会话彻底退出后释放绑定。
  - `ShardSelectorStrategy::pick` 直接调用 inner，行为等价于第六轮的 “sticky over least-loaded” 配方，但代码可读、`Arc::new(BalancedStickyShardStrategy::new(8_192))` 一行即可。
- `lib.rs` re-export `BalancedStickyShardStrategy`，与 `HashShardStrategy / LeastLoadedShardStrategy / StickyHashShardStrategy` 并列。
- 单元测试 2 条覆盖：第一次 pick 落到空闲 shard（即使 `WebRtcSessionId` 在 hash 上属于已饱和的 shard）；`forget(session_id)` 后 load 翻转，新 pick 跟随新 load 而非旧绑定。

## 已完成（Phase 02 follow-up 第六轮）

- `shard.rs` `ShardLoad` 升级为 `{ session_count, active_routes, stale_routes }` 三元组；`ShardLoadTable::record_route_counts(shard, active, stale)` 让 shard 在每轮事件循环结束发布本地 RouteTable 计数。`route.rs` 新增 `RouteTable::route_counts(&self) -> (usize, usize)` 私有 helper。`run_shard_loop` 在 compaction 同一周期内调用 `shard_loads.record_route_counts(shard_id, active, stale)`，使 `WebRtcDriverHandle::shard_stats()` 在 multi-shard 模式下报告真实 per-shard 路由数；single-shard fast path 仍把 directory aggregate 投影到 shard 0（与既有契约一致）。
- `StickyHashShardStrategy { inner: Arc<dyn ShardSelectorStrategy>, cache_capacity: usize }`：第一次 pick 时委托 inner（默认 hash），之后稳定返回缓存值，避免 ICE restart 在 least-loaded 策略下漂移到其他 shard。FIFO 缓存 + `forget(session_id)` 释放。`StickyHashShardStrategy::with_hash_default()` 提供 16k 缓存的便捷入口。Re-export 加入 `lib.rs`。
- `WebRtcDriverEvent::ShardStopped { shard_id, reason }` 新事件 + `io_front::spawn_shards` 的 supervisor 任务：每个 shard 由内部 `tokio::spawn` 跑实际 loop，外层 supervisor `await` 内层 join handle，按 `Ok(()) / JoinError::is_cancelled / JoinError::is_panic / generic JoinError` 派生 `reason` 并 send 到 event channel。当前不做自动重启（panic 通常意味着确定性状态损坏）。
- `WebRtcModule` 的 driver event worker 新增 `ShardStopped` 分支：`cancelled / exited` 走 debug，其他（含 `panic: ...`）走 warn — 让运营 dashboard 直接区分 graceful drain 与崩溃。
- `tests/driver_multishard.rs` 新增 2 条集成测试：
  - `multishard_cancel_emits_shard_stopped_for_each_shard`（cancel 后每 shard 收到一条 `ShardStopped { reason: cancelled / exited }`，shard_id 集合等于 `0..shard_count`）
  - `multishard_route_counts_track_per_shard`（4 个 session fan out 到 4 个 shard，per-shard `session_count == 1`、`active_routes == 0`、`stale_routes == 0` — 没有 inbound 包之前 routes 应保持 0）
- `shard::tests` 新增 5 条单元测试：sticky 缓存命中、forget 释放、capacity eviction、single-shard 透传、route counts 增量发布。

## 已完成（Phase 02 follow-up 第五轮）

- `cheetah-webrtc-driver-tokio/src/io_front.rs` 新增 I/O 前端：
  - `dispatch_command(...)`：命令按 session id 路由到 owner shard。`AcceptOffer/CreateOffer` 走 `ShardSelector::pick`（配合 `ShardLoadTable`），其他命令走 `RouteDirectory::lookup_session(session_id)`。未知 session 的命令产出 `Lifecycle` diagnostic 并丢弃，不再静默吞掉。
  - `dispatch_datagram(...)`：包按 source addr 走 `RouteDirectory::lookup_remote`；未命中时解析 STUN binding-request USERNAME 走 `lookup_ufrag`；都未命中则发 `UnroutedPacket` diagnostic + bump `unrouted_packets_total`，不再向所有 shard 广播（避免 thundering herd）。
  - `IoFrontConfig` + `run_io_front(...)`：把 `WebRtcDriverHandle` 上的全局命令 / 包通道接到前端任务。
  - `spawn_shards(...)`：根据 `effective_shard_count()` 拉起 N 个 `run_shard_loop` 任务，并把命令 / 包通道返回给前端。
- `cheetah-webrtc-driver-tokio/src/runner.rs` 新增 `ShardCommand` envelope（per-shard 命令通道携带的内部类型，`pub(crate)`）和 `run_shard_loop(...)`：
  - 与 `run_driver_core` 同样的状态机维护（核心 tick、握手 watchdog、route compaction、backpressure 监控），但所有 session 都被 pin 在该 shard，commands / packets 直接从 per-shard channel 取。
  - 每次 `LocalDescription` 输出（`AnswerReady` / `OfferReady`）解析 SDP 中的 `a=ice-ufrag:` 并 `RouteDirectory::register_ufrag(ufrag, shard_id)`，使初始 STUN binding 请求能直接 fast-path 到本 shard。`extract_local_ufrag_from_sdp` 是简单的逐行扫描（不引入新依赖）。
  - `CloseSession` 路径除了 `route_directory.forget_session(...)` 还调用 `forget_ufrag(...)` 清理 ufrag → shard 映射。
  - shard 0 负责 `route_directory.compact_expired(...)`，避免 N 个 shard 同时抢 directory 锁。
- `cheetah-webrtc-driver-tokio/src/stun.rs` 新增最小 STUN binding-request USERNAME 解析（仅提取 local ufrag，不验证 message integrity；那是 str0m 在 shard 内的事）。8 条单元测试覆盖 happy path、短包、DTLS / RTP 误判、缺 USERNAME、缺分隔符、错 magic cookie、binding response。
- `spawn_driver` 当 `effective_shard_count() > 1` 时拉起前端 + shard topology；`== 1` 时保持原 `run_driver_core` fast path 不动（既有测试不受影响）。
- 新增 `tests/driver_multishard.rs` 4 条集成测试：
  - `multishard_session_lands_on_selector_chosen_shard`（session 落到 selector 选定的 shard）
  - `multishard_concurrent_sessions_distribute_across_shards`（16 个 session 在 4 个 shard 上分布、close 后回落到 0）
  - `multishard_unrouted_packet_emits_diagnostic_at_front_end`（前端 UnroutedPacket diagnostic）
  - `multishard_stop_unknown_session_emits_diagnostic`（未知 session 的命令路由产出 `Lifecycle` diagnostic）

## 已完成（Phase 02 follow-up 第四轮）

- `cheetah-webrtc-driver-tokio/src/shard.rs` 引入 `ShardSelectorStrategy` trait：`pick(session_id, shard_count, &ShardLoadTable) -> ShardId`。
  - `HashShardStrategy`：splitmix64 hash，session 与 shard 之间是稳定映射（默认）。
  - `LeastLoadedShardStrategy`：选 `session_count` 最小的 shard，相同负载用 hash 决定，所以仍然是确定性的。
- `ShardSelector::with_strategy(shard_count, Arc<dyn ShardSelectorStrategy>)`：让运营端按需切换策略；默认 `ShardSelector::new(shard_count)` 仍是 hash。`pick(session_id, &loads)` 与 `pick_no_loads(session_id)` 双入口，避免无 load 表的调用方多余分配。
- `ShardLoad` 升级为公共类型，方便外部策略读取负载快照。
- `run_driver_core`：accept 路径使用 `pick(session_id, &shard_loads)`；migrate / close 路径改为读 `RouteDirectory::lookup_session(session_id)`，避免 least-loaded 策略下重新 pick 得到不同 shard。
- 驱动公开类型：`HashShardStrategy / LeastLoadedShardStrategy / ShardLoad / ShardSelectorStrategy` 新加入 `lib.rs` re-export。
- 单元测试 7 条（默认 hash 4 条 + LeastLoaded 3 条）：emptiest shard、tie 决定性、单 shard 直接返回 0。

## 已完成（Phase 02 follow-up 第三轮）

- `WebRtcDriverHandle::drain_within(timeout) -> bool`：异步轮询 `session_count()` 直到归零或超时；返回值告诉调用方是否在窗口内完成 graceful 关停。25ms 轮询周期，cheap 到可以在每次 reload 前直接调用。
- 集成测试 2 条（`tests/driver_shard.rs`）：
  - `driver_drain_within_returns_true_when_no_sessions`（idle driver 立即返回 true，避免空跑等待）；
  - `driver_drain_within_waits_for_session_close`（drain 与 StopSession 并发，事件循环耗尽后返回 true，并断言 `session_count` 归零）。

## 已完成（Phase 02 follow-up 第一轮）

- `cheetah-webrtc-driver-tokio/src/directory.rs`：全局 `RouteDirectory` 模块，提供 `session/remote/ufrag/stale` 四类映射和 `register_session / register_ufrag / bind_remote / migrate_remote / lookup_* / forget_session / compact_expired` 公共 API。配置项 `address_capacity / stale_capacity / stale_ttl` 自带默认值并和 `migration_route_ttl_ms` 对齐。
- `WebRtcDriverConfig` 新增 `driver_shards / shard_command_capacity / route_directory_capacity / route_directory_stale_capacity` 字段，及 `effective_shard_count()` 帮助方法（`0` 解析为 `available_parallelism()`，最少 1）。
- `WebRtcDriverHandle` 新增 `shard_count()`、`shard_stats()`（返回 `Vec<WebRtcShardStats>`）、`route_directory()` 公共 API。`stats_snapshot()` 同时输出 `shard_count` 和 `route_directory` 子结构。
- `WebRtcDriverDiagnosticKind::RouteDirectoryFull` 新枚举变体：directory 命中容量上限时的诊断。
- `run_driver_core` 在每条 `AcceptOffer / CreateOffer` 命中后自动 `register_session(session_id, ShardId(0))`；`handle_datagram` 在迁移分支同时调用 `migrate_remote`，迁移失败发出 `MigrationRejected` 诊断；`CloseSession` 路径同步 `forget_session`。
- `WebRtcModuleConfig::shard_count` 字段（默认 `0`，已存在）现在透传到驱动；driver config 的 directory 容量按 `max_sessions * 2` 估算，stale cap 按 `max_sessions` 估算。
- 集成测试：`tests/driver_shard.rs` 覆盖 `driver_shards=0` 解析、`driver_shards=4` 公共 API、session 创建后 `RouteDirectory::lookup_session` 命中 `ShardId(0)`、close 后从 directory 移除。
- 单元测试：`directory::tests` 覆盖 register/lookup、ufrag、bind/migrate、容量上限、stale 过期、`forget_session`、stats snapshot、stale 满时驱逐最旧条目，共 11 条。

## 已完成（Phase 02 follow-up 第二轮）

- `cheetah-webrtc-driver-tokio/src/shard.rs`：
  - `ShardSelector::new(shard_count)` + `pick(WebRtcSessionId) -> ShardId`：splitmix64 风格 hash，单 shard 时直接返回 0；多 shard 时分布稳定（随机扫描 1024 个 id 触达每个 bucket，验证已写为单元测试）。
  - `ShardLoadTable::new(shard_count)` + `record_session_added / record_session_removed / snapshot()`：按 shard 维度记录会话计数，snapshot 按 ShardId 顺序返回。
  - 4 条单元测试覆盖单 shard、多 shard 分布、确定性、负 saturation。
- `WebRtcDriverHandle::shard_selector()` 公开方法（返回 `ShardSelector` 克隆），`shard_stats()` 现在用 `ShardLoadTable::snapshot()` 输出真实的 per-shard 计数（不再把全部计数塞到 ShardId(0)）。
- `run_driver_core`：
  - 接受 `AcceptOffer / CreateOffer` 时调用 `shard_selector.pick(session_id)` 得到 owner shard，并同步 `route_directory.register_session(...)` + `shard_loads.record_session_added(...)`。
  - 迁移路径 `migrate_remote(...)` 也使用 `shard_selector.pick(session_id)`，保证 directory 上的 shard 与 selector 一致。
  - `CloseSession` 路径调用 `shard_loads.record_session_removed(shard_selector.pick(session_id))`，确保多 shard 模式下计数正确回落。
- 集成测试 `tests/driver_shard.rs` 增加 2 条：`driver_shard_selector_distributes_sessions_by_id`（256 个 id 在 4 个 shard 全命中）、`driver_shard_loads_track_active_sessions`（创建 + 关闭 session，断言 `shard_stats()` 中对应 shard 的 `session_count` 从 1 → 0）。

## 仍未落地（下一轮）

- TCP writer 跨 shard 真实迁移转移：当前 `TcpWriterRegistry` 在 STUN 解析出真正 owner shard 后只调用 `reassign_shard(addr, new_shard)` 改写索引，writer 实例本身仍由前端 accept loop 持有；shard panic / cancel / evict 时通过 `forget_shard` 物理释放该 shard 的 writer，依赖对端 TCP 重连重建。评估两个方向二选一：(a) 实现 writer 物理迁移（把 `OwnedWriteHalf` 作为 message 在 shard 通道间转交，目标 shard 接管发送责任）；(b) 在文档与代码注释中显式记录 "forget on owner change" 是设计选择而非待办，并删除本条候选。**前提：需要先用真实压测数据评估 TCP 重连成本是否值得引入跨 shard writer ownership 转移的复杂度，再决定方案方向。**
- `RouteCandidateDiff` 进 module event worker → 业务层 candidate churn 告警：第十三轮已让 `RouteUpdated` 携带 `RouteCandidateDiff { added, removed, stale }`，但 module 层 `run_driver_event_worker` 的 `RouteUpdated` 分支尚未消费 `diff` 字段。下一轮在 module 侧解析 diff，当单位时间内 `removed + stale` 数量超过阈值时产出 candidate churn 告警（`WebRtcDriverDiagnosticKind::CandidateChurn` 或等价 Prometheus alert rule），让运营 dashboard 能主动感知异常迁移频率，而不是事后翻日志。
## 设计目标

本阶段把 WebRTC driver 从单任务模型拆成前端 I/O + 多个 session owner shard。目标是支撑多线程高并发，同时保持 `cheetah-webrtc-core` Sans-I/O 和 `str0m` 状态机边界。

## 2.1 Shard 文件结构

建议拆分 `cheetah-webrtc-driver-tokio`：

```text
src/
  runner.rs          # public spawn_driver / handle / command fanout
  io_front.rs        # UDP/TCP accept/read front
  shard.rs           # per-shard event loop and WebRtcCore owner
  directory.rs       # global route directory
  route.rs           # existing route table, shard-local
  tcp.rs             # RFC4571 decoder/encoder/writer registry
  migration.rs       # route update structs
  config.rs
```

`runner.rs` 保持 public API 稳定，内部把原单 loop 迁移到 `shard.rs`。

## 2.2 Public API 调整

`WebRtcDriverConfig` 增加：

```rust
pub driver_shards: usize,          // 0 = auto
pub shard_command_capacity: usize,
pub shard_event_capacity: usize,
pub route_directory_capacity: usize,
pub shard_stats_interval_ms: u64,
```

`WebRtcDriverHandle` 增加：

```rust
pub fn shard_count(&self) -> usize;
pub fn shard_stats(&self) -> Vec<WebRtcShardStats>;
```

新增事件：

```rust
WebRtcDriverEvent::ShardStarted { shard_id }
WebRtcDriverEvent::ShardStopped { shard_id, reason }
WebRtcDriverEvent::ShardStats { snapshot }
WebRtcDriverEvent::RouteDirectoryUpdated { session_id, shard_id }
```

## 2.3 Route directory

新增 `RouteDirectory`：

- `session_to_shard: HashMap<WebRtcSessionId, ShardId>`
- `remote_to_shard: HashMap<SocketAddr, ShardId>`
- `ufrag_to_shard: HashMap<String, ShardId>`
- `stale_remote_to_shard: HashMap<SocketAddr, ShardId>`

方法：

- `register_session(session_id, shard_id)`
- `register_ufrag(ufrag, shard_id)`
- `bind_remote(remote_addr, shard_id)`
- `move_remote_to_stale(remote_addr, shard_id, expires_at)`
- `lookup_by_session(session_id)`
- `lookup_by_remote(remote_addr)`
- `lookup_by_ufrag(ufrag)`
- `compact_expired(now)`

目录只保存路由，不保存 `WebRtcCore` 或 session 对象。

## 2.4 Shard 选择策略

首版策略：

- `driver_shards=0`：`available_parallelism()`，最少为 1。
- 新 session 默认 `session_id % shard_count`。
- 后续可扩展 least-loaded，但首版不做动态迁移。

理由：

- session owner 固定，避免 `str0m::Rtc` 状态跨线程移动。
- hash 策略稳定、可测试、实现简单。
- 负载不均后续通过 metrics 观察再优化。

## 2.5 I/O 前端路由

UDP/TCP 前端收到 packet：

1. 按 remote addr 查 directory。
2. 命中则投递给 shard。
3. 未命中则尝试解析 STUN ufrag。
4. ufrag 命中则投递给 shard。
5. 仍未命中则发 `UnroutedPacket`。

TCP writer：

- TCP connection 由前端 accept。
- writer handle 可保存在前端 registry，并通过 remote addr 发送。
- shard 产出 outbound packet 时通过 front send command 发送，避免 writer half 跨 shard 混乱。
- 若保持 shard-local writer registry，则 TCP connection 在确定 owner shard 后转移 writer ownership；文档默认采用前端 registry，降低所有权复杂度。

## 2.6 Shard event loop

每个 shard 持有：

- `WebRtcCore`
- shard-local `RouteTable`
- timer heap
- session count
- pending output buffer
- command receiver
- packet receiver

Shard 处理：

- `CreateSession`：只在 owner shard 执行。
- `StopSession`：按 session id 投递到 owner shard。
- `SendFrame` / `SendDataChannel` / `RequestKeyframe`：按 session id 投递。
- network packet：前端按 directory 投递。
- core output：session route update 回写 directory，outbound packet 发给 I/O front。

## 2.7 连接迁移一致性

迁移流程：

1. shard 内 core 接受新 remote addr。
2. shard-local route table 更新 active/stale。
3. shard 向 directory 发原子 update 请求。
4. directory 如果发现新 remote addr 已属于其他 active session，拒绝。
5. 拒绝时 shard 发 `MigrationRejected`，丢弃 packet。
6. 成功时发 `RouteUpdated`。

约束：

- session 不跨 shard 迁移。
- remote addr 不能同时 active 绑定两个 session。
- stale route 保持原 shard，TTL 到期删除。

## 2.8 Candidate policy

Shard 化后补 candidate 诊断：

- local UDP host candidate count。
- local TCP passive candidate count。
- remote candidate accepted/rejected count。
- reject reason：policy、capacity、invalid、private-address。

`ice_transport_policy`：

- `all`：host/srflx/relay 都允许。
- `relay-only`：只接受 relay，host/srflx candidate 输出诊断后过滤。
- `p2p-only`：拒绝 relay。

## 2.9 测试要求

新增 driver 测试：

- `driver_shards_zero_uses_auto_at_least_one`
- `create_session_registers_owner_shard`
- `commands_are_routed_to_owner_shard`
- `udp_packet_routes_by_remote_addr_to_owner_shard`
- `unbound_stun_routes_by_ufrag_to_owner_shard`
- `migration_updates_directory_and_shard_route_table`
- `migration_rejects_remote_addr_owned_by_other_shard`
- `stop_session_removes_directory_entries`
- `shard_stats_reports_session_and_route_counts`

运行：

```powershell
cargo test -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-core
```

## 完成后检查

- `cheetah-webrtc-core` 没有新增 async、Tokio、socket 依赖。
- `driver_shards=1` 行为与原单 driver task 兼容。
- TCP/UDP outbound 在 shard 模式下都能发送。
- Route directory 和 shard route table 关闭后无残留。

