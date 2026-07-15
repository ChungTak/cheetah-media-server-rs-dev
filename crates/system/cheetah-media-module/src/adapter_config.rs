use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AuthConfig {
    #[serde(default = "default_auth_mode")]
    pub mode: String,
    #[serde(default)]
    pub session: Option<SessionAuthConfig>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: default_auth_mode(),
            session: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SessionAuthConfig {
    pub username: String,
    pub password: String,
    #[serde(default = "default_cookie_name")]
    pub cookie_name: String,
    #[serde(default = "default_session_ttl_sec")]
    pub session_ttl_sec: u64,
    #[serde(default)]
    pub max_sessions: Option<usize>,
}

fn default_cookie_name() -> String {
    "zlm_session".to_string()
}

fn default_session_ttl_sec() -> u64 {
    3600
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct NativeAdapterConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_native_prefix")]
    pub path_prefix: String,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
}

impl Default for NativeAdapterConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            path_prefix: default_native_prefix(),
            auth: AuthConfig::default(),
            request_timeout_ms: default_request_timeout_ms(),
            max_body_bytes: default_max_body_bytes(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ZlmAdapterConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_zlm_prefix")]
    pub path_prefix: String,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    pub secret: Option<String>,
    #[serde(default)]
    pub legacy_http_200: bool,
    #[serde(default)]
    pub strict_fields: bool,
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for ZlmAdapterConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            path_prefix: default_zlm_prefix(),
            auth: AuthConfig::default(),
            request_timeout_ms: default_request_timeout_ms(),
            max_body_bytes: default_max_body_bytes(),
            secret: None,
            legacy_http_200: false,
            strict_fields: false,
            extra: HashMap::new(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_native_prefix() -> String {
    "/api/v1".to_string()
}

fn default_zlm_prefix() -> String {
    "/index".to_string()
}

fn default_auth_mode() -> String {
    "token".to_string()
}

fn default_request_timeout_ms() -> u64 {
    30_000
}

fn default_max_body_bytes() -> usize {
    8 * 1024 * 1024
}

pub fn load_native_config(global: &serde_json::Value) -> NativeAdapterConfig {
    let value = global
        .get("media")
        .and_then(|m| m.get("native"))
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    serde_json::from_value(value).unwrap_or_default()
}

pub fn load_zlm_config(global: &serde_json::Value) -> ZlmAdapterConfig {
    let value = global
        .get("media")
        .and_then(|m| m.get("zlm"))
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    serde_json::from_value(value).unwrap_or_default()
}

/// Extract the media-native configuration object from a raw JSON value.
/// Tries `media.native` first, then treats the value itself as the config.
pub fn extract_native_config(value: &serde_json::Value) -> NativeAdapterConfig {
    let candidate = value
        .get("media")
        .and_then(|m| m.get("native"))
        .or(Some(value));
    candidate
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Extract the media-zlm configuration object from a raw JSON value.
pub fn extract_zlm_config(value: &serde_json::Value) -> ZlmAdapterConfig {
    let candidate = value
        .get("media")
        .and_then(|m| m.get("zlm"))
        .or(Some(value));
    candidate
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_config_defaults() {
        let cfg = load_native_config(&json!({}));
        assert!(cfg.enabled);
        assert_eq!(cfg.path_prefix, "/api/v1");
        assert_eq!(cfg.auth.mode, "token");
        assert_eq!(cfg.request_timeout_ms, 30_000);
        assert_eq!(cfg.max_body_bytes, 8 * 1024 * 1024);
    }

    #[test]
    fn native_config_parses_custom_values() {
        let cfg = load_native_config(&json!({
            "media": {
                "native": {
                    "enabled": false,
                    "path_prefix": "/custom",
                    "auth": { "mode": "none" },
                    "request_timeout_ms": 5000,
                    "max_body_bytes": 1024
                }
            }
        }));
        assert!(!cfg.enabled);
        assert_eq!(cfg.path_prefix, "/custom");
        assert_eq!(cfg.auth.mode, "none");
        assert_eq!(cfg.request_timeout_ms, 5000);
        assert_eq!(cfg.max_body_bytes, 1024);
    }

    #[test]
    fn zlm_secret_and_flags_parsed() {
        let cfg = load_zlm_config(&json!({
            "media": {
                "zlm": {
                    "secret": "zlm-secret",
                    "legacy_http_200": true,
                    "strict_fields": true
                }
            }
        }));
        assert_eq!(cfg.secret, Some("zlm-secret".to_string()));
        assert!(cfg.legacy_http_200);
        assert!(cfg.strict_fields);
    }
}
