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
}

impl Default for ProxyModuleConfig {
    fn default() -> Self {
        Self {
            max_proxies: default_max_proxies(),
            retry_max: default_retry_max(),
            retry_delay_ms: default_retry_delay_ms(),
            retry_max_delay_ms: default_retry_max_delay_ms(),
            connect_timeout_ms: default_connect_timeout_ms(),
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
        serde_json::to_value(Self::default()).expect("default config serializes")
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
