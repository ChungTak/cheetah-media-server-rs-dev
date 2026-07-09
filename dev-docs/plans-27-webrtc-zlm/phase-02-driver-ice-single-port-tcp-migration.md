# Phase 02 — Driver、ICE、单端口、TCP 与 连接迁移

- **状态**: 部分完成（Phase 02 第一+二+三轮：`listen_tcp` 真正绑定，RFC 4571 framing 解码 / 编码、TCP accept loop、TCP write registry、`TcpAccepted` / `TcpClosed` driver 事件、TCP framing 单元和集成测试矩阵已落地；TCP idle timeout 与 `WebRtcTcpCloseReason::IdleTimeout` 上线，半开连接到期后会被 driver 自动关闭；`handshake_timeout_ms` 真正接入 watchdog，未在窗口内连接成功的会话由 driver 主动关闭并发出 `WebRtcCloseReason::HandshakeTimeout`。多线程 shard、ICE full、完整 candidate policy、连接迁移完整状态机、backpressure 上界事件留作后续小步迭代）

## 实现概览

本阶段把 `cheetah-webrtc-driver-tokio` 从基础 UDP driver 升级为生产传输层：UDP/TCP 同端口、多线程分片、ICE full、candidate policy、连接迁移和背压。

## 已完成（Phase 02 第一轮）

- `crates/protocols/webrtc/driver-tokio/src/tcp.rs`：纯 Sans-I/O 的 RFC 4571 解码器 `Tcp4571Decoder` 与 `encode_frame`；上界来自 `tcp_frame_max_bytes`，命中时返回 `Tcp4571Error::FrameTooLarge` 而不 panic。
- `WebRtcDriverConfig` 新增 `tcp_read_chunk_size`、`tcp_frame_max_bytes`，与 UDP `read_buffer_size` 对齐；module 配置层默认值已同步。
- `spawn_driver` 当 `listen_tcp` 配置存在时绑定 `TcpListener`，并在 driver handle 上暴露 `local_tcp_addr()`。
- `tcp_accept_loop`：接受 TCP 连接、设置 `TCP_NODELAY`、向 module 发出 `WebRtcDriverEvent::TcpAccepted`，按连接分发到 `tcp_connection_loop`。
- `tcp_connection_loop`：使用 `Tcp4571Decoder` 流式解析 TCP bytes，把每个完整 frame 当作 `NetDatagram::Tcp` 投入与 UDP 共用的 `route_unbound_packet` 路径；EOF / I/O 错误 / framing 错误统一通过 `TcpClosed { reason }` 通知 module。
- `TcpWriterRegistry`：按 remote `SocketAddr` 索引 `OwnedWriteHalf`；`drain_core_outputs` 在 `SendPacket` 时优先选 TCP 通道，无则回落到 UDP `send_to`，写失败时同时移除 registry 条目避免悬挂引用。
- `WebRtcTcpCloseReason` 公共枚举区分 `PeerEof / Io / FramingError / Shutdown`，便于 module 与运维做诊断分类。
- `crates/protocols/webrtc/driver-tokio/tests/driver_tcp.rs` 集成测试覆盖：未配置 `listen_tcp` 时 `local_tcp_addr` 为 `None`、TCP accept 后 `TcpAccepted` 事件、有效 RFC 4571 帧落入 `route_unbound_packet` 并产出 `UnroutedPacket` 诊断、peer EOF 触发 `TcpClosed::PeerEof`、超长 frame 触发 `TcpClosed::FramingError`。

## 已完成（Phase 02 第二轮）

- `WebRtcDriverConfig::tcp_idle_timeout_ms`（默认 30 000ms，与 `session_idle_timeout_ms` 解耦）。`spawn_driver` 在为零时禁用 idle timeout；非零时把 `Duration` 注入 `tcp_accept_loop` 与 `tcp_connection_loop`。
- `tcp_connection_loop` 在 `tokio::select!` 里增加 `idle_sleep` 分支：每读一次 buf 重置一次 sleep，没读到字节但 sleep 触发时按 `WebRtcTcpCloseReason::IdleTimeout` 关闭连接，避免半开连接长期占用 driver task 资源。
- `WebRtcTcpCloseReason::IdleTimeout` 公开枚举变体，module 端 `TcpClosed` 事件已透传该原因。
- `WebRtcModuleConfig::tcp_idle_timeout_ms` wire 字段（默认 30 000ms）与 `to_driver_config` 透传。
- 集成测试：`driver_emits_tcp_closed_on_idle_timeout`（250ms 窗口下，未发送任何字节的连接被关闭，`reason = IdleTimeout`）；`driver_idle_timeout_disabled_when_zero`（`tcp_idle_timeout_ms = 0` 时 500ms 内不会触发 `IdleTimeout`）。

## 已完成（Phase 02 第三轮）

- `WebRtcDriverConfig::handshake_timeout_ms` 不再是占位，被注入到 `run_driver_core` 的 watchdog：每条 `AcceptOffer` / `CreateOffer` 命令落入 core 后，`pending_handshakes: HashMap<WebRtcSessionId, Instant>` 记录 `now + timeout`；`drain_core_outputs` 在看到 `WebRtcCoreEvent::Lifecycle::Connected` 或 `WebRtcCoreOutput::CloseSession` 时立刻清空对应条目。
- watchdog 每秒 sweep 一次（与现有 `Tick` 一起复用 `sleep_until` 唤醒，不增加额外定时器），到期 session 经由 `WebRtcCoreCommand::Close { reason: WebRtcCloseReason::HandshakeTimeout }` 关闭，并同步发出 `Diagnostic { kind: Lifecycle, message: "session ... handshake timed out" }`，下游 module 通过 `SessionClosed` 事件感知。
- `handshake_timeout_ms = 0` 显式禁用 watchdog（`pending_handshakes` 仍按需填充但永不触发），保持向后兼容。
- 集成测试：`driver_handshake_timeout_closes_stuck_session`（500ms 配置 + 4s 等待窗口，断言 `SessionClosed::HandshakeTimeout` 必出现）；`driver_handshake_timeout_disabled_when_zero`（`= 0` 时 1.5s 内不触发）。

## 已完成（Phase 02 第四轮）

- `RouteTable::compact_expired` 方法：返回过期的 stale route 条目列表（`Vec<(SocketAddr, WebRtcSessionId)>`），供 driver 发出诊断。
- `WebRtcDriverDiagnosticKind::RouteExpired` 新枚举变体：标识 stale route 过期事件。
- driver core loop 在每次迭代末尾调用 `routes.compact_expired(now)`，对每个过期条目发出 `RouteExpired` 诊断，让 module / 运维侧可观测连接迁移生命周期完成。
- 单元测试 2 条：`compact_expired_returns_expired_entries`（验证过期条目被返回并清除）、`compact_expired_does_not_return_fresh_entries`（验证未过期条目不被返回）。

## 已完成（Phase 02 第五轮）

- TCP keepalive 探针：在 `tcp_accept_loop` 中对每个新接受的 TCP 连接设置 `SO_KEEPALIVE`（idle=30s, interval=10s, count=3），通过 `libc::setsockopt` 在 Unix 平台上启用 OS 级 TCP keepalive，补充应用层 idle timeout 对长连接 NAT 的可靠性。
- `libc` 依赖添加为 `[target.'cfg(unix)'.dependencies]`，仅在 Unix 平台编译。

## 已完成（Phase 02 第六轮）

- `RouteTable::try_bind_migration` 方法：硬容量 cap（`hard_cap = soft_cap * 4`）的迁移绑定，超过 cap 时返回 `Err(())`，让调用方知道迁移被拒绝。
- `WebRtcDriverDiagnosticKind::MigrationRejected` 新枚举变体：标识 route table 容量不足导致的迁移拒绝。
- `handle_datagram` 集成：检测到迁移（`previous_addr != source` 且 `previous_addr.is_some()`）时使用 `try_bind_migration`，被拒绝时发出 `MigrationRejected` 诊断并丢弃包；非迁移路径继续使用普通 `bind`。
- 单元测试 3 条：`try_bind_migration_succeeds_when_below_hard_cap`、`try_bind_migration_rejects_when_at_hard_cap`、`try_bind_migration_allows_reaffirming_existing_binding`（验证幂等）。

## 已完成（Phase 02 第七轮）

- `WebRtcDriverEvent::Backpressure { queue, pending }` 新事件变体：标识驱动层队列接近容量，`queue` 字段区分 events 通道和 packets 通道。
- driver core loop 中加入 backpressure 监控：每次迭代末尾检查 `event_tx.capacity()` / `event_tx.max_capacity()`，剩余容量低于 25% 时发出 `Backpressure` 事件。
- module event worker 处理 `Backpressure` 事件，写入 `warn!` 日志便于运维观测。

## 已完成（Phase 02 第八轮）

- `WebRtcIceTransportPolicy` 公共枚举：核心层新增 `All` / `RelayOnly` / `P2pOnly` 三档候选过滤策略，对齐 W3C `RTCIceTransportPolicy` 语义。
- `WebRtcCoreConfig::ice_transport_policy` 字段：核心层存储策略，driver 层负责实际过滤（占位实现，等待后续 candidate gathering 接入）。
- `WebRtcModuleConfig::ice_transport_policy` 字段（默认 `"all"`）：模块配置 wire 字段，支持 `all` / `relay-only` / `relay_only` / `relayonly` / `p2p-only` / `p2p_only` / `p2ponly` 七种写法（大小写不敏感）。
- `parse_ice_transport_policy` 函数：解析字符串到枚举，未知值返回明确错误信息。
- `validate` 拒绝非法 `ice_transport_policy` 并在 `to_driver_config` 路径中透传到 core。
- 单元测试 5 条：解析全部已知值、拒绝未知值、validate 路径错误传播、driver_config 透传。

## 后续小步迭代

- 多线程 shard：把现有单 driver task 拆分为 `N` 个 shard（`driver_shards`），新 session 按 hash/负载选择 shard，UDP listener 作为前端只负责路由分发。
- candidate gathering：在 driver 层根据 `ice_transport_policy` 过滤本地候选，并显式发布 UDP host candidate 与 TCP passive host candidate。
- TURN credential 注入：当前 `RelayOnly` 模式仅落地策略枚举，TURN server 配置仍是占位。
- packets 通道 backpressure：当前仅监控 events 通道，packets 通道（UDP send queue）满时的丢帧策略尚未实现。


## 2.1 UDP/TCP 同端口

目标：

- `listen_udp` 绑定 UDP socket。
- `listen_tcp` 绑定 TCP listener，默认可与 UDP 使用相同端口。
- candidate gathering 同时发布 UDP host candidate 和 TCP passive host candidate。
- TCP session 使用 WebRTC TCP framing，把 stream bytes 拆成 core network packet。

实现原则：

- TCP framing 属于 driver，不进入 core。
- driver 输出统一 `WebRtcNetworkInput` 给 core。
- TCP 半包、粘包、超长 frame、连接半开都要有限长和超时。
- `enable_tcp=false` 时不发布 TCP candidate，也不绑定 TCP listener。

## 2.2 多线程 shard

新增 shard 模型：

- driver 启动 `N` 个 shard task，`driver_shards=0` 时按 CPU 数或 runtime worker 数选择。
- session 创建时按 session id hash 或负载选择 shard。
- 每个 shard 持有自己的 `WebRtcCore`、route table、timer queue、outbound queue。
- UDP listener 只负责收包、初步路由和投递；不直接驱动所有 session。

unbound packet 处理：

- 先按 remote addr 查 active route。
- 未命中时解析 STUN ufrag，通过 core/session registry 找 shard。
- 仍未命中则丢弃并记录 `UnroutedPacket` diagnostic。

## 2.3 ICE full 与 candidate policy

配置：

- `ice_lite=false` 为默认。
- `ice_transport_policy=all|relay-only|p2p-only`。
- `stun_servers`、`turn_servers`、`extern_ips`、`interfaces`。

边界：

- 本项目不实现 TURN server。
- TURN relay candidate 只来自配置的远端 TURN 服务。
- host candidate 可以按 interface allowlist 生成。
- server reflexive candidate 可由 `str0m` 与配置的 STUN server 生成；driver 负责 I/O。

## 2.4 连接迁移

连接迁移状态：

- `ActiveRoute(session_id, remote_addr)`
- `StaleRoute(session_id, old_remote_addr, expires_at)`
- `ExpiredRoute`

迁移触发：

- core 判断已知 session 接受来自新 remote addr 的 packet。
- driver 在 route table 原子更新 active route。
- 旧 route 进入 stale set，TTL 默认 30s。
- stale route 只允许投递给原 session，不允许绑定新 session。

诊断：

- `RouteUpdated { previous_addr, new_addr, reason }`
- `StaleRouteHit`
- `RouteExpired`
- `MigrationRejected`

## 2.5 Backpressure 与资源上界

必须有上界：

- command queue
- event queue
- outbound packet queue
- per TCP connection read buffer
- per session remote candidate count
- route table active/stale entry count
- pending timer count

策略：

- media outbound queue 满时优先丢弃 delta frame 或低优先级 RTP，保留 RTCP、ICE、DTLS、keyframe。
- control queue 满时返回 `Busy` diagnostic。
- TCP 写超时关闭会话。
- UDP send error 计数超过阈值关闭或降级。

## 2.6 Driver public API

`WebRtcDriverConfig` 扩展：

```text
listen_udp: SocketAddr
listen_tcp: Option<SocketAddr>
enable_tcp: bool
driver_shards: usize
udp_recv_buffer_bytes: usize
tcp_read_buffer_bytes: usize
tcp_frame_max_bytes: usize
handshake_timeout_ms: u64
migration_route_ttl_ms: u64
max_sessions: usize
max_routes: usize
```

`WebRtcDriverEvent` 扩展：

- `RouteUpdated`
- `TcpAccepted`
- `TcpClosed`
- `Backpressure`
- `UnroutedPacket`
- `ShardStats`

## 2.7 测试要求

运行：

```powershell
cargo test -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-core
```

新增测试：

- TCP framing 半包/粘包/超长包。
- UDP route active/stale/expired。
- 同 session remote addr 迁移。
- stale route 不误绑定新 session。
- shard 选择稳定，StopSession 只关闭所属 shard。
- `enable_tcp=false` 时不绑定 TCP、不生成 TCP candidate。
- backpressure 满队列返回 diagnostic。

## 完成后检查

- `listen_tcp` 不再是配置占位。
- driver 不持有 PublishLease、Subscriber、StreamKey 等业务状态。
- module 公共接口仍不暴露 Tokio 类型。

