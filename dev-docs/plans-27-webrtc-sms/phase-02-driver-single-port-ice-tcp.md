# Phase 02 — Driver 单端口、ICE、TCP 与连接迁移

- **状态**: 已完成（基础设施落地）
- **完成位置**: `crates/protocols/webrtc/driver-tokio/`
- **范围**: 实现 `cheetah-webrtc-driver-tokio`，负责 UDP/TCP I/O、单端口多会话路由、多线程 shard、timer 驱动、candidate 注入、bounded queue、连接迁移
- **完成标准**: 同一 UDP/TCP 端口可承载多个 WebRTC session；driver 能驱动 core 完成 ICE/DTLS/RTP/RTCP 数据交换；route migration 有可测试行为
- **落地清单**:
  - `spawn_driver` 异步入口绑定 UDP socket（支持 `0.0.0.0:0` 由 OS 分配端口）、暴露 `local_udp_addr()`/`session_count()`/`send_command()`/`recv_event()` 接口。
  - 单端口路由表 `RouteTable` 通过 remote `SocketAddr` 命中已绑定 session；新地址首次到达的 STUN 报文调用 `WebRtcCore::route_unbound_packet` 经 `Rtc::accepts` 完成 ICE ufrag/credentials 反查并自动绑定。
  - 路由抖动/迁移：地址变更触发 `WebRtcRouteUpdate` 事件，旧地址进入 stale TTL 表（`migration_route_ttl_ms`）。session 关闭后立即清理所有路由。
  - 驱动事件类型：`AnswerReady` / `Core(WebRtcCoreEvent)` / `SessionClosed` / `RouteUpdated` / `Diagnostic`，全部经 bounded MPSC 通道传递。
  - 计时器：`SetTimer` 输出转换为 `tokio::time::sleep_until`；冷路径 1 小时虚拟唤醒确保 session 不饿死。
  - 集成测试：`tests/driver_smoke.rs` 验证 UDP bind、accept_offer 产生 AnswerReady、垃圾 SDP 触发 Diagnostic、未路由的 UDP 包不会 panic 且记录 `UnroutedPacket` 诊断、`StopSession` 后 `session_count` 复位为 0、saturated 命令队列返回 `WebRtcSendError::QueueFull` 而非阻塞、`stats_snapshot()` 报告 `commands_accepted_total` / `events_emitted_total` / `unrouted_packets_total` 计数器与绑定地址；`tests/route.rs` 单元覆盖 bind/lookup/rebind/forget/stale TTL/migration race 六种行为；`tests/driver_tcp.rs` 覆盖 TCP 监听、RFC 4571 framing、idle/handshake timeout。
  - **`WebRtcDriverHandle::stats_snapshot()`** 暴露轻量 `WebRtcDriverStats { local_udp_addr, local_tcp_addr, session_count, commands_accepted_total, events_emitted_total, unrouted_packets_total }`，所有计数器为 monotonic AtomicU64，`Ordering::Relaxed` 加载，避免锁。Dashboard 通过两次快照差值得到速率。
  - 仍属于后续小步迭代：完整的 STUN reflexive / TURN candidate 自动发现，跨 shard 工作分发（当前仍是单 worker，`shard_count` 字段为占位）。本期把 TCP listener 留为可启用配置项但默认关闭，shard_count 字段保留以备未来切分。

## 2.1 Driver 文件结构

```text
crates/protocols/webrtc/driver-tokio/src/
  lib.rs
  config.rs
  command.rs
  event.rs
  driver.rs
  udp.rs
  tcp.rs
  router.rs
  shard.rs
  timer.rs
  candidate.rs
  migration.rs
  metrics.rs
```

职责：

- `udp.rs`: UDP bind、recv_from、send_to。
- `tcp.rs`: TCP listener、connection reader/writer、WebRTC over TCP framing。
- `router.rs`: 包分类和 session route table。
- `shard.rs`: session worker 分片。
- `timer.rs`: core timer 输出转 runtime sleep。
- `candidate.rs`: local candidate 构造和配置注入。
- `migration.rs`: remote address 更新策略。
- `driver.rs`: 主循环和 command/event glue。

## 2.2 Driver public API

```rust
pub struct WebRtcDriverConfig {
    pub listen_udp: SocketAddr,
    pub listen_tcp: Option<SocketAddr>,
    pub enable_udp: bool,
    pub enable_tcp: bool,
    pub public_ips: Vec<IpAddr>,
    pub candidate_hostname: Option<String>,
    pub shard_count: usize,
    pub max_sessions: usize,
    pub read_buffer_size: usize,
    pub write_queue_capacity: usize,
    pub event_queue_capacity: usize,
    pub route_ttl_ms: u64,
    pub migration_route_ttl_ms: u64,
    pub handshake_timeout_ms: u64,
    pub session_idle_timeout_ms: u64,
}

pub enum WebRtcDriverCommand {
    CreateSession(WebRtcSessionSpec),
    ApplyRemoteDescription { session_id: WebRtcSessionId, sdp: String },
    AddRemoteCandidate { session_id: WebRtcSessionId, candidate: String },
    SendFrame(Box<WebRtcSendFrame>),
    SendRtp(Box<WebRtcSendRtp>),
    SendDataChannel(WebRtcDataMessage),
    RequestKeyframe { session_id: WebRtcSessionId, track_id: TrackId },
    StopSession(WebRtcSessionId),
}

pub enum WebRtcDriverEvent {
    Core(WebRtcCoreEvent),
    RouteUpdated(WebRtcRouteUpdate),
    Diagnostic(WebRtcDriverDiagnostic),
}
```

`WebRtcDriverHandle`：

- `send_command(&self, cmd) -> async`
- `recv_event(&self) -> async Option<WebRtcDriverEvent>`
- `session_count(&self) -> usize`
- `stats_snapshot(&self) -> WebRtcDriverStats`

## 2.3 单端口 UDP 路由

包分类：

```text
0x00..0x03 with STUN magic cookie -> STUN
0x14..0x40 -> DTLS
RTP/RTCP payload type range -> RTP/RTCP
else -> unknown diagnostic
```

路由优先级：

1. 已绑定 remote `SocketAddr` -> session。
2. STUN username 中的 local ufrag -> pending session。
3. RTP/RTCP SSRC -> active media session。
4. unknown -> drop + diagnostic。

route table：

```rust
struct RouteTable {
    by_addr: HashMap<SocketAddr, RouteEntry>,
    by_local_ufrag: HashMap<String, WebRtcSessionId>,
    by_ssrc: HashMap<u32, WebRtcSessionId>,
    stale_addr: HashMap<SocketAddr, StaleRouteEntry>,
}
```

规则：

- pending session 创建后注册 local ufrag。
- ICE connected 后绑定 remote addr。
- RTP media event 出现后注册 SSRC。
- session close 后删除所有 route。
- route table 必须 bounded，超过 `max_sessions * route_factor` 后拒绝新 session 或清理 stale route。

## 2.4 多线程 shard

目标：

- 单端口 socket recv task 只做轻量分类和 route。
- 每个 session 固定归属一个 shard worker。
- worker 内部持有 `WebRtcCore` 或 session map，避免跨 session 全局锁。

建议结构：

```text
UDP recv task
  -> classify
  -> router lookup
  -> shard_tx[hash(session_id)]
  -> shard worker
  -> core.handle_input
  -> outputs
  -> socket writer queue / event queue / timer wheel
```

配置：

- `shard_count=0` 表示使用 runtime worker 数或 CPU 数。
- `shard_count=1` 用于测试可重复性。
- 每个 shard command queue 有 capacity，满时优先丢弃低优先级 RTP，不能阻塞 UDP recv。

## 2.5 TCP WebRTC

首版支持：

- TCP passive server candidate。
- 一个 TCP listener 绑定 `listen_tcp`。
- 每条 TCP connection 建立独立 read/write queue。
- 初始 route 可按 STUN username 绑定 session。

WebRTC over TCP 注意点：

- TCP 上仍可能承载 STUN/DTLS/RTP/RTCP，需要同 UDP 一样分类。
- TCP 连接是 stream，需要 framing。实现要以 `str0m` 实际 TCP candidate 数据格式为准；如果 `str0m` 要求外部提供完整 packet boundary，driver 负责从 TCP stream 中恢复 packet。
- 首版如果无法可靠恢复通用 TCP framing，则只开启经实测确认的 passive TCP 模式，并在配置中默认关闭 TCP。

测试：

- TCP listener accept 后可以把 STUN/DTLS packet 路由到指定 session。
- TCP write queue 满时关闭 connection 或丢弃低优先级输出。
- TCP connection close 清理 route，但不立即关闭 session，允许 ICE fallback 或 migration。

## 2.6 Candidate gathering

`str0m` 不负责 network interface enum，因此 candidate 来源由 driver/module 配置提供。

候选来源：

- `listen_udp` host candidate。
- `listen_tcp` TCP host candidate。
- `public_ips` 显式公网地址。
- `candidate_hostname` 可选 hostname candidate。
- STUN reflexive candidate：Phase 02 可预留，Phase 05 再实现完整 STUN gather。
- TURN relay candidate：V1 不实现 TURN server，只支持配置远端 TURN 服务后按 `str0m` 能力接入。

策略：

- 不自动枚举所有网卡，避免泄漏内网和不可控行为。
- 默认只发布 listen addr 和 configured public addr。
- candidate policy 在 config 中显式表达。

## 2.7 连接迁移

连接迁移定义：

- 同一 WebRTC session 在 ICE username/ufrag 不变或 ICE restart 可验证的情况下，remote address 从 A 变为 B。
- driver 将 session route 从 A 更新到 B，并保留 A 为 stale route 一段 TTL。

迁移触发：

1. STUN binding request 使用已知 local ufrag，并来自新 addr。
2. `str0m` 接受该网络输入并继续产生有效输出。
3. driver 更新 route，并发 `RouteUpdated`。

安全约束：

- 未知 ufrag 不迁移。
- 同一 addr 同时映射多个 session 时拒绝迁移并上报 conflict。
- RTP-only addr 变化不单独触发迁移，除非已有 STUN 验证。

测试：

- old addr -> connected。
- new addr 发送 STUN binding request。
- route 切换到 new addr。
- old addr 在 TTL 内仍能被识别为 stale。
- TTL 后 old addr 被清理。

## 2.8 Timer 与 backpressure

timer：

- core 输出 `SetTimer` 后 driver 建 per-session timer。
- 新 timer 覆盖旧 timer。
- session close 后取消 timer。
- timer firing 时发送 `WebRtcCoreInput::Timeout`。

backpressure：

- UDP send queue bounded。
- TCP per-connection write queue bounded。
- driver event queue bounded。
- shard input queue bounded。
- RTP/media packet queue 满时允许丢弃低优先级包；STUN/DTLS/RTCP 控制包优先级更高。

建议优先级：

1. Session close / stop command。
2. STUN / DTLS / ICE control。
3. RTCP feedback。
4. DataChannel control。
5. Keyframe / IDR RTP。
6. 普通 video RTP。
7. 可丢弃 temporal layer RTP。

## 2.9 Phase 02 测试要求

命令：

```text
cargo fmt
cargo clippy -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-core
cargo clippy -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-driver-tokio
```

集成测试：

- `driver_udp_single_port_routes_two_sessions_by_ufrag`
- `driver_udp_routes_rtp_by_bound_addr_after_ice`
- `driver_rejects_unknown_packet_without_panic`
- `driver_tcp_passive_accepts_session_packet`
- `driver_route_migration_updates_remote_addr_after_stun`
- `driver_stale_route_expires_after_ttl`
- `driver_queue_full_does_not_block_recv_loop`
- `driver_session_close_cleans_routes_and_timers`
- `driver_shards_keep_session_affinity`

## 2.10 Phase 02 验收标准

- driver 公共接口不暴露 `tokio::*` 类型。
- UDP 单端口可同时驱动多个 session。
- TCP passive 路径有明确开关和测试。
- timer、route、queue 都有上界。
- migration 行为可测，且不会凭 RTP-only 包劫持 session。
- module 尚未接入时，driver/core integration test 可完成基本 packet flow。

