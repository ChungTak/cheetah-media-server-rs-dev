//! GB28181 module configuration.
//!
//! GB28181 模块配置。

use serde::{Deserialize, Serialize};

/// Who owns the GB28181 control plane.
///
/// 谁负责 GB28181 控制面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlOwner {
    /// Media process exposes the local REST media API.
    #[default]
    Local,
    /// The cluster signaling control plane owns GB control; the media process only
    /// accepts calls through the internal `RtpSessionApi`.
    Signaling,
}

/// Configuration for the GB28181 media module.
///
/// GB28181 媒体模块配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gb28181ModuleConfig {
    pub enabled: bool,
    /// Default local RTP port to advertise for passive receive/talk when the REST request
    /// omits the `port` field.
    ///
    /// 当 REST 请求缺少 `port` 字段时，被动接收/对讲使用的默认本地 RTP 端口。
    #[serde(default = "default_media_port")]
    pub default_media_port: u16,
    /// Who owns the GB28181 control plane.
    /// `local` exposes the REST media endpoints.
    /// `signaling` disables those endpoints; the signaling control plane drives sessions
    /// through `RtpSessionApi`.
    ///
    /// 谁拥有 GB28181 控制面。
    #[serde(default)]
    pub control_owner: ControlOwner,
}

/// Default local media port.
///
/// 默认本地媒体端口。
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
            default_media_port: default_media_port(),
            control_owner: ControlOwner::Local,
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

    /// Validate the media-only configuration.
    ///
    /// 校验仅媒体相关的配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.default_media_port == 0 {
            return Err("invalid default_media_port: must be non-zero".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_control_owner_is_local() {
        let cfg = Gb28181ModuleConfig::default();
        assert_eq!(cfg.control_owner, ControlOwner::Local);
    }

    #[test]
    fn control_owner_deserializes_from_snake_case() {
        let json = serde_json::json!({
            "enabled": true,
            "control_owner": "signaling"
        });
        let cfg: Gb28181ModuleConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.control_owner, ControlOwner::Signaling);
    }
}
