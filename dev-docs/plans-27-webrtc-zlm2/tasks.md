# Phase 02 follow-up — RouteCandidateDiff + ShardCandidateTable + LocalCandidateSnapshot Prometheus 指标（第十三轮）

本 spec 落地 `dev-docs/plans-27-webrtc-zlm2/phase-02-driver-multithread-shard.md` "仍未落地（下一轮）" 中的三项（与 `index.md` 状态行对齐；TCP writer 跨 shard 真实迁移转移延后到下一轮，先用真实压测数据评估再决定方案）：

1. `RouteUpdated` 事件携带 `RouteCandidateDiff { added, removed, stale }`：把 shard-local `RouteTable` 在 `bind / unbind_address / try_bind_migration / forget_session` 三条路径上的 remote address 差量收敛上来，向后兼容地新增字段。
2. `ShardCandidateTable` + `WebRtcDriverHandle::shard_candidate_stats() -> Vec<WebRtcShardCandidateStats>`：按 shard 维度持久化最新 `LocalCandidateSnapshot` 计数（host/srflx/prflx/relay/udp/tcp/ipv4/ipv6），让运营 dashboard 不必自己累加事件。
3. module 层把 `LocalCandidateSnapshot` 从 `tracing::debug!` 升级为 Prometheus 指标：`webrtc.driver.local_candidate_*` 八个 counter（host/srflx/prflx/relay/udp/tcp/ipv4/ipv6），与现有 `webrtc.driver.diagnostic_*` 指标并列；保留既有 debug 日志。

参考目录：
- `crates/protocols/webrtc/driver-tokio/src/migration.rs`（`WebRtcRouteUpdate` 结构体）
- `crates/protocols/webrtc/driver-tokio/src/runner.rs`（`RouteUpdated` 发出点 + `LocalCandidateSnapshot` 发出点）
- `crates/protocols/webrtc/driver-tokio/src/route.rs`（`RouteTable` bind / unbind_address / try_bind_migration / forget_session / compact_expired）
- `crates/protocols/webrtc/driver-tokio/src/directory.rs`（`ShardLoadTable` 旁的 `WebRtcShardStats` 模式，`shard_candidate_stats` 走相同模式）
- `crates/protocols/webrtc/driver-tokio/src/io_front.rs`（multi-shard `LocalCandidateSnapshot` 投递点）
- `crates/protocols/webrtc/driver-tokio/src/sdp.rs`（`LocalCandidateCounts`，作为 ShardCandidateTable 的存储单元）
- `crates/protocols/webrtc/module/src/metrics.rs`（`WebRtcModuleMetrics` 计数器模式 + snapshot helper）
- `crates/protocols/webrtc/module/src/module.rs`（`run_driver_event_worker` 的 `LocalCandidateSnapshot` 分支已在第十二轮落地，本轮在该分支内 bump counter）

## 任务清单

- [x] 1. `RouteUpdated` 携带 `RouteCandidateDiff`
  - [x] 1.1 在 `migration.rs` 新增 `pub struct RouteCandidateDiff { pub added: Vec<SocketAddr>, pub removed: Vec<SocketAddr>, pub stale: Vec<SocketAddr> }`，derive `Debug, Clone, Default, PartialEq, Eq`；`Default` 用于既有调用方在不计算 diff 时回退到空 diff
  - [x] 1.2 在 `WebRtcRouteUpdate` 添加 `pub diff: RouteCandidateDiff` 字段（向后兼容地追加字段，不删 `previous_addr` / `new_addr`）；既有所有 `WebRtcRouteUpdate { session_id, previous_addr, new_addr, .. }` 构造点先填 `RouteCandidateDiff::default()` 占位，后续步骤再换上真实 diff
  - [x] 1.3 `route.rs` 的 `RouteTable` 三条路径升级为返回 `RouteCandidateDiff`：
    - `bind(addr, session, now)`：返回 `(prior, RouteCandidateDiff { added: [addr], removed: prior.map(|_| addr).into_iter().collect(), stale: ... })`，其中 `stale` 在覆盖旧 session 时填一份；首次 bind 时 `removed` / `stale` 为空
    - `try_bind_migration(addr, session, now)`：成功路径返回 `(Result<Option<WebRtcSessionId>>, RouteCandidateDiff)`；rejected 路径返回 `(Err(()), RouteCandidateDiff::default())`
    - `unbind_address(addr, now)`：返回 `RouteCandidateDiff { stale: [addr], removed: [addr], added: [] }`（active → stale 同时算 stale 与 removed）；未命中时返回空 diff
    - `forget_session(session)`：升级为返回 `RouteCandidateDiff { removed: vec![..], stale: vec![..] }`，列出本次清理掉的 active / stale 地址
  - [x] 1.4 `runner.rs` 的两处 `WebRtcDriverEvent::RouteUpdated(WebRtcRouteUpdate { ... })` 构造点（迁移成功分支：约 1779 行 `handle_command_packet` 与 2341 行 `handle_datagram`）替换为带 diff 版本：先在 `try_bind_migration` 之前 snapshot 当前 active addr 集合（仅相关 session），成功后用 `bind_remote` / `unbind_address` 返回的 diff 合并填充 `WebRtcRouteUpdate.diff`
  - [x] 1.5 `forget_session` / `CloseSession` 路径不再 emit `RouteUpdated`（这是新行为吗？检查既有逻辑：close 路径不发 RouteUpdated，只发 lifecycle，不动）；只是把返回的 diff 用作未来扩展点，本轮不向上传
  - [x] 1.6 单元测试 `route::tests`：
    - `bind_first_time_returns_diff_with_only_added`
    - `bind_overwrite_same_addr_different_session_returns_added_removed_stale`
    - `unbind_address_returns_removed_and_stale_for_target_addr`
    - `try_bind_migration_success_returns_diff_added_for_new_addr`
    - `forget_session_returns_diff_listing_active_and_stale_addresses`
  - [x] 1.7 `migration::tests` 新增（既有文件无 tests 模块，需要新建）：
    - `route_candidate_diff_default_is_all_empty`
    - `webrtc_route_update_with_default_diff_round_trips_via_clone`
  - [x] 1.8 `lib.rs` re-export `RouteCandidateDiff`，与既有 `WebRtcRouteUpdate` 并列

- [x] 2. 集成测试：迁移路径上 `RouteUpdated` 携带真实 diff
  - [x] 2.1 `tests/driver_smoke.rs::driver_route_updated_carries_candidate_diff_on_migration`：
    - 单 shard，AcceptOffer + 第一条 STUN binding-request 从 addr_a 进 → 触发 `bind_remote` 但 addr_a 是首次绑定，不发 `RouteUpdated`
    - 第二条 binding-request 从同 session 但不同 addr_b 进 → 触发 migration，断言收到 `WebRtcDriverEvent::RouteUpdated(WebRtcRouteUpdate { diff, .. })`，其中 `diff.added == [addr_b]`、`diff.removed == [addr_a]`、`diff.stale == [addr_a]`
  - [x] 2.2 `tests/driver_multishard.rs::multishard_route_updated_carries_candidate_diff_on_migration`：
    - 4 shard，模拟同 session 在 owner shard 上从 addr_a 迁移到 addr_b，断言事件 stream 收到一条 `RouteUpdated`，`diff` 字段非空且与单 shard 路径一致
  - [x] 2.3 既有 `tests/driver_smoke.rs` 中所有引用 `WebRtcRouteUpdate { ... }` 的断言（如果直接 pattern match）按需补 `..` 或 `diff: _` 防止 strict pattern 失败

- [x] 3. `ShardCandidateTable` + `WebRtcDriverHandle::shard_candidate_stats()`
  - [x] 3.1 `directory.rs` 新增 `pub struct ShardCandidateTable { inner: parking_lot::RwLock<Vec<LocalCandidateCounts>> }`，构造时按 `shard_count` 预分配；公共 API：
    - `record_snapshot(shard: ShardId, counts: LocalCandidateCounts)`：写入对应 shard 的最新一条（last-writer-wins，与 `WebRtcModuleMetrics` 的 gauge 语义一致）
    - `snapshot() -> Vec<WebRtcShardCandidateStats>`：返回 `Vec<WebRtcShardCandidateStats { shard_id, counts: LocalCandidateCounts }>`，按 shard id 顺序返回
    - `clear_shard(shard: ShardId)`：把对应 shard 重置为 `LocalCandidateCounts::default()`，supervisor auto-evict 时调用
  - [x] 3.2 `pub struct WebRtcShardCandidateStats { pub shard_id: ShardId, pub counts: LocalCandidateCounts }`，derive `Debug, Clone, Copy, PartialEq, Eq`
  - [x] 3.3 `runner.rs` 新增字段 `shard_candidates: Arc<ShardCandidateTable>`，在 `spawn_driver` 构造时按 `effective_shard_count()` 预分配
  - [x] 3.4 在 `run_shard_loop` / `run_driver_core` emit `LocalCandidateSnapshot` 的同一位置，先调用 `shard_candidates.record_snapshot(shard_id, counts)`，再投出事件
  - [x] 3.5 `WebRtcDriverHandle::shard_candidate_stats(&self) -> Vec<WebRtcShardCandidateStats>`：薄包装 `Arc<ShardCandidateTable>::snapshot()`，与既有 `shard_stats()` 并列
  - [x] 3.6 `io_front::spawn_shards` 的 supervisor auto-evict 分支（`shard_restart_on_panic = true` 命中 panic）调用 `shard_candidates.clear_shard(shard_id)`，让运营看到清理的 shard 候选计数归零
  - [x] 3.7 `lib.rs` re-export `ShardCandidateTable`、`WebRtcShardCandidateStats`
  - [x] 3.8 单元测试 `directory::tests`（覆盖 ShardCandidateTable）：
    - `shard_candidate_table_default_is_zero`
    - `record_snapshot_updates_only_target_shard`
    - `record_snapshot_is_last_writer_wins`
    - `clear_shard_resets_only_target`
    - `snapshot_returns_entries_in_shard_id_order`

- [x] 4. 集成测试：`shard_candidate_stats()` 与事件序列对齐
  - [x] 4.1 `tests/driver_smoke.rs::driver_shard_candidate_stats_reflects_latest_snapshot`：
    - 单 shard，AcceptOffer 后断言 `handle.shard_candidate_stats()` 长度 == 1，`shard_id == ShardId(0)`，`counts.total()` 与 `LocalCandidateSnapshot` 事件载荷一致
  - [x] 4.2 `tests/driver_multishard.rs::multishard_shard_candidate_stats_per_shard`：
    - 4 shard，并发创建 4 个 session（一个 shard 一个），断言 `shard_candidate_stats()` 长度 == 4，每个 shard 的 `counts` 与对应 session 的 `LocalCandidateSnapshot.counts` 一致；shard 计数互不干扰
  - [x] 4.3 `tests/driver_multishard.rs::multishard_shard_candidate_stats_clear_on_auto_evict`：
    - 开启 `shard_restart_on_panic = true`，让目标 shard panic，断言 supervisor auto-evict 后 `shard_candidate_stats()` 中目标 shard 的 counts 全为 0，其他 shard 不受影响

- [x] 5. module 层 Prometheus 指标：`webrtc.driver.local_candidate_*`
  - [x] 5.1 `metrics.rs` 给 `WebRtcModuleMetrics` 新增 8 个 `AtomicU64` 字段：`local_candidate_host / local_candidate_srflx / local_candidate_prflx / local_candidate_relay / local_candidate_udp / local_candidate_tcp / local_candidate_ipv4 / local_candidate_ipv6`；`Default::default()` 初始化为 0
  - [x] 5.2 `WebRtcModuleMetrics::record_local_candidate_snapshot(&self, counts: LocalCandidateCounts)` 公共方法：把 `counts.host / srflx / prflx / relay / udp / tcp / ipv4 / ipv6` 加到对应 counter（`Ordering::Relaxed`，monotonic — 每次 snapshot 累加，与 packet counter 模式一致）
  - [x] 5.3 `WebRtcModuleCounterSnapshot` / `WebRtcModuleMetricsSnapshot` 同步加 8 个字段；`assemble` / 文档头部的 metrics 列表同步更新（注释加上 §"Phase 02 follow-up 第十三轮：local candidate counters"）
  - [x] 5.4 `module.rs::run_driver_event_worker` 的 `LocalCandidateSnapshot` 分支：在既有 `tracing::debug!` 之前调用 `metrics.record_local_candidate_snapshot(counts)`；保留既有日志（log + metric 双写）
  - [x] 5.5 `metrics::tests` 新增：
    - `record_local_candidate_snapshot_accumulates_per_type`
    - `record_local_candidate_snapshot_is_monotonic`
    - `local_candidate_counters_default_to_zero`
    - `assemble_includes_local_candidate_fields`

- [x] 6. 测试与回归
  - [x] 6.1 `cargo fmt`
  - [x] 6.2 `cargo clippy -p cheetah-webrtc-driver-tokio`
  - [x] 6.3 `cargo test -p cheetah-webrtc-driver-tokio`
  - [x] 6.4 `cargo clippy -p cheetah-webrtc-module`
  - [x] 6.5 `cargo test -p cheetah-webrtc-module`

- [x] 7. 更新 `phase-02-driver-multithread-shard.md` 与 `index.md`
  - [x] 7.1 在 `phase-02-driver-multithread-shard.md` 顶部追加 "已完成（Phase 02 follow-up 第十三轮）" 段落，按 Section 1/2/3 列出 `RouteCandidateDiff`、`ShardCandidateTable` / `shard_candidate_stats()`、module Prometheus 指标三项落地内容、新增 API、测试列表
  - [x] 7.2 更新该文件的 "仍未落地（下一轮）" 段：移除已落地的三项，保留 / 重写 TCP writer 跨 shard 真实迁移转移条目作为下一轮唯一候选项（含真实压测评估前提）；加入新发现的候选项（如 `RouteCandidateDiff` 进 module event worker → 业务层 candidate churn 告警）
  - [x] 7.3 更新 `index.md`：状态行改为 "第十三轮已落地"；`计划文件清单` Phase 02 行末尾追加 "+ RouteCandidateDiff + shard_candidate_stats + local candidate Prometheus 指标"；状态行末尾的"下一轮聚焦"段同步重写为 TCP writer 物理迁移评估 + RouteCandidateDiff 业务层消费

## 验收标准

- `WebRtcDriverEvent::RouteUpdated(WebRtcRouteUpdate { diff, .. })` 在迁移成功路径上 `diff` 非空，`added / removed / stale` 字段语义一致
- `WebRtcDriverHandle::shard_candidate_stats()` 在 multi-shard 模式下按 shard id 顺序返回真实候选计数；auto-evict 后目标 shard 归零
- `WebRtcModuleMetricsSnapshot` 多出 8 个 `local_candidate_*` 字段，monotonic 计数器在每条 `LocalCandidateSnapshot` 进入后增长；既有 metrics 字段不破坏
- `cargo test -p cheetah-webrtc-driver-tokio` 与 `cargo test -p cheetah-webrtc-module` 全部通过
- `phase-02-driver-multithread-shard.md` / `index.md` 已同步状态与下一轮范围
