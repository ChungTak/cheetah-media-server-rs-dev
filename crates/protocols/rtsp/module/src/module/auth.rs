use super::*;
use cheetah_rtsp_core::{
    parse_authorization_header, verify_digest_response, RtspAuthorization, RtspDigestChallenge,
};
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// `request_requires_auth` function.
/// `request_requires_auth` 函数.
pub(super) fn request_requires_auth(method: &RtspMethod, config: &RtspModuleConfig) -> bool {
    if !config.auth.enabled {
        return false;
    }
    match method {
        RtspMethod::Describe | RtspMethod::Setup | RtspMethod::Play => true,
        RtspMethod::Announce | RtspMethod::Record => config.auth.require_publish_auth,
        _ => false,
    }
}

/// `AuthError` enumeration.
/// `AuthError` 枚举.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AuthError {
    /// Authentication failed — send 401 without stale hint.
    Rejected(&'static str),
    /// Nonce expired but credentials may be valid — send 401 with stale=true.
    StaleNonce,
}

/// `check_request_auth` function.
/// `check_request_auth` 函数.
pub(super) fn check_request_auth(
    connection_id: RtspConnectionId,
    req: &RtspRequest,
    config: &RtspModuleConfig,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    now_unix_micros: u64,
) -> Result<(), AuthError> {
    if !request_requires_auth_for_connection(connection_id, &req.method, config, sessions) {
        return Ok(());
    }

    let Some(auth_header) = req.header_value("authorization") else {
        return Err(AuthError::Rejected("missing Authorization header"));
    };
    let parsed = parse_authorization_header(auth_header)
        .map_err(|_| AuthError::Rejected("invalid Authorization header"))?;

    match parsed {
        RtspAuthorization::Basic { username, password } => {
            if !config.auth.allow_basic {
                return Err(AuthError::Rejected("Basic auth is disabled"));
            }
            if validate_user_password(config, &username, &password) {
                Ok(())
            } else {
                Err(AuthError::Rejected("invalid credentials"))
            }
        }
        RtspAuthorization::Digest(digest) => {
            if !config.auth.allow_digest {
                return Err(AuthError::Rejected("Digest auth is disabled"));
            }
            if !digest.uri.eq(&req.uri) {
                return Err(AuthError::Rejected("digest uri mismatch"));
            }
            let Some(password) = lookup_password(config, &digest.username) else {
                return Err(AuthError::Rejected("invalid credentials"));
            };
            let (nonce, issued_at_micros, last_nc) = {
                let guard = sessions.lock();
                let Some(state) = guard.get(&connection_id) else {
                    return Err(AuthError::Rejected("session state not found"));
                };
                let Some(nonce) = state.auth_digest_nonce.clone() else {
                    return Err(AuthError::Rejected("digest nonce not issued"));
                };
                let Some(issued_at_micros) = state.auth_digest_nonce_issued_at_micros else {
                    return Err(AuthError::Rejected("digest nonce timestamp missing"));
                };
                (nonce, issued_at_micros, state.auth_digest_nc_last)
            };
            // Check nonce TTL
            let age_micros = now_unix_micros.saturating_sub(issued_at_micros);
            let ttl_micros = u64::from(config.auth.nonce_ttl_secs).saturating_mul(1_000_000u64);
            if age_micros > ttl_micros {
                return Err(AuthError::StaleNonce);
            }
            // Validate nc is monotonically increasing (anti-replay)
            if let Some(nc_str) = digest.nc.as_deref() {
                let nc_value = u32::from_str_radix(nc_str, 16)
                    .map_err(|_| AuthError::Rejected("invalid nc value"))?;
                if nc_value <= last_nc {
                    return Err(AuthError::Rejected("nc replay detected"));
                }
            }
            let challenge = RtspDigestChallenge {
                realm: config.auth.realm.clone(),
                nonce,
                algorithm: digest.algorithm,
                stale: false,
            };
            if verify_digest_response(&digest, &challenge, &req.method, password) {
                // Update nc tracking on success
                if let Some(nc_str) = digest.nc.as_deref() {
                    if let Ok(nc_value) = u32::from_str_radix(nc_str, 16) {
                        let mut guard = sessions.lock();
                        if let Some(state) = guard.get_mut(&connection_id) {
                            state.auth_digest_nc_last = nc_value;
                        }
                    }
                }
                Ok(())
            } else {
                Err(AuthError::Rejected("digest response mismatch"))
            }
        }
    }
}

fn request_requires_auth_for_connection(
    connection_id: RtspConnectionId,
    method: &RtspMethod,
    config: &RtspModuleConfig,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
) -> bool {
    if !config.auth.enabled {
        return false;
    }
    if matches!(method, RtspMethod::Setup) {
        let mode = sessions
            .lock()
            .get(&connection_id)
            .and_then(|state| state.mode);
        return match mode {
            Some(SessionMode::Publish) => config.auth.require_publish_auth,
            Some(SessionMode::Play) | None => true,
        };
    }
    request_requires_auth(method, config)
}

/// `issue_digest_nonce` function.
/// `issue_digest_nonce` 函数.
pub(super) fn issue_digest_nonce(
    connection_id: RtspConnectionId,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    now_unix_micros: u64,
) -> String {
    let nonce = build_digest_nonce(connection_id, now_unix_micros);
    let mut guard = sessions.lock();
    if let Some(state) = guard.get_mut(&connection_id) {
        state.auth_digest_nonce = Some(nonce.clone());
        state.auth_digest_nonce_issued_at_micros = Some(now_unix_micros);
    }
    nonce
}

fn build_digest_nonce(connection_id: RtspConnectionId, now_unix_micros: u64) -> String {
    let mut random = [0u8; 16];
    let mut nonce = String::with_capacity(80);
    let counter = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    if getrandom::getrandom(&mut random).is_ok() {
        for byte in random {
            let _ = write!(&mut nonce, "{byte:02x}");
        }
    } else {
        let _ = write!(
            &mut nonce,
            "{connection_id:016x}{now_unix_micros:016x}{counter:016x}"
        );
    }
    let _ = write!(
        &mut nonce,
        "{connection_id:016x}{now_unix_micros:016x}{counter:016x}"
    );
    nonce
}

/// Builds `www_authenticate_headers` output.
/// 构建 `www_authenticate_headers` 输出.
pub(super) fn build_www_authenticate_headers(
    config: &RtspModuleConfig,
    digest_nonce: Option<&str>,
) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    if config.auth.allow_digest {
        if let Some(nonce) = digest_nonce {
            // Prefer SHA-256, also offer MD5 for backward compatibility
            headers.push((
                "WWW-Authenticate".to_string(),
                format!(
                    "Digest realm=\"{}\", nonce=\"{}\", algorithm=SHA-256",
                    config.auth.realm, nonce
                ),
            ));
            headers.push((
                "WWW-Authenticate".to_string(),
                format!(
                    "Digest realm=\"{}\", nonce=\"{}\", algorithm=MD5",
                    config.auth.realm, nonce
                ),
            ));
        }
    }
    if config.auth.allow_basic {
        headers.push((
            "WWW-Authenticate".to_string(),
            format!("Basic realm=\"{}\"", config.auth.realm),
        ));
    }
    headers
}

fn validate_user_password(config: &RtspModuleConfig, username: &str, password: &str) -> bool {
    config
        .auth
        .users
        .iter()
        .any(|user| user.username == username && user.password == password)
}

fn lookup_password<'a>(config: &'a RtspModuleConfig, username: &str) -> Option<&'a str> {
    config
        .auth
        .users
        .iter()
        .find(|user| user.username == username)
        .map(|user| user.password.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_rtsp_driver_tokio::RtspHeader;

    fn md5_hex(value: &str) -> String {
        format!("{:x}", md5::compute(value.as_bytes()))
    }

    fn build_digest_response(
        username: &str,
        realm: &str,
        password: &str,
        nonce: &str,
        method: &str,
        uri: &str,
    ) -> String {
        let ha1 = md5_hex(&format!("{username}:{realm}:{password}"));
        let ha2 = md5_hex(&format!("{method}:{uri}"));
        md5_hex(&format!("{ha1}:{nonce}:{ha2}"))
    }

    fn test_config() -> RtspModuleConfig {
        RtspModuleConfig {
            auth: crate::config::RtspAuthConfig {
                enabled: true,
                require_publish_auth: false,
                realm: "cheetah".to_string(),
                users: vec![crate::config::RtspAuthUserConfig {
                    username: "user".to_string(),
                    password: "pass".to_string(),
                }],
                allow_basic: true,
                allow_digest: true,
                nonce_ttl_secs: 60,
            },
            ..RtspModuleConfig::default()
        }
    }

    fn sessions_with_connection(
        connection_id: RtspConnectionId,
    ) -> Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>> {
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        sessions
            .lock()
            .insert(connection_id, RtspConnectionState::new(connection_id));
        sessions
    }

    fn request(method: RtspMethod, uri: &str, authorization: Option<&str>) -> RtspRequest {
        let mut headers = Vec::new();
        if let Some(value) = authorization {
            headers.push(RtspHeader {
                name: "Authorization".to_string(),
                value: value.to_string(),
            });
        }
        RtspRequest {
            method,
            uri: uri.to_string(),
            version: "RTSP/1.0".to_string(),
            headers,
            body: Bytes::new(),
            cseq: Some(1),
            session: None,
        }
    }

    #[test]
    fn basic_auth_success_and_failure() {
        let cfg = test_config();
        let sessions = sessions_with_connection(1);
        let ok_req = request(
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            Some("Basic dXNlcjpwYXNz"),
        );
        check_request_auth(1, &ok_req, &cfg, &sessions, 1).expect("basic ok");

        let bad_req = request(
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            Some("Basic dXNlcjp3cm9uZw=="),
        );
        assert!(check_request_auth(1, &bad_req, &cfg, &sessions, 1).is_err());
    }

    #[test]
    fn digest_auth_success_and_nonce_mismatch() {
        let cfg = test_config();
        let sessions = sessions_with_connection(2);
        let nonce = issue_digest_nonce(2, &sessions, 10);
        let uri = "rtsp://127.0.0.1/live/test";
        let response = build_digest_response("user", "cheetah", "pass", &nonce, "DESCRIBE", uri);
        let good_header = format!(
            "Digest username=\"user\", realm=\"cheetah\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\", algorithm=MD5"
        );
        let req = request(RtspMethod::Describe, uri, Some(&good_header));
        check_request_auth(2, &req, &cfg, &sessions, 11).expect("digest ok");

        let bad_header = good_header.replace(&nonce, "badnonce");
        let req = request(RtspMethod::Describe, uri, Some(&bad_header));
        assert!(check_request_auth(2, &req, &cfg, &sessions, 11).is_err());
    }

    #[test]
    fn publish_auth_disabled_by_default_for_announce_record() {
        let cfg = test_config();
        let sessions = sessions_with_connection(3);
        let announce = request(RtspMethod::Announce, "rtsp://127.0.0.1/live/test", None);
        let record = request(RtspMethod::Record, "rtsp://127.0.0.1/live/test", None);
        check_request_auth(3, &announce, &cfg, &sessions, 1).expect("announce should pass");
        check_request_auth(3, &record, &cfg, &sessions, 1).expect("record should pass");
    }

    #[test]
    fn setup_requires_play_auth_before_allocating_transport_resources() {
        let cfg = test_config();
        assert!(request_requires_auth(&RtspMethod::Setup, &cfg));
    }

    #[test]
    fn setup_honors_publish_auth_disabled_after_announce() {
        let cfg = test_config();
        let sessions = sessions_with_connection(5);
        sessions.lock().get_mut(&5).expect("state").mode = Some(SessionMode::Publish);
        let setup = request(
            RtspMethod::Setup,
            "rtsp://127.0.0.1/live/test/trackID=0",
            None,
        );
        check_request_auth(5, &setup, &cfg, &sessions, 1)
            .expect("publish SETUP should pass when publish auth is disabled");
    }

    #[test]
    fn auth_failure_does_not_create_publish_session_state() {
        let mut cfg = test_config();
        cfg.auth.require_publish_auth = true;
        let sessions = sessions_with_connection(4);
        let announce = request(RtspMethod::Announce, "rtsp://127.0.0.1/live/test", None);
        let err = check_request_auth(4, &announce, &cfg, &sessions, 1).expect_err("must fail");
        assert_eq!(err, AuthError::Rejected("missing Authorization header"));
        let publish_exists = sessions
            .lock()
            .get(&4)
            .and_then(|state| state.publish.as_ref())
            .is_some();
        assert!(!publish_exists);
    }

    #[test]
    fn issued_digest_nonce_is_not_deterministic_for_same_inputs() {
        let sessions = sessions_with_connection(9);
        let nonce1 = issue_digest_nonce(9, &sessions, 123_456);
        let nonce2 = issue_digest_nonce(9, &sessions, 123_456);
        assert_ne!(nonce1, nonce2, "nonce should include unpredictable entropy");
    }

    #[test]
    fn stale_nonce_returns_stale_error() {
        let mut cfg = test_config();
        cfg.auth.nonce_ttl_secs = 1; // 1 second TTL
        let sessions = sessions_with_connection(10);
        let nonce = issue_digest_nonce(10, &sessions, 1_000_000); // issued at 1s
        let uri = "rtsp://127.0.0.1/live/test";
        let response = build_digest_response("user", "cheetah", "pass", &nonce, "DESCRIBE", uri);
        let header = format!(
            "Digest username=\"user\", realm=\"cheetah\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\", algorithm=MD5"
        );
        let req = request(RtspMethod::Describe, uri, Some(&header));
        // 3 seconds later — nonce expired (TTL is 1s)
        let err = check_request_auth(10, &req, &cfg, &sessions, 3_000_001).expect_err("stale");
        assert_eq!(err, AuthError::StaleNonce);
    }

    #[test]
    fn nc_replay_is_rejected() {
        let cfg = test_config();
        let sessions = sessions_with_connection(11);
        let nonce = issue_digest_nonce(11, &sessions, 10);
        let uri = "rtsp://127.0.0.1/live/test";
        let nc = "00000001";
        let cnonce = "cafebabe";
        let ha1 = md5_hex("user:cheetah:pass");
        let ha2 = md5_hex(&format!("DESCRIBE:{uri}"));
        let response = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"));
        let header = format!(
            "Digest username=\"user\", realm=\"cheetah\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\", algorithm=MD5, qop=auth, nc={nc}, cnonce=\"{cnonce}\""
        );
        let req = request(RtspMethod::Describe, uri, Some(&header));
        check_request_auth(11, &req, &cfg, &sessions, 11).expect("first request ok");

        // Replay same nc=00000001 — should be rejected
        let req2 = request(RtspMethod::Describe, uri, Some(&header));
        let err = check_request_auth(11, &req2, &cfg, &sessions, 12).expect_err("replay");
        assert_eq!(err, AuthError::Rejected("nc replay detected"));
    }

    #[test]
    fn nc_monotonic_increase_accepted() {
        let cfg = test_config();
        let sessions = sessions_with_connection(12);
        let nonce = issue_digest_nonce(12, &sessions, 10);
        let uri = "rtsp://127.0.0.1/live/test";
        let ha1 = md5_hex("user:cheetah:pass");
        let ha2 = md5_hex(&format!("DESCRIBE:{uri}"));

        // nc=1
        let response1 = md5_hex(&format!("{ha1}:{nonce}:00000001:c1:auth:{ha2}"));
        let header1 = format!(
            "Digest username=\"user\", realm=\"cheetah\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response1}\", algorithm=MD5, qop=auth, nc=00000001, cnonce=\"c1\""
        );
        let req1 = request(RtspMethod::Describe, uri, Some(&header1));
        check_request_auth(12, &req1, &cfg, &sessions, 11).expect("nc=1 ok");

        // nc=2
        let response2 = md5_hex(&format!("{ha1}:{nonce}:00000002:c2:auth:{ha2}"));
        let header2 = format!(
            "Digest username=\"user\", realm=\"cheetah\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response2}\", algorithm=MD5, qop=auth, nc=00000002, cnonce=\"c2\""
        );
        let req2 = request(RtspMethod::Describe, uri, Some(&header2));
        check_request_auth(12, &req2, &cfg, &sessions, 12).expect("nc=2 ok");
    }
}
