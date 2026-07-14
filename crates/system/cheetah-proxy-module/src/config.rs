//! Proxy module configuration.
//!
//! 代理模块配置。

use serde::{Deserialize, Serialize};

/// Configuration for the proxy module.
///
/// 代理模块配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyModuleConfig {
    /// Maximum number of proxy entries kept in memory.
    #[serde(default = "default_max_total_proxies")]
    pub max_total_proxies: u32,
}

impl Default for ProxyModuleConfig {
    fn default() -> Self {
        Self {
            max_total_proxies: default_max_total_proxies(),
        }
    }
}

impl ProxyModuleConfig {
    /// Validate the configuration and report the first error found.
    pub fn validate(&self) -> Result<(), String> {
        if self.max_total_proxies == 0 {
            return Err("proxy max_total_proxies must be greater than 0".to_string());
        }
        Ok(())
    }

    /// Parse a JSON value into a configuration.
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// Return the configuration as a JSON value.
    pub fn to_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }

    /// Return the default configuration as a JSON value.
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).expect("default config serializes")
    }
}

fn default_max_total_proxies() -> u32 {
    1_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_passes_validation() {
        assert!(ProxyModuleConfig::default().validate().is_ok());
    }

    #[test]
    fn zero_max_total_is_rejected() {
        let cfg = ProxyModuleConfig {
            max_total_proxies: 0,
        };
        assert!(cfg.validate().is_err());
    }
}
