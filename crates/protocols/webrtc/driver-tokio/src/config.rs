//! Driver configuration consumed by [`crate::spawn_driver`].
//!
//! [`crate::spawn_driver`] 使用的 driver 配置。

use std::net::{IpAddr, SocketAddr};

use cheetah_webrtc_core::WebRtcCoreConfig;

/// Optional UDP port range for the driver listener. When configured,
/// the driver binds within `[min, max]` instead of using the port
/// from `listen_udp`. On bind failure the driver tries the next port
/// in the range. Released ports become available for reuse on
/// subsequent driver restarts.
///
/// driver 侦听器的可选 UDP 端口范围。
/// 配置后，driver 绑定在 `[min, max]` 内，而不是使用 `listen_udp` 中的端口。
/// 绑定失败时，driver 会尝试范围内的下一个端口。
/// 已释放的端口可在后续 driver 重新启动时重用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpPortRange {
    /// Minimum UDP port (inclusive).
    ///
    /// 最小 UDP 端口（含）。
    pub min: u16,
    /// Maximum UDP port (inclusive).
    ///
    /// 最大 UDP 端口（含）。
    pub max: u16,
}

impl UdpPortRange {
    /// Validate the port range. Returns an error string if the range
    /// is invalid (min == 0, max == 0, or min > max).
    ///
    /// 验证端口范围。
    /// 如果范围无效（min == 0、max == 0 或 min > max），则返回错误字符串。
    pub fn validate(&self) -> Result<(), String> {
        if self.min == 0 {
            return Err("udp_port_min must be > 0".into());
        }
        if self.max == 0 {
            return Err("udp_port_max must be > 0".into());
        }
        if self.min > self.max {
            return Err(format!(
                "udp_port_min ({}) must be <= udp_port_max ({})",
                self.min, self.max
            ));
        }
        Ok(())
    }

    /// Number of ports in the range (inclusive).
    ///
    /// 范围内的端口数量（含）。
    pub fn len(&self) -> u32 {
        u32::from(self.max) - u32::from(self.min) + 1
    }

    /// Returns `true` if the range contains zero ports (impossible
    /// after validation, but required by clippy for `len` parity).
    ///
    /// 如果范围包含零端口，则返回 `true` （验证后不可能，但 Clippy 需要 `len` 奇偶校验）。
    pub fn is_empty(&self) -> bool {
        // After validation min <= max, so this is always false.
        // Kept for API completeness.
        self.min > self.max
    }
}
/// Runtime configuration consumed by spawn_driver.
/// Groups socket binding, queue sizing, shard topology,
/// supervisor retry policy, and the inner core settings.
///
/// spawn_driver 使用的运行时配置。
/// 组套接字绑定、队列大小、shard 拓扑、主管重试策略和内核设置。
#[derive(Debug, Clone)]
pub struct WebRtcDriverConfig {
    /// UDP listen address. Required.
    ///
    /// UDP 监听地址。
    /// 必需的。
    pub listen_udp: SocketAddr,
    /// Optional UDP port range. When set, the driver ignores the port
    /// component of `listen_udp` and instead tries to bind within
    /// `[udp_port_range.min, udp_port_range.max]`. The IP from
    /// `listen_udp` is still used as the bind address.
    ///
    /// 可选 UDP 端口范围。
    /// 设置后，driver 会忽略 `listen_udp` 的端口组件，而是尝试在 `[udp_port_range.min, udp_port_range.max]` 内绑定。
    /// `listen_udp` 中的 IP 仍用作绑定地址。
    pub udp_port_range: Option<UdpPortRange>,
    /// Optional TCP listen address for WebRTC over TCP candidates.
    ///
    /// WebRTC-over-TCP candidates 的可选 TCP 监听地址。
    pub listen_tcp: Option<SocketAddr>,
    /// Public IPs that should be advertised as host candidates in addition
    /// to whatever the listener bound to. Populated by the module from
    /// configuration.
    ///
    /// 除了侦听器绑定的任何内容之外，还应将其广告为主机 candidates 的公共 IP。
    /// 由配置中的模块填充。
    pub public_ips: Vec<IpAddr>,
    /// Optional hostname candidate to advertise.
    ///
    /// 用于通告的可选主机名 candidate。
    pub candidate_hostname: Option<String>,
    /// Hard cap on the number of concurrent sessions managed by the
    /// driver.
    ///
    /// driver 管理的并发会话数的硬性上限。
    pub max_sessions: usize,
    /// Bytes of UDP recv buffer reserved per receive call.
    ///
    /// 每个接收调用保留的 UDP recv 缓冲区字节数。
    pub read_buffer_size: usize,
    /// Bytes reserved per TCP read call. Each TCP connection keeps a
    /// streaming RFC 4571 decoder; this controls how much we read at a
    /// time off the kernel buffer.
    ///
    /// 每个 TCP 读取调用保留的字节数。
    /// 每个 TCP 连接都保留一个流式 RFC 4571 解码器；
    /// 这控制我们一次从内核缓冲区读取多少内容。
    pub tcp_read_chunk_size: usize,
    /// Maximum RFC 4571 frame size (including length prefix overhead).
    /// Frames exceeding this cap close the offending connection.
    ///
    /// 最大 RFC 4571 帧大小（包括长度前缀开销）。
    /// 超过此上限的帧会关闭有问题的连接。
    pub tcp_frame_max_bytes: usize,
    /// Idle timeout (ms) for an accepted TCP connection. When the
    /// remote peer sends no bytes for this long the driver closes the
    /// connection. `0` disables the timeout (legacy behaviour).
    ///
    /// 接受的 TCP 连接的空闲超时（毫秒）。
    /// 当远程对等方在这么长的时间内没有发送任何字节时，driver 将关闭连接。
    /// `0` 禁用超时（旧行为）。
    pub tcp_idle_timeout_ms: u64,
    /// Bound on the per-session outbound packet queue.
    ///
    /// 绑定在每个会话的出站数据包队列上。
    pub write_queue_capacity: usize,
    /// Bound on the driver-wide event queue.
    ///
    /// 绑定在 driver 范围的事件队列上。
    pub event_queue_capacity: usize,
    /// Bound on the driver-wide command queue.
    ///
    /// 绑定在 driver 范围的命令队列上。
    pub command_queue_capacity: usize,
    /// Idle timeout (ms) after which sessions with no inbound traffic are
    /// closed.
    ///
    /// 空闲超时（毫秒），之后没有入站流量的会话将被关闭。
    pub session_idle_timeout_ms: u64,
    /// Handshake timeout (ms) for ICE/DTLS to come up.
    ///
    /// ICE/DTLS 出现的握手超时（毫秒）。
    pub handshake_timeout_ms: u64,
    /// Stale route TTL (ms) used during connection migration.
    ///
    /// 连接迁移期间使用的过时路由 TTL (ms)。
    pub migration_route_ttl_ms: u64,
    /// Number of session-owner shards to spawn. `0` selects the
    /// runtime default (`available_parallelism()`, minimum 1). The
    /// current driver uses one shard for protocol state but exposes
    /// the value through `WebRtcDriverHandle::shard_count` so the
    /// public API stays stable when the multi-shard front-end lands.
    ///
    /// 要生成的会话所有者 shards 的数量。
    /// `0` 选择运行时默认值（`available_parallelism()`，最小值 1）。
    /// 当前的 driver 使用一个 shard 来表示协议状态，但通过 `WebRtcDriverHandle::shard_count` 公开该值，因此当多 shard 前端登陆时
    /// ，公共 API 保持稳定。
    pub driver_shards: usize,
    /// Per-shard command channel capacity. Reserved for the
    /// multi-shard front-end; falls back to `command_queue_capacity`
    /// when the front-end is single-shard.
    ///
    /// 每 shard 命令通道容量。
    /// 为多 shard 前端保留；
    /// 当前端是 single-shard 时，回退到 `command_queue_capacity`。
    pub shard_command_capacity: usize,
    /// Hard cap on the number of `(remote address, shard)` bindings
    /// that the global route directory will hold at once. Above the
    /// cap the front-end surfaces a `RouteDirectoryFull` diagnostic
    /// and drops the offending datagram.
    ///
    /// 全局路由目录一次保存的 `(remote address, shard)` 绑定数量的硬性上限。
    /// 在上限之上，前端会显示 `RouteDirectoryFull` 诊断并丢弃有问题的数据报。
    pub route_directory_capacity: usize,
    /// Hard cap on the number of stale (post-migration) bindings the
    /// route directory keeps around.
    ///
    /// 对路由目录保留的陈旧（迁移后）绑定数量的硬性上限。
    pub route_directory_stale_capacity: usize,
    /// Whether the supervisor should automatically respawn a shard
    /// task that exited with a panic. When `true` (default `false`),
    /// the supervisor calls `evict_shard` to clear orphaned
    /// directory entries and then re-spawns the shard loop with
    /// fresh state. The session count budget allowed per shard
    /// across the driver lifetime is `shard_max_restart_count`; once
    /// exceeded the supervisor stops emitting fresh tasks for that
    /// shard and only surfaces `ShardStopped { reason }` events.
    ///
    /// 主管是否应自动重新生成因恐慌而退出的 shard 任务。
    /// 当 `true` （默认 `false`）时，主管调用 `evict_shard` 来清除孤立的目录条目，然后以新状态重新生成 shard 循环。
    /// 在 driver 生命周期内每个 shard 允许的会话计数预算为 `shard_max_restart_count`；
    /// 一旦超过，主管将停止为该 shard 发出新任务，并且仅表面 `ShardStopped { reason }` 事件。
    pub shard_restart_on_panic: bool,
    /// Maximum number of times each individual shard may be
    /// auto-restarted by the supervisor over the driver's lifetime.
    /// Defaults to 3 — the goal is to recover from transient
    /// panics while still surfacing systemic failures (a crash loop
    /// indicates deterministic state corruption that retries will
    /// not fix). Ignored when `shard_restart_on_panic` is false.
    ///
    /// 在 driver 的生命​​周期内，主管可以自动重新启动每个单独的 shard 的最大次数。
    /// 默认为 3 — 目标是从短暂的恐慌中恢复，同时仍然面临系统故障（崩溃循环表明重试无法修复的确定性状态损坏）。
    /// 当 `shard_restart_on_panic` 为 false 时被忽略。
    pub shard_max_restart_count: u32,
    /// Initial backoff (ms) for the shard restart loop. The
    /// supervisor sleeps for this long before respawning the first
    /// time and doubles the wait on each subsequent restart up to
    /// `shard_max_restart_backoff_ms`.
    ///
    /// shard 重新启动循环的初始退避（毫秒）。
    /// 主管在第一次重生之前会休眠这么长时间，并在每次后续重新启动时将等待时间加倍，直至 `shard_max_restart_backoff_ms`。
    pub shard_restart_backoff_ms: u64,
    /// Upper bound (ms) on the exponential backoff between shard
    /// restarts. Prevents unbounded growth when a shard panics
    /// repeatedly.
    ///
    /// shard 重新启动之间指数退避的上限（毫秒）。
    /// 当 shard 反复出现恐慌时，防止无限增长。
    pub shard_max_restart_backoff_ms: u64,
    /// Inner core configuration applied to every session.
    ///
    /// 内核配置应用于每个会话。
    pub core: WebRtcCoreConfig,
}

impl Default for WebRtcDriverConfig {
    fn default() -> Self {
        Self {
            listen_udp: SocketAddr::from(([0, 0, 0, 0], 8000)),
            udp_port_range: None,
            listen_tcp: None,
            public_ips: Vec::new(),
            candidate_hostname: None,
            max_sessions: 4096,
            read_buffer_size: 65_536,
            tcp_read_chunk_size: 16_384,
            tcp_frame_max_bytes: 65_535,
            tcp_idle_timeout_ms: 30_000,
            write_queue_capacity: 512,
            event_queue_capacity: 1024,
            command_queue_capacity: 256,
            session_idle_timeout_ms: 30_000,
            handshake_timeout_ms: 10_000,
            migration_route_ttl_ms: 30_000,
            driver_shards: 0,
            shard_command_capacity: 256,
            route_directory_capacity: 16_384,
            route_directory_stale_capacity: 4_096,
            shard_restart_on_panic: false,
            shard_max_restart_count: 3,
            shard_restart_backoff_ms: 250,
            shard_max_restart_backoff_ms: 30_000,
            core: WebRtcCoreConfig::default(),
        }
    }
}

impl WebRtcDriverConfig {
    /// Resolve the effective shard count.
    ///
    /// `0` means "auto"; we pick `available_parallelism()` and clamp
    /// to a minimum of 1. The result is stable for the lifetime of a
    /// driver task — callers should cache it rather than recomputing.
    ///
    /// 解析有效 shard 计数。
    ///
    /// `0` 表示“自动”；
    /// 我们选择 `available_parallelism()` 并将其限制为最小值 1。
    /// 结果在 driver 任务的生命周期内是稳定的——调用者应该缓存它而不是重新计算。
    pub fn effective_shard_count(&self) -> usize {
        if self.driver_shards == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .max(1)
        } else {
            self.driver_shards
        }
    }

    /// Validate the driver configuration. Returns an error string if
    /// any field is invalid. Called by `spawn_driver` before binding.
    ///
    /// 验证 driver 配置。
    /// 如果任何字段无效，则返回错误字符串。
    /// 在绑定之前由 `spawn_driver` 调用。
    pub fn validate(&self) -> Result<(), String> {
        if let Some(range) = &self.udp_port_range {
            range.validate()?;
        }
        Ok(())
    }
}
