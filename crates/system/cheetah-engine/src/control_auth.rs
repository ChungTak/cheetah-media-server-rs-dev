use std::collections::HashMap;

use cheetah_media_api::auth::{AuthCredentials, MediaScope, Principal};
use cheetah_media_api::error::{MediaError, MediaErrorCode, Result};
use cheetah_media_api::port::ControlAuthApi;
use cheetah_sdk::ConfigProvider;

/// Default `ControlAuthApi` implementation backed by the global config.
///
/// Tokens are read from `media.native.tokens` as a map of bearer token to a
/// principal description:
///
/// ```json
/// {
///   "media": {
///     "native": {
///       "tokens": {
///         "admin-secret": { "principal": "admin", "scopes": ["server.admin"] }
///       }
///     }
///   }
/// }
/// ```
///
/// Requests without credentials are authenticated as `anonymous` with only
/// `media.read` scope, so anonymous callers can query but cannot perform
/// high-risk operations.
///
/// mTLS identity is only trusted when `media.native.trust_mtls_identity` is
/// `true`. This should only be enabled when a reverse proxy terminates TLS and
/// strips client-provided `x-mtls-identity` headers before setting the value from
/// the validated peer certificate.
pub struct ConfigControlAuth {
    config: std::sync::Arc<dyn ConfigProvider>,
}

impl ConfigControlAuth {
    pub fn new(config: std::sync::Arc<dyn ConfigProvider>) -> Self {
        Self { config }
    }

    fn token_map(&self) -> HashMap<String, (String, Vec<MediaScope>)> {
        let mut map = HashMap::new();
        let global = self.config.global();
        let Some(tokens) = global
            .get("media")
            .and_then(|v| v.get("native"))
            .and_then(|v| v.get("tokens"))
            .and_then(|v| v.as_object())
        else {
            return map;
        };

        for (token, value) in tokens {
            let Some(obj) = value.as_object() else {
                continue;
            };
            let principal = obj
                .get("principal")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let scopes = obj
                .get("scopes")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().and_then(|s| s.parse::<MediaScope>().ok()))
                        .collect()
                })
                .unwrap_or_default();
            map.insert(token.clone(), (principal, scopes));
        }
        map
    }

    fn trust_mtls_identity(&self) -> bool {
        self.config
            .global()
            .get("media")
            .and_then(|v| v.get("native"))
            .and_then(|v| v.get("trust_mtls_identity"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    fn bearer_token(&self, header: Option<&str>) -> Option<String> {
        let header = header?;
        let header = header.trim();
        let prefix = "bearer ";
        if header.len() > prefix.len() && header[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return Some(header[prefix.len()..].trim().to_string());
        }
        None
    }
}

impl ControlAuthApi for ConfigControlAuth {
    fn authenticate(&self, credentials: &AuthCredentials) -> Result<Principal> {
        // Bearer token from the Authorization header takes precedence.
        if let Some(token) = credentials
            .authorization_header
            .as_deref()
            .and_then(|h| self.bearer_token(Some(h)))
        {
            let map = self.token_map();
            if let Some((identity, scopes)) = map.get(&token) {
                return Ok(Principal {
                    identity: identity.clone(),
                    scopes: scopes.clone(),
                });
            }
            return Err(MediaError::new(
                MediaErrorCode::Unauthenticated,
                "invalid bearer token",
            ));
        }

        // mTLS identity is only trusted when the deployment explicitly enables
        // media.native.trust_mtls_identity, e.g. when a reverse proxy terminates
        // TLS and strips any client-provided x-mtls-identity header before setting
        // it from the peer certificate. Otherwise a client could spoof the header.
        if self.trust_mtls_identity() {
            if let Some(identity) = credentials.mtls_identity.as_deref() {
                return Ok(Principal {
                    identity: identity.to_string(),
                    scopes: vec![MediaScope::MediaRead],
                });
            }
        }

        // Anonymous callers get read-only access by default. High-risk operations
        // require an explicit scope from a token or deployment credential.
        Ok(Principal::anonymous())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_sdk::{ConfigProvider, ModuleId};
    use serde_json::json;

    #[derive(Default)]
    struct TestConfig(serde_json::Value);

    impl ConfigProvider for TestConfig {
        fn global(&self) -> serde_json::Value {
            self.0.clone()
        }
        fn module(&self, _module_id: &ModuleId) -> serde_json::Value {
            serde_json::Value::Null
        }
        fn version(&self) -> u64 {
            0
        }
    }

    #[test]
    fn anonymous_principal_has_read_scope() {
        let auth = ConfigControlAuth::new(std::sync::Arc::new(TestConfig::default()));
        let p = auth.authenticate(&AuthCredentials::default()).unwrap();
        assert_eq!(p.identity, "anonymous");
        assert!(p.has_scope(&MediaScope::MediaRead));
        assert!(!p.has_scope(&MediaScope::MediaControl));
    }

    #[test]
    fn bearer_token_maps_to_configured_principal() {
        let config = TestConfig(json!({
            "media": {
                "native": {
                    "tokens": {
                        "secret": { "principal": "admin", "scopes": ["media.control", "record.manage"] }
                    }
                }
            }
        }));
        let auth = ConfigControlAuth::new(std::sync::Arc::new(config));
        let p = auth
            .authenticate(&AuthCredentials {
                authorization_header: Some("Bearer secret".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p.identity, "admin");
        assert!(p.has_scope(&MediaScope::MediaControl));
        assert!(p.has_scope(&MediaScope::RecordManage));
    }

    #[test]
    fn bearer_token_scheme_is_case_insensitive() {
        let config = TestConfig(json!({
            "media": {
                "native": {
                    "tokens": {
                        "secret": { "principal": "admin", "scopes": ["media.read"] }
                    }
                }
            }
        }));
        let auth = ConfigControlAuth::new(std::sync::Arc::new(config));
        for scheme in ["Bearer secret", "bearer secret", "BEARER secret"] {
            let p = auth
                .authenticate(&AuthCredentials {
                    authorization_header: Some(scheme.to_string()),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(p.identity, "admin", "failed for {scheme}");
        }
    }

    #[test]
    fn mtls_identity_is_ignored_by_default() {
        let config = TestConfig(json!({ "media": { "native": {} } }));
        let auth = ConfigControlAuth::new(std::sync::Arc::new(config));
        let p = auth
            .authenticate(&AuthCredentials {
                mtls_identity: Some("alice".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p.identity, "anonymous");
        assert!(p.has_scope(&MediaScope::MediaRead));
    }

    #[test]
    fn mtls_identity_is_trusted_when_enabled() {
        let config = TestConfig(json!({
            "media": {
                "native": {
                    "trust_mtls_identity": true
                }
            }
        }));
        let auth = ConfigControlAuth::new(std::sync::Arc::new(config));
        let p = auth
            .authenticate(&AuthCredentials {
                mtls_identity: Some("alice".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p.identity, "alice");
        assert!(p.has_scope(&MediaScope::MediaRead));
    }

    #[test]
    fn invalid_bearer_token_is_unauthenticated() {
        let config = TestConfig(json!({
            "media": { "native": { "tokens": {} } }
        }));
        let auth = ConfigControlAuth::new(std::sync::Arc::new(config));
        let err = auth
            .authenticate(&AuthCredentials {
                authorization_header: Some("Bearer wrong".to_string()),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.code, MediaErrorCode::Unauthenticated);
    }
}
