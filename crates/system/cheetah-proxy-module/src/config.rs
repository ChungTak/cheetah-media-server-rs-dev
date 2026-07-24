use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Configuration for the proxy module.
///
/// 代理模块配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyModuleConfig {
    #[serde(default = "default_max_proxies")]
    pub max_proxies: usize,
    #[serde(default = "default_retry_max")]
    pub retry_max: u32,
    #[serde(default = "default_retry_delay_ms")]
    pub retry_delay_ms: u64,
    #[serde(default = "default_retry_max_delay_ms")]
    pub retry_max_delay_ms: u64,
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    /// CIDR allowlist for proxy source / destination hosts.
    ///
    /// Entries such as `127.0.0.0/8` or `::1/128` bypass the default SSRF
    /// denial of loopback / private / link-local addresses. DNS names are
    /// resolved to all A/AAAA records and each address must fall inside an
    /// allowed CIDR before the request proceeds.
    ///
    /// 代理源/目标主机的 CIDR 白名单。`127.0.0.0/8` 或 `::1/128` 等可绕过默认
    /// 的 SSRF 拒绝。DNS 名称会解析为所有 A/AAAA 地址，每个地址都必须在白名单内。
    #[serde(default = "default_ssrf_allowlist_cidrs")]
    pub ssrf_allowlist_cidrs: Vec<String>,
}

impl Default for ProxyModuleConfig {
    fn default() -> Self {
        Self {
            max_proxies: default_max_proxies(),
            retry_max: default_retry_max(),
            retry_delay_ms: default_retry_delay_ms(),
            retry_max_delay_ms: default_retry_max_delay_ms(),
            connect_timeout_ms: default_connect_timeout_ms(),
            ssrf_allowlist_cidrs: default_ssrf_allowlist_cidrs(),
        }
    }
}

impl ProxyModuleConfig {
    /// Build a config from a JSON value, defaulting to an empty config.
    ///
    /// 从 JSON 值构造配置；传入 null 时使用默认配置。
    pub fn from_value(value: Value) -> Result<Self, serde_json::Error> {
        if value.is_null() {
            return Ok(Self::default());
        }
        serde_json::from_value(value)
    }

    /// Return the default configuration as a JSON value.
    ///
    /// 以 JSON 值形式返回默认配置。
    pub fn default_json() -> Value {
        serde_json::to_value(Self::default()).unwrap_or_default()
    }

    /// Validate the loaded configuration.
    ///
    /// 校验加载后的配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.max_proxies == 0 {
            return Err("max_proxies must be > 0".into());
        }
        if self.retry_max_delay_ms < self.retry_delay_ms {
            return Err("retry_max_delay_ms must be >= retry_delay_ms".into());
        }
        if self.connect_timeout_ms == 0 {
            return Err("connect_timeout_ms must be > 0".into());
        }
        if let Err(e) = crate::ssrf::parse_allowlist(&self.ssrf_allowlist_cidrs) {
            return Err(format!("invalid ssrf_allowlist_cidrs: {e}"));
        }
        Ok(())
    }
}

fn default_max_proxies() -> usize {
    256
}

fn default_retry_max() -> u32 {
    3
}

fn default_retry_delay_ms() -> u64 {
    1_000
}

fn default_retry_max_delay_ms() -> u64 {
    30_000
}

fn default_connect_timeout_ms() -> u64 {
    10_000
}

fn default_ssrf_allowlist_cidrs() -> Vec<String> {
    Vec::new()
}
