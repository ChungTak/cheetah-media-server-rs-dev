//! GB28181 module configuration.
//!
//! GB28181 模块配置。

use serde::{Deserialize, Serialize};

/// Configuration for the GB28181 module.
///
/// GB28181 模块配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gb28181ModuleConfig {
    pub enabled: bool,
    #[serde(default = "default_listen_udp")]
    pub listen_udp: String,
    #[serde(default = "default_listen_tcp")]
    pub listen_tcp: String,
    #[serde(default = "default_read_buffer_size")]
    pub read_buffer_size: usize,
    #[serde(default = "default_tick_interval_ms")]
    pub tick_interval_ms: u64,
    /// Local IP advertised in SDP `c=IN IP4 ...` and `m=` lines, plus SIP `Contact`/`Via`.
    /// If empty, falls back to the listen UDP address.
    ///
    /// 在 SDP `c=IN IP4 ...` 与 `m=` 行及 SIP `Contact`/`Via` 中宣告的本地 IP。为空时回退到 UDP 监听地址。
    #[serde(default)]
    pub public_ip: String,
    /// Default local RTP port to advertise when issuing INVITE/talk SDPs. Overridden per
    /// REST request (`port` field) when present.
    ///
    /// 发起 INVITE/talk SDP 时宣告的默认本地 RTP 端口。REST 请求中的 `port` 字段可覆盖。
    #[serde(default = "default_media_port")]
    pub default_media_port: u16,
}

/// Default UDP listen address.
///
/// 默认 UDP 监听地址。
fn default_listen_udp() -> String {
    "0.0.0.0:5060".to_string()
}

/// Default TCP listen address.
///
/// 默认 TCP 监听地址。
fn default_listen_tcp() -> String {
    "0.0.0.0:5060".to_string()
}

/// Default socket read buffer size.
///
/// 默认套接字读缓冲区大小。
fn default_read_buffer_size() -> usize {
    65536
}

/// Default tick interval for the driver state machine.
///
/// 驱动状态机默认 tick 间隔（毫秒）。
fn default_tick_interval_ms() -> u64 {
    1000
}

/// Default local media port for SDP.
///
/// SDP 默认本地媒体端口。
fn default_media_port() -> u16 {
    30000
}

/// Default values for `Gb28181ModuleConfig`.
///
/// `Gb28181ModuleConfig` 默认值。
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

/// `Gb28181ModuleConfig` serialization helpers.
///
/// `Gb28181ModuleConfig` 序列化辅助。
impl Gb28181ModuleConfig {
    /// Return the default config as a JSON value.
    ///
    /// 以 JSON 值返回默认配置。
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).unwrap_or_default()
    }

    /// Deserialize from a JSON value.
    ///
    /// 从 JSON 值反序列化。
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// Validate the listen addresses.
    ///
    /// 校验监听地址。
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
