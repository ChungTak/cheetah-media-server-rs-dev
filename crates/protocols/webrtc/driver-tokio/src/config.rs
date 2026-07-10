//! Driver configuration consumed by [`crate::spawn_driver`].

use std::net::{IpAddr, SocketAddr};

use cheetah_webrtc_core::WebRtcCoreConfig;

/// Optional UDP port range for the driver listener. When configured,
/// the driver binds within `[min, max]` instead of using the port
/// from `listen_udp`. On bind failure the driver tries the next port
/// in the range. Released ports become available for reuse on
/// subsequent driver restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpPortRange {
    /// `min` field of type `u16`.
    /// `min` 字段，类型为 `u16`.
    pub min: u16,
    /// `max` field of type `u16`.
    /// `max` 字段，类型为 `u16`.
    pub max: u16,
}

impl UdpPortRange {
    /// Validate the port range. Returns an error string if the range
    /// is invalid (min == 0, max == 0, or min > max).
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
    pub fn len(&self) -> u32 {
        u32::from(self.max) - u32::from(self.min) + 1
    }

    /// Returns `true` if the range contains zero ports (impossible
    /// after validation, but required by clippy for `len` parity).
    pub fn is_empty(&self) -> bool {
        // After validation min <= max, so this is always false.
        // Kept for API completeness.
        self.min > self.max
    }
}

/// `WebRtcDriverConfig` data structure.
/// `WebRtcDriverConfig` 数据结构.
#[derive(Debug, Clone)]
pub struct WebRtcDriverConfig {
    /// UDP listen address. Required.
    pub listen_udp: SocketAddr,
    /// Optional UDP port range. When set, the driver ignores the port
    /// component of `listen_udp` and instead tries to bind within
    /// `[udp_port_range.min, udp_port_range.max]`. The IP from
    /// `listen_udp` is still used as the bind address.
    pub udp_port_range: Option<UdpPortRange>,
    /// Optional TCP listen address for WebRTC over TCP candidates.
    pub listen_tcp: Option<SocketAddr>,
    /// Public IPs that should be advertised as host candidates in addition
    /// to whatever the listener bound to. Populated by the module from
    /// configuration.
    pub public_ips: Vec<IpAddr>,
    /// Optional hostname candidate to advertise.
    pub candidate_hostname: Option<String>,
    /// Hard cap on the number of concurrent sessions managed by the
    /// driver.
    pub max_sessions: usize,
    /// Bytes of UDP recv buffer reserved per receive call.
    pub read_buffer_size: usize,
    /// Bytes reserved per TCP read call. Each TCP connection keeps a
    /// streaming RFC 4571 decoder; this controls how much we read at a
    /// time off the kernel buffer.
    pub tcp_read_chunk_size: usize,
    /// Maximum RFC 4571 frame size (including length prefix overhead).
    /// Frames exceeding this cap close the offending connection.
    pub tcp_frame_max_bytes: usize,
    /// Idle timeout (ms) for an accepted TCP connection. When the
    /// remote peer sends no bytes for this long the driver closes the
    /// connection. `0` disables the timeout (legacy behaviour).
    pub tcp_idle_timeout_ms: u64,
    /// Bound on the per-session outbound packet queue.
    pub write_queue_capacity: usize,
    /// Bound on the driver-wide event queue.
    pub event_queue_capacity: usize,
    /// Bound on the driver-wide command queue.
    pub command_queue_capacity: usize,
    /// Idle timeout (ms) after which sessions with no inbound traffic are
    /// closed.
    pub session_idle_timeout_ms: u64,
    /// Handshake timeout (ms) for ICE/DTLS to come up.
    pub handshake_timeout_ms: u64,
    /// Stale route TTL (ms) used during connection migration.
    pub migration_route_ttl_ms: u64,
    /// Number of session-owner shards to spawn. `0` selects the
    /// runtime default (`available_parallelism()`, minimum 1). The
    /// current driver uses one shard for protocol state but exposes
    /// the value through `WebRtcDriverHandle::shard_count` so the
    /// public API stays stable when the multi-shard front-end lands.
    pub driver_shards: usize,
    /// Per-shard command channel capacity. Reserved for the
    /// multi-shard front-end; falls back to `command_queue_capacity`
    /// when the front-end is single-shard.
    pub shard_command_capacity: usize,
    /// Hard cap on the number of `(remote address, shard)` bindings
    /// that the global route directory will hold at once. Above the
    /// cap the front-end surfaces a `RouteDirectoryFull` diagnostic
    /// and drops the offending datagram.
    pub route_directory_capacity: usize,
    /// Hard cap on the number of stale (post-migration) bindings the
    /// route directory keeps around.
    pub route_directory_stale_capacity: usize,
    /// Whether the supervisor should automatically respawn a shard
    /// task that exited with a panic. When `true` (default `false`),
    /// the supervisor calls `evict_shard` to clear orphaned
    /// directory entries and then re-spawns the shard loop with
    /// fresh state. The session count budget allowed per shard
    /// across the driver lifetime is `shard_max_restart_count`; once
    /// exceeded the supervisor stops emitting fresh tasks for that
    /// shard and only surfaces `ShardStopped { reason }` events.
    pub shard_restart_on_panic: bool,
    /// Maximum number of times each individual shard may be
    /// auto-restarted by the supervisor over the driver's lifetime.
    /// Defaults to 3 — the goal is to recover from transient
    /// panics while still surfacing systemic failures (a crash loop
    /// indicates deterministic state corruption that retries will
    /// not fix). Ignored when `shard_restart_on_panic` is false.
    pub shard_max_restart_count: u32,
    /// Initial backoff (ms) for the shard restart loop. The
    /// supervisor sleeps for this long before respawning the first
    /// time and doubles the wait on each subsequent restart up to
    /// `shard_max_restart_backoff_ms`.
    pub shard_restart_backoff_ms: u64,
    /// Upper bound (ms) on the exponential backoff between shard
    /// restarts. Prevents unbounded growth when a shard panics
    /// repeatedly.
    pub shard_max_restart_backoff_ms: u64,
    /// Inner core configuration applied to every session.
    pub core: WebRtcCoreConfig,
}

impl Default for WebRtcDriverConfig {
    fn default() -> Self {
        Self {
            listen_udp: "0.0.0.0:8000".parse().expect("default listen addr"),
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
    pub fn validate(&self) -> Result<(), String> {
        if let Some(range) = &self.udp_port_range {
            range.validate()?;
        }
        Ok(())
    }
}
