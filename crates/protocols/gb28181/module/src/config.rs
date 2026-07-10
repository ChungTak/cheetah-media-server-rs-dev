use serde::{Deserialize, Serialize};

/// `Gb28181ModuleConfig` data structure.
/// `Gb28181ModuleConfig` 数据结构.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gb28181ModuleConfig {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `listen_udp` field of type `String`.
    /// `listen_udp` 字段，类型为 `String`.
    #[serde(default = "default_listen_udp")]
    pub listen_udp: String,
    /// `listen_tcp` field of type `String`.
    /// `listen_tcp` 字段，类型为 `String`.
    #[serde(default = "default_listen_tcp")]
    pub listen_tcp: String,
    /// `read_buffer_size` field of type `usize`.
    /// `read_buffer_size` 字段，类型为 `usize`.
    #[serde(default = "default_read_buffer_size")]
    pub read_buffer_size: usize,
    /// `tick_interval_ms` field of type `u64`.
    /// `tick_interval_ms` 字段，类型为 `u64`.
    #[serde(default = "default_tick_interval_ms")]
    pub tick_interval_ms: u64,
    /// Local IP advertised in SDP `c=IN IP4 ...` and `m=` lines, plus SIP `Contact`/`Via`.
    /// If empty, falls back to the listen UDP address.
    #[serde(default)]
    pub public_ip: String,
    /// Default local RTP port to advertise when issuing INVITE/talk SDPs. Overridden per
    /// REST request (`port` field) when present.
    #[serde(default = "default_media_port")]
    pub default_media_port: u16,
}

fn default_listen_udp() -> String {
    "0.0.0.0:5060".to_string()
}

fn default_listen_tcp() -> String {
    "0.0.0.0:5060".to_string()
}

fn default_read_buffer_size() -> usize {
    65536
}

fn default_tick_interval_ms() -> u64 {
    1000
}

fn default_media_port() -> u16 {
    30000
}

impl Default for Gb28181ModuleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen_udp: default_listen_udp(),
            listen_tcp: default_listen_tcp(),
            read_buffer_size: default_read_buffer_size(),
            tick_interval_ms: default_tick_interval_ms(),
            public_ip: String::new(),
            default_media_port: default_media_port(),
        }
    }
}

impl Gb28181ModuleConfig {
    /// `default_json` function.
    /// `default_json` 函数.
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap_or_default()
    }

    /// Creates `value` from input.
    /// 创建 `值` 来自 输入.
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// `validate` function.
    /// `validate` 函数.
    pub fn validate(&self) -> Result<(), String> {
        if self.listen_udp.parse::<std::net::SocketAddr>().is_err() {
            return Err(format!("invalid listen_udp: {}", self.listen_udp));
        }
        if self.listen_tcp.parse::<std::net::SocketAddr>().is_err() {
            return Err(format!("invalid listen_tcp: {}", self.listen_tcp));
        }
        Ok(())
    }
}
