use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};

use cheetah_sdk::{BackpressurePolicy, SdkError};
use serde::{Deserialize, Serialize};

/// `RtspModuleConfig` data structure.
/// `RtspModuleConfig` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtspModuleConfig {
    /// 是否启用 RTSP module。
    pub enabled: bool,
    /// RTSP TCP 监听地址，格式为 `ip:port`。
    pub listen: String,
    /// 会话空闲超时时间（秒）。
    pub session_timeout_secs: u32,
    /// 单连接发送队列容量（单位：包）。
    pub write_queue_capacity: usize,
    /// 每个订阅者的缓存队列容量（单位：帧）。
    pub subscriber_queue_capacity: usize,
    /// 订阅者背压策略，用于慢消费者处理。
    pub subscriber_backpressure: BackpressurePolicy,
    /// 新订阅者是否必须从关键帧开始下发。
    pub start_from_keyframe: bool,
    /// 新订阅者启动阶段可回放的最大帧数。
    pub bootstrap_max_frames: usize,
    /// RTP 分包 MTU（字节），建议保持在网络路径可承载范围内。
    pub rtp_mtu: usize,
    /// DESCRIBE 在源不存在时的等待时长（毫秒）；0 表示立即返回 404。
    pub play_wait_source_timeout_ms: u64,
    /// DESCRIBE 等待所有 Track 就绪（extradata 填充完成）的超时时长（毫秒）。
    /// 用于跨协议场景：RTMP 序列头可能晚于首帧到达。0 表示不等待。
    pub track_ready_timeout_ms: u64,
    /// 推流断开后保持源存活的时长（毫秒），允许推流端重连。0 表示立即释放。
    pub continue_push_ms: u64,
    /// 是否为纯视频流自动注入静音 AAC 音频。
    pub enable_mute_audio: bool,
    /// 是否启用 Direct Proxy 模式（RTSP→RTSP 零解码 RTP 转发）。
    pub enable_direct_proxy: bool,
    /// RTSP 鉴权配置。
    pub auth: RtspAuthConfig,
    /// RTSPS (TLS) 配置。
    pub tls: RtspTlsConfig,
    /// RTP over UDP 传输配置。
    pub udp: RtspUdpConfig,
    /// RTP multicast PLAY 传输配置。
    pub multicast: RtspMulticastConfig,
    /// 可观测性告警阈值配置。
    pub alert_thresholds: RtspAlertThresholds,
    /// RTSP 远端拉流任务配置。
    pub pull_jobs: Vec<RtspPullJobConfig>,
    /// RTSP 远端推流任务配置。
    pub push_jobs: Vec<RtspPushJobConfig>,
    /// RTSP 转发任务配置。
    pub relay_jobs: Vec<RtspRelayJobConfig>,
}

/// `RtspAuthConfig` data structure.
/// `RtspAuthConfig` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtspAuthConfig {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `require_publish_auth` field of type `bool`.
    /// `require_publish_auth` 字段，类型为 `bool`.
    pub require_publish_auth: bool,
    /// `realm` field of type `String`.
    /// `realm` 字段，类型为 `String`.
    pub realm: String,
    /// `users` field.
    /// `users` 字段.
    pub users: Vec<RtspAuthUserConfig>,
    /// `allow_basic` field of type `bool`.
    /// `allow_basic` 字段，类型为 `bool`.
    pub allow_basic: bool,
    /// `allow_digest` field of type `bool`.
    /// `allow_digest` 字段，类型为 `bool`.
    pub allow_digest: bool,
    /// `nonce_ttl_secs` field of type `u32`.
    /// `nonce_ttl_secs` 字段，类型为 `u32`.
    pub nonce_ttl_secs: u32,
}

/// `RtspAuthUserConfig` data structure.
/// `RtspAuthUserConfig` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RtspAuthUserConfig {
    /// `username` field of type `String`.
    /// `username` 字段，类型为 `String`.
    pub username: String,
    /// `password` field of type `String`.
    /// `password` 字段，类型为 `String`.
    pub password: String,
}

/// `RtspTlsConfig` data structure.
/// `RtspTlsConfig` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtspTlsConfig {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `listen` field of type `String`.
    /// `listen` 字段，类型为 `String`.
    pub listen: String,
    /// `cert_path` field of type `String`.
    /// `cert_path` 字段，类型为 `String`.
    pub cert_path: String,
    /// `key_path` field of type `String`.
    /// `key_path` 字段，类型为 `String`.
    pub key_path: String,
    /// `handshake_timeout_ms` field of type `u64`.
    /// `handshake_timeout_ms` 字段，类型为 `u64`.
    pub handshake_timeout_ms: u64,
}

impl Default for RtspTlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "0.0.0.0:322".to_string(),
            cert_path: String::new(),
            key_path: String::new(),
            handshake_timeout_ms: 10_000,
        }
    }
}

impl RtspTlsConfig {
    /// `validate` function.
    /// `validate` 函数.
    pub fn validate(&self) -> Result<(), SdkError> {
        if !self.enabled {
            return Ok(());
        }
        if self.cert_path.is_empty() {
            return Err(SdkError::InvalidArgument(
                "rtsp.tls.cert_path must be set when TLS is enabled".to_string(),
            ));
        }
        if self.key_path.is_empty() {
            return Err(SdkError::InvalidArgument(
                "rtsp.tls.key_path must be set when TLS is enabled".to_string(),
            ));
        }
        self.listen
            .parse::<SocketAddr>()
            .map_err(|err| SdkError::InvalidArgument(format!("invalid rtsp.tls.listen: {err}")))?;
        Ok(())
    }
}

/// `RtspHeartbeatMode` enumeration.
/// `RtspHeartbeatMode` 枚举.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RtspHeartbeatMode {
    /// `GetParameter` variant.
    /// `GetParameter` 变体.
    #[default]
    GetParameter,
    /// `Options` variant.
    /// `Options` 变体.
    Options,
}

/// `RtspAlertThresholds` data structure.
/// `RtspAlertThresholds` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtspAlertThresholds {
    /// 起播等待告警阈值（毫秒）。
    pub startup_timeout_ms: u64,
    /// 时间戳逆序修正计数告警阈值。
    pub timestamp_repair_count: u64,
    /// 队列回压丢帧计数告警阈值。
    pub queue_drop_count: u64,
}

/// `RtspUdpConfig` data structure.
/// `RtspUdpConfig` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtspUdpConfig {
    /// RTP/RTCP 服务端端口池起始端口（含）。
    pub server_port_pool_start: u16,
    /// RTP/RTCP 服务端端口池结束端口（含）。
    pub server_port_pool_end: u16,
    /// 端口池分配最大尝试次数。
    pub bind_pair_attempts: usize,
    /// SETUP 后是否发送最小 UDP probe 包（用于 NAT 打洞）。
    pub enable_hole_punching_probe: bool,
    /// NAT 打洞探测超时（毫秒）。超时后使用 SETUP 中声明的地址。
    pub nat_probe_timeout_ms: u64,
    /// 是否接受 NAT 重绑定（源地址变化后继续接收）。
    pub accept_source_change: bool,
    /// 是否随机化端口分配（避免重启后冲突）。
    pub randomize_ports: bool,
    /// 是否启用 UDP publish ingest 的 RTP reorder buffer。
    pub enable_reorder_buffer: bool,
}

/// `RtspMulticastConfig` data structure.
/// `RtspMulticastConfig` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtspMulticastConfig {
    /// 是否启用 RTP multicast PLAY。
    pub enabled: bool,
    /// 组播地址池起始地址（含）。
    pub group_start: Ipv4Addr,
    /// 组播地址池结束地址（含）。
    pub group_end: Ipv4Addr,
    /// 组播 RTP 端口池起始端口（含，建议偶数）。
    pub port_start: u16,
    /// 组播 RTP 端口池结束端口（含，需至少包含一组 RTP/RTCP 对）。
    pub port_end: u16,
    /// 组播 TTL。
    pub ttl: u8,
    /// 组播发送 socket 绑定的本地接口地址；0.0.0.0 表示由系统路由选择。
    pub interface: Ipv4Addr,
    /// 最大并发多播 sender 数。
    pub max_groups: usize,
    /// 最后一个订阅者离开后的 sender 保留时间（毫秒）。
    pub idle_release_ms: u64,
}

/// `RtspPullTransport` enumeration.
/// `RtspPullTransport` 枚举.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RtspPullTransport {
    /// `TcpInterleaved` variant.
    /// `TcpInterleaved` 变体.
    TcpInterleaved,
    /// `Udp` variant.
    /// `Udp` 变体.
    Udp,
    /// `HttpTunnel` variant.
    /// `HttpTunnel` 变体.
    HttpTunnel,
    /// `Multicast` variant.
    /// `Multicast` 变体.
    Multicast,
}

/// `RtspPullJobConfig` data structure.
/// `RtspPullJobConfig` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtspPullJobConfig {
    /// 任务名（同一 module 内唯一）。
    pub name: String,
    /// 是否启用任务。
    pub enabled: bool,
    /// 源 RTSP 地址。
    pub source_url: String,
    /// 写入本地 engine 的目标流 key。
    pub target_stream_key: String,
    /// 认证用户名。
    pub username: Option<String>,
    /// 认证密码。
    pub password: Option<String>,
    /// 传输优先级。
    pub transport_preference: Vec<RtspPullTransport>,
    /// 心跳模式。
    pub heartbeat_mode: RtspHeartbeatMode,
    /// 重试退避起始（毫秒）。
    pub retry_backoff_ms: u64,
    /// 重试退避上限（毫秒）。
    pub max_retry_backoff_ms: u64,
}

/// `RtspPushTransport` enumeration.
/// `RtspPushTransport` 枚举.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RtspPushTransport {
    /// `TcpInterleaved` variant.
    /// `TcpInterleaved` 变体.
    TcpInterleaved,
    /// `Udp` variant.
    /// `Udp` 变体.
    Udp,
    /// `HttpTunnel` variant.
    /// `HttpTunnel` 变体.
    HttpTunnel,
}

/// `RtspPushJobConfig` data structure.
/// `RtspPushJobConfig` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtspPushJobConfig {
    /// 任务名（同一 module 内唯一）。
    pub name: String,
    /// 是否启用任务。
    pub enabled: bool,
    /// 本地源流 key。
    pub source_stream_key: String,
    /// 目标 RTSP 地址。
    pub target_url: String,
    /// 认证用户名。
    pub username: Option<String>,
    /// 认证密码。
    pub password: Option<String>,
    /// 传输优先级。
    pub transport_preference: Vec<RtspPushTransport>,
    /// 重试退避起始（毫秒）。
    pub retry_backoff_ms: u64,
    /// 重试退避上限（毫秒）。
    pub max_retry_backoff_ms: u64,
}

/// `RtspRelayJobConfig` data structure.
/// `RtspRelayJobConfig` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RtspRelayJobConfig {
    /// 任务名（同一 module 内唯一）。
    pub name: String,
    /// 是否启用任务。
    pub enabled: bool,
    /// 源 RTSP 地址。
    pub source_url: String,
    /// 目标 RTSP 地址。
    pub target_url: String,
    /// 本地中继流 key，为空表示内部隐藏流。
    pub local_stream_key: Option<String>,
    /// 传输优先级。
    pub transport_preference: Vec<RtspPullTransport>,
    /// 重试退避起始（毫秒）。
    pub retry_backoff_ms: u64,
    /// 重试退避上限（毫秒）。
    pub max_retry_backoff_ms: u64,
}

impl Default for RtspModuleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: "0.0.0.0:554".to_string(),
            session_timeout_secs: 60,
            write_queue_capacity: 256,
            subscriber_queue_capacity: 256,
            subscriber_backpressure: BackpressurePolicy::DropUntilNextKeyframe,
            start_from_keyframe: true,
            bootstrap_max_frames: 150,
            rtp_mtu: 1200,
            play_wait_source_timeout_ms: 0,
            track_ready_timeout_ms: 500,
            continue_push_ms: 0,
            enable_mute_audio: false,
            enable_direct_proxy: false,
            auth: RtspAuthConfig::default(),
            tls: RtspTlsConfig::default(),
            udp: RtspUdpConfig::default(),
            multicast: RtspMulticastConfig::default(),
            alert_thresholds: RtspAlertThresholds::default(),
            pull_jobs: Vec::new(),
            push_jobs: Vec::new(),
            relay_jobs: Vec::new(),
        }
    }
}

impl Default for RtspAuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            require_publish_auth: false,
            realm: "cheetah".to_string(),
            users: Vec::new(),
            allow_basic: true,
            allow_digest: true,
            nonce_ttl_secs: 60,
        }
    }
}

impl Default for RtspAlertThresholds {
    fn default() -> Self {
        Self {
            startup_timeout_ms: 3_000,
            timestamp_repair_count: 32,
            queue_drop_count: 64,
        }
    }
}

impl Default for RtspUdpConfig {
    fn default() -> Self {
        Self {
            server_port_pool_start: 62_000,
            server_port_pool_end: 62_999,
            bind_pair_attempts: 64,
            enable_hole_punching_probe: false,
            nat_probe_timeout_ms: 5_000,
            accept_source_change: true,
            randomize_ports: false,
            enable_reorder_buffer: false,
        }
    }
}

impl Default for RtspMulticastConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            group_start: Ipv4Addr::new(239, 1, 0, 1),
            group_end: Ipv4Addr::new(239, 1, 0, 255),
            port_start: 63_000,
            port_end: 63_511,
            ttl: 16,
            interface: Ipv4Addr::new(0, 0, 0, 0),
            max_groups: 256,
            idle_release_ms: 30_000,
        }
    }
}

impl Default for RtspPullJobConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: false,
            source_url: String::new(),
            target_stream_key: String::new(),
            username: None,
            password: None,
            transport_preference: vec![
                RtspPullTransport::TcpInterleaved,
                RtspPullTransport::Udp,
                RtspPullTransport::HttpTunnel,
                RtspPullTransport::Multicast,
            ],
            heartbeat_mode: RtspHeartbeatMode::default(),
            retry_backoff_ms: 1_000,
            max_retry_backoff_ms: 30_000,
        }
    }
}

impl Default for RtspPushJobConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: false,
            source_stream_key: String::new(),
            target_url: String::new(),
            username: None,
            password: None,
            transport_preference: vec![
                RtspPushTransport::TcpInterleaved,
                RtspPushTransport::Udp,
                RtspPushTransport::HttpTunnel,
            ],
            retry_backoff_ms: 1_000,
            max_retry_backoff_ms: 30_000,
        }
    }
}

impl Default for RtspRelayJobConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: false,
            source_url: String::new(),
            target_url: String::new(),
            local_stream_key: None,
            transport_preference: vec![
                RtspPullTransport::TcpInterleaved,
                RtspPullTransport::Udp,
                RtspPullTransport::HttpTunnel,
            ],
            retry_backoff_ms: 1_000,
            max_retry_backoff_ms: 30_000,
        }
    }
}

impl RtspModuleConfig {
    /// Creates `value` from input.
    /// 创建 `值` 来自 输入.
    pub fn from_value(value: serde_json::Value) -> Result<Self, SdkError> {
        let cfg: Self = serde_json::from_value(value)
            .map_err(|err| SdkError::InvalidArgument(format!("invalid rtsp config: {err}")))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// `validate` function.
    /// `validate` 函数.
    pub fn validate(&self) -> Result<(), SdkError> {
        self.listen
            .parse::<SocketAddr>()
            .map_err(|err| SdkError::InvalidArgument(format!("invalid rtsp.listen: {err}")))?;
        if self.session_timeout_secs == 0 {
            return Err(SdkError::InvalidArgument(
                "rtsp.session_timeout_secs must be > 0".to_string(),
            ));
        }
        if self.write_queue_capacity == 0 {
            return Err(SdkError::InvalidArgument(
                "rtsp.write_queue_capacity must be > 0".to_string(),
            ));
        }
        if self.subscriber_queue_capacity == 0 {
            return Err(SdkError::InvalidArgument(
                "rtsp.subscriber_queue_capacity must be > 0".to_string(),
            ));
        }
        if self.bootstrap_max_frames == 0 {
            return Err(SdkError::InvalidArgument(
                "rtsp.bootstrap_max_frames must be > 0".to_string(),
            ));
        }
        if self.rtp_mtu < 400 {
            return Err(SdkError::InvalidArgument(
                "rtsp.rtp_mtu must be >= 400".to_string(),
            ));
        }
        if self.rtp_mtu > 9000 {
            return Err(SdkError::InvalidArgument(
                "rtsp.rtp_mtu must be <= 9000 (jumbo frame limit)".to_string(),
            ));
        }
        if self.udp.server_port_pool_start >= self.udp.server_port_pool_end {
            return Err(SdkError::InvalidArgument(
                "rtsp.udp.server_port_pool_start must be < rtsp.udp.server_port_pool_end"
                    .to_string(),
            ));
        }
        let first_rtp_port = if self.udp.server_port_pool_start.is_multiple_of(2) {
            self.udp.server_port_pool_start
        } else {
            self.udp.server_port_pool_start.saturating_add(1)
        };
        if first_rtp_port == u16::MAX
            || first_rtp_port.saturating_add(1) > self.udp.server_port_pool_end
        {
            return Err(SdkError::InvalidArgument(
                "rtsp.udp.server_port_pool must contain at least one even RTP port followed by its RTCP port"
                    .to_string(),
            ));
        }
        if self.udp.bind_pair_attempts == 0 {
            return Err(SdkError::InvalidArgument(
                "rtsp.udp.bind_pair_attempts must be > 0".to_string(),
            ));
        }
        if self.multicast.group_start > self.multicast.group_end {
            return Err(SdkError::InvalidArgument(
                "rtsp.multicast.group_start must be <= rtsp.multicast.group_end".to_string(),
            ));
        }
        if !self.multicast.group_start.is_multicast() || !self.multicast.group_end.is_multicast() {
            return Err(SdkError::InvalidArgument(
                "rtsp.multicast.group_start/group_end must be multicast addresses".to_string(),
            ));
        }
        if self.multicast.group_start.octets()[0] != 239
            || self.multicast.group_end.octets()[0] != 239
        {
            return Err(SdkError::InvalidArgument(
                "rtsp.multicast.group_start/group_end must be within 239.0.0.0/8".to_string(),
            ));
        }
        let first_even_port = if self.multicast.port_start.is_multiple_of(2) {
            self.multicast.port_start
        } else {
            self.multicast.port_start.saturating_add(1)
        };
        if first_even_port.saturating_add(1) > self.multicast.port_end {
            return Err(SdkError::InvalidArgument(
                "rtsp.multicast.port_start/port_end must include at least one even/odd RTP/RTCP pair"
                    .to_string(),
            ));
        }
        if self.multicast.ttl == 0 {
            return Err(SdkError::InvalidArgument(
                "rtsp.multicast.ttl must be > 0".to_string(),
            ));
        }
        if self.multicast.max_groups == 0 {
            return Err(SdkError::InvalidArgument(
                "rtsp.multicast.max_groups must be > 0".to_string(),
            ));
        }
        if self.multicast.idle_release_ms == 0 {
            return Err(SdkError::InvalidArgument(
                "rtsp.multicast.idle_release_ms must be > 0".to_string(),
            ));
        }
        if self.multicast.interface.is_multicast() {
            return Err(SdkError::InvalidArgument(
                "rtsp.multicast.interface must not be a multicast address".to_string(),
            ));
        }
        if self.auth.enabled {
            if !self.auth.allow_basic && !self.auth.allow_digest {
                return Err(SdkError::InvalidArgument(
                    "rtsp.auth requires allow_basic or allow_digest".to_string(),
                ));
            }
            if self.auth.realm.trim().is_empty() {
                return Err(SdkError::InvalidArgument(
                    "rtsp.auth.realm must not be empty".to_string(),
                ));
            }
            if self.auth.users.is_empty() {
                return Err(SdkError::InvalidArgument(
                    "rtsp.auth.users must not be empty when auth is enabled".to_string(),
                ));
            }
            if self.auth.nonce_ttl_secs == 0 {
                return Err(SdkError::InvalidArgument(
                    "rtsp.auth.nonce_ttl_secs must be > 0".to_string(),
                ));
            }
            for (index, user) in self.auth.users.iter().enumerate() {
                if user.username.trim().is_empty() {
                    return Err(SdkError::InvalidArgument(format!(
                        "rtsp.auth.users[{index}].username must not be empty"
                    )));
                }
                if user.password.is_empty() {
                    return Err(SdkError::InvalidArgument(format!(
                        "rtsp.auth.users[{index}].password must not be empty"
                    )));
                }
            }
        }
        if self.alert_thresholds.startup_timeout_ms == 0 {
            return Err(SdkError::InvalidArgument(
                "rtsp.alert_thresholds.startup_timeout_ms must be > 0".to_string(),
            ));
        }
        if self.alert_thresholds.timestamp_repair_count == 0 {
            return Err(SdkError::InvalidArgument(
                "rtsp.alert_thresholds.timestamp_repair_count must be > 0".to_string(),
            ));
        }
        if self.alert_thresholds.queue_drop_count == 0 {
            return Err(SdkError::InvalidArgument(
                "rtsp.alert_thresholds.queue_drop_count must be > 0".to_string(),
            ));
        }
        let mut job_names = HashSet::<String>::new();
        for (index, job) in self.pull_jobs.iter().enumerate() {
            let name = job.name.trim();
            if name.is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.pull_jobs[{index}].name must not be empty"
                )));
            }
            if !job_names.insert(name.to_string()) {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.pull_jobs contains duplicated name: {name}"
                )));
            }
            Self::validate_rtsp_url(
                &job.source_url,
                &format!("rtsp.pull_jobs[{index}].source_url"),
            )?;
            if job.target_stream_key.trim().is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.pull_jobs[{index}].target_stream_key must not be empty"
                )));
            }
            if job.transport_preference.is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.pull_jobs[{index}].transport_preference must not be empty"
                )));
            }
            let mut transport_set = HashSet::<RtspPullTransport>::new();
            for transport in job.transport_preference.iter().copied() {
                if !transport_set.insert(transport) {
                    return Err(SdkError::InvalidArgument(format!(
                        "rtsp.pull_jobs[{index}].transport_preference must not contain duplicates"
                    )));
                }
            }
            if job.retry_backoff_ms == 0 {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.pull_jobs[{index}].retry_backoff_ms must be > 0"
                )));
            }
            if job.max_retry_backoff_ms < job.retry_backoff_ms {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.pull_jobs[{index}].max_retry_backoff_ms must be >= retry_backoff_ms"
                )));
            }
            Self::validate_optional_credentials(
                &job.username,
                &job.password,
                &format!("rtsp.pull_jobs[{index}]"),
            )?;
        }

        for (index, job) in self.push_jobs.iter().enumerate() {
            let name = job.name.trim();
            if name.is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.push_jobs[{index}].name must not be empty"
                )));
            }
            if !job_names.insert(name.to_string()) {
                return Err(SdkError::InvalidArgument(format!(
                    "duplicated rtsp job name across pull/push/relay jobs: {name}"
                )));
            }
            if job.source_stream_key.trim().is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.push_jobs[{index}].source_stream_key must not be empty"
                )));
            }
            Self::validate_rtsp_url(
                &job.target_url,
                &format!("rtsp.push_jobs[{index}].target_url"),
            )?;
            if job.transport_preference.is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.push_jobs[{index}].transport_preference must not be empty"
                )));
            }
            let mut transport_set = HashSet::<RtspPushTransport>::new();
            for transport in job.transport_preference.iter().copied() {
                if !transport_set.insert(transport) {
                    return Err(SdkError::InvalidArgument(format!(
                        "rtsp.push_jobs[{index}].transport_preference must not contain duplicates"
                    )));
                }
            }
            if job.retry_backoff_ms == 0 {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.push_jobs[{index}].retry_backoff_ms must be > 0"
                )));
            }
            if job.max_retry_backoff_ms < job.retry_backoff_ms {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.push_jobs[{index}].max_retry_backoff_ms must be >= retry_backoff_ms"
                )));
            }
            Self::validate_optional_credentials(
                &job.username,
                &job.password,
                &format!("rtsp.push_jobs[{index}]"),
            )?;
        }

        for (index, job) in self.relay_jobs.iter().enumerate() {
            let name = job.name.trim();
            if name.is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.relay_jobs[{index}].name must not be empty"
                )));
            }
            if !job_names.insert(name.to_string()) {
                return Err(SdkError::InvalidArgument(format!(
                    "duplicated rtsp job name across pull/push/relay jobs: {name}"
                )));
            }
            Self::validate_rtsp_url(
                &job.source_url,
                &format!("rtsp.relay_jobs[{index}].source_url"),
            )?;
            Self::validate_rtsp_url(
                &job.target_url,
                &format!("rtsp.relay_jobs[{index}].target_url"),
            )?;
            if job
                .local_stream_key
                .as_ref()
                .is_some_and(|key| key.trim().is_empty())
            {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.relay_jobs[{index}].local_stream_key must not be empty when provided"
                )));
            }
            if job.transport_preference.is_empty() {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.relay_jobs[{index}].transport_preference must not be empty"
                )));
            }
            let mut transport_set = HashSet::<RtspPullTransport>::new();
            for transport in job.transport_preference.iter().copied() {
                if !transport_set.insert(transport) {
                    return Err(SdkError::InvalidArgument(format!(
                        "rtsp.relay_jobs[{index}].transport_preference must not contain duplicates"
                    )));
                }
            }
            if job.retry_backoff_ms == 0 {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.relay_jobs[{index}].retry_backoff_ms must be > 0"
                )));
            }
            if job.max_retry_backoff_ms < job.retry_backoff_ms {
                return Err(SdkError::InvalidArgument(format!(
                    "rtsp.relay_jobs[{index}].max_retry_backoff_ms must be >= retry_backoff_ms"
                )));
            }
        }
        self.tls.validate()?;
        Ok(())
    }

    /// `default_json` function.
    /// `default_json` 函数.
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).expect("serialize default rtsp config")
    }

    fn validate_rtsp_url(value: &str, field: &str) -> Result<(), SdkError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(SdkError::InvalidArgument(format!(
                "{field} must not be empty"
            )));
        }
        if !value.starts_with("rtsp://") {
            return Err(SdkError::InvalidArgument(format!(
                "{field} must start with rtsp://"
            )));
        }
        let authority = &value["rtsp://".len()..];
        if authority.is_empty() || authority.starts_with('/') {
            return Err(SdkError::InvalidArgument(format!(
                "{field} must include host"
            )));
        }
        Ok(())
    }

    fn validate_optional_credentials(
        username: &Option<String>,
        password: &Option<String>,
        field_prefix: &str,
    ) -> Result<(), SdkError> {
        match (username, password) {
            (Some(username), Some(_)) | (Some(username), None) => {
                if username.trim().is_empty() {
                    return Err(SdkError::InvalidArgument(format!(
                        "{field_prefix}.username must not be empty when provided"
                    )));
                }
            }
            (None, Some(_)) => {
                return Err(SdkError::InvalidArgument(format!(
                    "{field_prefix}.password requires username"
                )));
            }
            (None, None) => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_positive_session_timeout() {
        let cfg = RtspModuleConfig::default();
        assert_eq!(cfg.session_timeout_secs, 60);
        assert!(!cfg.auth.enabled);
        assert_eq!(cfg.udp.server_port_pool_start, 62_000);
        assert_eq!(cfg.udp.server_port_pool_end, 62_999);
        assert_eq!(cfg.udp.bind_pair_attempts, 64);
        assert!(!cfg.udp.enable_hole_punching_probe);
        assert!(!cfg.udp.enable_reorder_buffer);
        assert!(!cfg.multicast.enabled);
        assert_eq!(cfg.multicast.group_start, Ipv4Addr::new(239, 1, 0, 1));
        assert_eq!(cfg.multicast.group_end, Ipv4Addr::new(239, 1, 0, 255));
        assert_eq!(cfg.multicast.port_start, 63_000);
        assert_eq!(cfg.multicast.port_end, 63_511);
        assert_eq!(cfg.multicast.ttl, 16);
        assert_eq!(cfg.multicast.interface, Ipv4Addr::new(0, 0, 0, 0));
        assert_eq!(cfg.multicast.max_groups, 256);
        assert_eq!(cfg.multicast.idle_release_ms, 30_000);
        assert_eq!(cfg.auth.realm, "cheetah");
        assert_eq!(cfg.play_wait_source_timeout_ms, 0);
        assert_eq!(cfg.alert_thresholds.startup_timeout_ms, 3_000);
        assert_eq!(cfg.alert_thresholds.timestamp_repair_count, 32);
        assert_eq!(cfg.alert_thresholds.queue_drop_count, 64);
        assert!(cfg.pull_jobs.is_empty());
        assert!(cfg.push_jobs.is_empty());
        assert!(cfg.relay_jobs.is_empty());
        cfg.validate().expect("default config should validate");
    }

    #[test]
    fn reject_zero_session_timeout() {
        let cfg = RtspModuleConfig {
            session_timeout_secs: 0,
            ..RtspModuleConfig::default()
        };
        let err = cfg.validate().expect_err("must reject zero timeout");
        assert!(err.to_string().contains("rtsp.session_timeout_secs"));
    }

    #[test]
    fn reject_zero_alert_threshold() {
        let cfg = RtspModuleConfig {
            alert_thresholds: RtspAlertThresholds {
                startup_timeout_ms: 1,
                timestamp_repair_count: 0,
                queue_drop_count: 64,
            },
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject zero alert threshold");
        assert!(err
            .to_string()
            .contains("rtsp.alert_thresholds.timestamp_repair_count"));
    }

    #[test]
    fn reject_enabled_auth_without_users() {
        let cfg = RtspModuleConfig {
            auth: RtspAuthConfig {
                enabled: true,
                users: Vec::new(),
                ..RtspAuthConfig::default()
            },
            ..RtspModuleConfig::default()
        };
        let err = cfg.validate().expect_err("must reject auth without users");
        assert!(err.to_string().contains("rtsp.auth.users"));
    }

    #[test]
    fn reject_enabled_auth_without_enabled_scheme() {
        let cfg = RtspModuleConfig {
            auth: RtspAuthConfig {
                enabled: true,
                allow_basic: false,
                allow_digest: false,
                users: vec![RtspAuthUserConfig {
                    username: "user".to_string(),
                    password: "pass".to_string(),
                }],
                ..RtspAuthConfig::default()
            },
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject auth without enabled scheme");
        assert!(err.to_string().contains("allow_basic or allow_digest"));
    }

    #[test]
    fn reject_invalid_udp_pool_range() {
        let cfg = RtspModuleConfig {
            udp: RtspUdpConfig {
                server_port_pool_start: 62_001,
                server_port_pool_end: 62_000,
                ..RtspUdpConfig::default()
            },
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject invalid udp port pool range");
        assert!(err.to_string().contains("rtsp.udp.server_port_pool_start"));
    }

    #[test]
    fn reject_udp_pool_without_even_rtp_followed_by_rtcp_pair() {
        let cfg = RtspModuleConfig {
            udp: RtspUdpConfig {
                server_port_pool_start: 62_001,
                server_port_pool_end: 62_002,
                ..RtspUdpConfig::default()
            },
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject udp port pool without an RTP/RTCP pair");
        assert!(err.to_string().contains("rtsp.udp.server_port_pool"));
    }

    #[test]
    fn reject_zero_udp_bind_attempts() {
        let cfg = RtspModuleConfig {
            udp: RtspUdpConfig {
                bind_pair_attempts: 0,
                ..RtspUdpConfig::default()
            },
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject zero udp bind attempts");
        assert!(err.to_string().contains("rtsp.udp.bind_pair_attempts"));
    }

    #[test]
    fn reject_invalid_multicast_group_range() {
        let cfg = RtspModuleConfig {
            multicast: RtspMulticastConfig {
                group_start: Ipv4Addr::new(239, 1, 0, 2),
                group_end: Ipv4Addr::new(239, 1, 0, 1),
                ..RtspMulticastConfig::default()
            },
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject invalid multicast group range");
        assert!(err.to_string().contains("rtsp.multicast.group_start"));
    }

    #[test]
    fn reject_non_admin_scoped_multicast_range() {
        let cfg = RtspModuleConfig {
            multicast: RtspMulticastConfig {
                group_start: Ipv4Addr::new(224, 1, 0, 1),
                group_end: Ipv4Addr::new(224, 1, 0, 2),
                ..RtspMulticastConfig::default()
            },
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject non admin scoped multicast range");
        assert!(err.to_string().contains("239.0.0.0/8"));
    }

    #[test]
    fn reject_invalid_multicast_port_pool() {
        let cfg = RtspModuleConfig {
            multicast: RtspMulticastConfig {
                port_start: 63_001,
                port_end: 63_002,
                ..RtspMulticastConfig::default()
            },
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject invalid multicast port pool");
        assert!(err
            .to_string()
            .contains("rtsp.multicast.port_start/port_end"));
    }

    #[test]
    fn reject_zero_multicast_idle_release() {
        let cfg = RtspModuleConfig {
            multicast: RtspMulticastConfig {
                idle_release_ms: 0,
                ..RtspMulticastConfig::default()
            },
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject zero multicast idle release");
        assert!(err.to_string().contains("rtsp.multicast.idle_release_ms"));
    }

    #[test]
    fn pull_job_validates_successfully() {
        let cfg = RtspModuleConfig {
            pull_jobs: vec![RtspPullJobConfig {
                name: "cam-main".to_string(),
                enabled: true,
                source_url: "rtsp://127.0.0.1:554/live/cam-main".to_string(),
                target_stream_key: "live/cam-main".to_string(),
                username: Some("user".to_string()),
                password: Some("pass".to_string()),
                transport_preference: vec![
                    RtspPullTransport::TcpInterleaved,
                    RtspPullTransport::Udp,
                ],
                heartbeat_mode: RtspHeartbeatMode::default(),
                retry_backoff_ms: 500,
                max_retry_backoff_ms: 5_000,
            }],
            ..RtspModuleConfig::default()
        };
        cfg.validate().expect("valid pull job should pass");
    }

    #[test]
    fn reject_pull_job_with_empty_name() {
        let cfg = RtspModuleConfig {
            pull_jobs: vec![RtspPullJobConfig {
                name: " ".to_string(),
                source_url: "rtsp://127.0.0.1:554/live/a".to_string(),
                target_stream_key: "live/a".to_string(),
                ..RtspPullJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject pull job with empty name");
        assert!(err.to_string().contains("rtsp.pull_jobs[0].name"));
    }

    #[test]
    fn reject_pull_job_with_duplicated_name() {
        let cfg = RtspModuleConfig {
            pull_jobs: vec![
                RtspPullJobConfig {
                    name: "cam-1".to_string(),
                    source_url: "rtsp://127.0.0.1:554/live/a".to_string(),
                    target_stream_key: "live/a".to_string(),
                    ..RtspPullJobConfig::default()
                },
                RtspPullJobConfig {
                    name: "cam-1".to_string(),
                    source_url: "rtsp://127.0.0.1:554/live/b".to_string(),
                    target_stream_key: "live/b".to_string(),
                    ..RtspPullJobConfig::default()
                },
            ],
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject duplicated pull job name");
        assert!(err.to_string().contains("duplicated name"));
    }

    #[test]
    fn reject_pull_job_with_invalid_source_url() {
        let cfg = RtspModuleConfig {
            pull_jobs: vec![RtspPullJobConfig {
                name: "cam-1".to_string(),
                source_url: "http://127.0.0.1/live/a".to_string(),
                target_stream_key: "live/a".to_string(),
                ..RtspPullJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg.validate().expect_err("must reject non-rtsp source url");
        assert!(err
            .to_string()
            .contains("source_url must start with rtsp://"));
    }

    #[test]
    fn reject_pull_job_with_empty_transport_preference() {
        let cfg = RtspModuleConfig {
            pull_jobs: vec![RtspPullJobConfig {
                name: "cam-1".to_string(),
                source_url: "rtsp://127.0.0.1:554/live/a".to_string(),
                target_stream_key: "live/a".to_string(),
                transport_preference: Vec::new(),
                ..RtspPullJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject empty transport preference");
        assert!(err.to_string().contains("transport_preference"));
    }

    #[test]
    fn reject_pull_job_with_password_without_username() {
        let cfg = RtspModuleConfig {
            pull_jobs: vec![RtspPullJobConfig {
                name: "cam-1".to_string(),
                source_url: "rtsp://127.0.0.1:554/live/a".to_string(),
                target_stream_key: "live/a".to_string(),
                username: None,
                password: Some("pass".to_string()),
                ..RtspPullJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject password without username");
        assert!(err.to_string().contains("password requires username"));
    }

    #[test]
    fn reject_pull_job_with_invalid_backoff() {
        let cfg = RtspModuleConfig {
            pull_jobs: vec![RtspPullJobConfig {
                name: "cam-1".to_string(),
                source_url: "rtsp://127.0.0.1:554/live/a".to_string(),
                target_stream_key: "live/a".to_string(),
                retry_backoff_ms: 5_000,
                max_retry_backoff_ms: 1_000,
                ..RtspPullJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg.validate().expect_err("must reject invalid backoff");
        assert!(err.to_string().contains("max_retry_backoff_ms"));
    }

    #[test]
    fn push_job_validates_successfully() {
        let cfg = RtspModuleConfig {
            push_jobs: vec![RtspPushJobConfig {
                name: "push-main".to_string(),
                enabled: true,
                source_stream_key: "live/push-main".to_string(),
                target_url: "rtsp://127.0.0.1:8554/live/push-main".to_string(),
                username: Some("user".to_string()),
                password: Some("pass".to_string()),
                transport_preference: vec![
                    RtspPushTransport::TcpInterleaved,
                    RtspPushTransport::Udp,
                ],
                retry_backoff_ms: 500,
                max_retry_backoff_ms: 5_000,
            }],
            ..RtspModuleConfig::default()
        };
        cfg.validate().expect("valid push job should pass");
    }

    #[test]
    fn reject_push_job_with_invalid_target_url() {
        let cfg = RtspModuleConfig {
            push_jobs: vec![RtspPushJobConfig {
                name: "push-main".to_string(),
                source_stream_key: "live/push-main".to_string(),
                target_url: "http://127.0.0.1/live/push-main".to_string(),
                ..RtspPushJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject non-rtsp push target url");
        assert!(err
            .to_string()
            .contains("target_url must start with rtsp://"));
    }

    #[test]
    fn reject_push_job_with_password_without_username() {
        let cfg = RtspModuleConfig {
            push_jobs: vec![RtspPushJobConfig {
                name: "push-main".to_string(),
                source_stream_key: "live/push-main".to_string(),
                target_url: "rtsp://127.0.0.1/live/push-main".to_string(),
                username: None,
                password: Some("pass".to_string()),
                ..RtspPushJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject push password without username");
        assert!(err
            .to_string()
            .contains("rtsp.push_jobs[0].password requires username"));
    }

    #[test]
    fn reject_push_job_with_invalid_backoff() {
        let cfg = RtspModuleConfig {
            push_jobs: vec![RtspPushJobConfig {
                name: "push-main".to_string(),
                source_stream_key: "live/push-main".to_string(),
                target_url: "rtsp://127.0.0.1/live/push-main".to_string(),
                retry_backoff_ms: 5_000,
                max_retry_backoff_ms: 1_000,
                ..RtspPushJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject invalid push backoff");
        assert!(err
            .to_string()
            .contains("rtsp.push_jobs[0].max_retry_backoff_ms"));
    }

    #[test]
    fn relay_job_validates_successfully() {
        let cfg = RtspModuleConfig {
            relay_jobs: vec![RtspRelayJobConfig {
                name: "relay-main".to_string(),
                enabled: true,
                source_url: "rtsp://127.0.0.1:8554/live/source".to_string(),
                target_url: "rtsp://127.0.0.1:8555/live/target".to_string(),
                local_stream_key: Some("live/relay-main".to_string()),
                transport_preference: vec![
                    RtspPullTransport::TcpInterleaved,
                    RtspPullTransport::Udp,
                ],
                retry_backoff_ms: 500,
                max_retry_backoff_ms: 5_000,
            }],
            ..RtspModuleConfig::default()
        };
        cfg.validate().expect("valid relay job should pass");
    }

    #[test]
    fn reject_relay_job_with_empty_local_stream_key() {
        let cfg = RtspModuleConfig {
            relay_jobs: vec![RtspRelayJobConfig {
                name: "relay-main".to_string(),
                source_url: "rtsp://127.0.0.1:8554/live/source".to_string(),
                target_url: "rtsp://127.0.0.1:8555/live/target".to_string(),
                local_stream_key: Some("   ".to_string()),
                ..RtspRelayJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject empty relay local stream key");
        assert!(err
            .to_string()
            .contains("rtsp.relay_jobs[0].local_stream_key"));
    }

    #[test]
    fn reject_relay_job_with_invalid_target_url() {
        let cfg = RtspModuleConfig {
            relay_jobs: vec![RtspRelayJobConfig {
                name: "relay-main".to_string(),
                source_url: "rtsp://127.0.0.1:8554/live/source".to_string(),
                target_url: "ftp://127.0.0.1:8555/live/target".to_string(),
                ..RtspRelayJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject non-rtsp relay target url");
        assert!(err
            .to_string()
            .contains("target_url must start with rtsp://"));
    }

    #[test]
    fn reject_relay_job_with_invalid_backoff() {
        let cfg = RtspModuleConfig {
            relay_jobs: vec![RtspRelayJobConfig {
                name: "relay-main".to_string(),
                source_url: "rtsp://127.0.0.1:8554/live/source".to_string(),
                target_url: "rtsp://127.0.0.1:8555/live/target".to_string(),
                retry_backoff_ms: 5_000,
                max_retry_backoff_ms: 1_000,
                ..RtspRelayJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject invalid relay backoff");
        assert!(err
            .to_string()
            .contains("rtsp.relay_jobs[0].max_retry_backoff_ms"));
    }

    #[test]
    fn reject_cross_job_type_duplicated_name() {
        let cfg = RtspModuleConfig {
            pull_jobs: vec![RtspPullJobConfig {
                name: "dup-job".to_string(),
                source_url: "rtsp://127.0.0.1:554/live/a".to_string(),
                target_stream_key: "live/a".to_string(),
                ..RtspPullJobConfig::default()
            }],
            push_jobs: vec![RtspPushJobConfig {
                name: "dup-job".to_string(),
                source_stream_key: "live/a".to_string(),
                target_url: "rtsp://127.0.0.1:8554/live/a".to_string(),
                ..RtspPushJobConfig::default()
            }],
            ..RtspModuleConfig::default()
        };
        let err = cfg
            .validate()
            .expect_err("must reject duplicated names across job types");
        assert!(err.to_string().contains("duplicated rtsp job name"));
    }
}
