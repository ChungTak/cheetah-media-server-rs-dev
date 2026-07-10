use base64::Engine;
use sha2::{Digest, Sha256};

use super::method::RtspMethod;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtspAuthorization {
    Basic { username: String, password: String },
    Digest(RtspDigestAuthorization),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspDigestAuthorization {
    pub username: String,
    pub realm: String,
    pub nonce: String,
    pub uri: String,
    pub response: String,
    pub algorithm: RtspDigestAlgorithm,
    pub qop: Option<String>,
    pub nc: Option<String>,
    pub cnonce: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspDigestChallenge {
    pub realm: String,
    pub nonce: String,
    pub algorithm: RtspDigestAlgorithm,
    /// When true, the nonce is stale but credentials are valid.
    /// Client should retry with the new nonce without re-prompting.
    pub stale: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtspDigestAlgorithm {
    Md5,
    Sha256,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RtspAuthorizationError {
    #[error("unsupported authorization scheme")]
    UnsupportedScheme,
    #[error("invalid basic authorization payload")]
    InvalidBasicPayload,
    #[error("basic authorization payload is not valid utf-8")]
    InvalidBasicUtf8,
    #[error("invalid digest authorization parameter: {0}")]
    InvalidDigestParameter(String),
}

pub fn parse_authorization_header(
    value: &str,
) -> Result<RtspAuthorization, RtspAuthorizationError> {
    let trimmed = value.trim();
    if let Some(payload) = trim_scheme_prefix(trimmed, "Basic") {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(|_| RtspAuthorizationError::InvalidBasicPayload)?;
        let decoded =
            String::from_utf8(decoded).map_err(|_| RtspAuthorizationError::InvalidBasicUtf8)?;
        let (username, password) = decoded
            .split_once(':')
            .ok_or(RtspAuthorizationError::InvalidBasicPayload)?;
        return Ok(RtspAuthorization::Basic {
            username: username.to_string(),
            password: password.to_string(),
        });
    }

    if let Some(payload) = trim_scheme_prefix(trimmed, "Digest") {
        let params = parse_digest_params(payload)?;
        let username = required_digest_param(&params, "username")?.to_string();
        let realm = required_digest_param(&params, "realm")?.to_string();
        let nonce = required_digest_param(&params, "nonce")?.to_string();
        let uri = required_digest_param(&params, "uri")?.to_string();
        let response = required_digest_param(&params, "response")?.to_string();
        let algorithm = params
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("algorithm"))
            .map(|(_, value)| parse_digest_algorithm(value))
            .unwrap_or(RtspDigestAlgorithm::Md5);
        let qop = params
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("qop"))
            .map(|(_, value)| value.clone());
        let nc = params
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("nc"))
            .map(|(_, value)| value.clone());
        let cnonce = params
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("cnonce"))
            .map(|(_, value)| value.clone());
        return Ok(RtspAuthorization::Digest(RtspDigestAuthorization {
            username,
            realm,
            nonce,
            uri,
            response,
            algorithm,
            qop,
            nc,
            cnonce,
        }));
    }

    Err(RtspAuthorizationError::UnsupportedScheme)
}

pub fn verify_digest_response(
    auth: &RtspDigestAuthorization,
    challenge: &RtspDigestChallenge,
    method: &RtspMethod,
    password: &str,
) -> bool {
    if auth.algorithm != challenge.algorithm {
        return false;
    }
    if auth.realm != challenge.realm || auth.nonce != challenge.nonce {
        return false;
    }
    let ha1 = digest_hex(
        auth.algorithm,
        &format!("{}:{}:{password}", auth.username, challenge.realm),
    );
    let ha2 = digest_hex(auth.algorithm, &format!("{}:{}", method.as_str(), auth.uri));
    let expected = match auth.qop.as_deref() {
        Some(qop) => {
            if !qop.eq_ignore_ascii_case("auth") {
                return false;
            }
            let Some(nc) = auth.nc.as_deref() else {
                return false;
            };
            let Some(cnonce) = auth.cnonce.as_deref() else {
                return false;
            };
            digest_hex(
                auth.algorithm,
                &format!("{ha1}:{}:{nc}:{cnonce}:{qop}:{ha2}", challenge.nonce),
            )
        }
        None => digest_hex(auth.algorithm, &format!("{ha1}:{}:{ha2}", challenge.nonce)),
    };
    auth.response.eq_ignore_ascii_case(&expected)
}

/// Compute a digest response for client-side authentication.
#[allow(clippy::too_many_arguments)]
pub fn compute_digest_response(
    username: &str,
    realm: &str,
    password: &str,
    nonce: &str,
    method: &RtspMethod,
    uri: &str,
    algorithm: RtspDigestAlgorithm,
    qop: Option<&str>,
    nc: Option<&str>,
    cnonce: Option<&str>,
) -> String {
    let ha1 = digest_hex(algorithm, &format!("{username}:{realm}:{password}"));
    let ha2 = digest_hex(algorithm, &format!("{}:{uri}", method.as_str()));
    match qop {
        Some(qop) => {
            let nc = nc.unwrap_or("00000001");
            let cnonce = cnonce.unwrap_or("");
            digest_hex(
                algorithm,
                &format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}"),
            )
        }
        None => digest_hex(algorithm, &format!("{ha1}:{nonce}:{ha2}")),
    }
}

fn trim_scheme_prefix<'a>(value: &'a str, scheme: &str) -> Option<&'a str> {
    let (prefix, payload) = value.split_once(char::is_whitespace)?;
    if prefix.eq_ignore_ascii_case(scheme) {
        Some(payload)
    } else {
        None
    }
}

fn parse_digest_params(value: &str) -> Result<Vec<(String, String)>, RtspAuthorizationError> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    for ch in value.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                current.push(ch);
            }
            ',' if !in_quote => {
                parse_digest_pair(current.trim(), &mut out)?;
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parse_digest_pair(current.trim(), &mut out)?;
    }
    Ok(out)
}

fn parse_digest_pair(
    value: &str,
    out: &mut Vec<(String, String)>,
) -> Result<(), RtspAuthorizationError> {
    let (raw_name, raw_value) = value
        .split_once('=')
        .ok_or_else(|| RtspAuthorizationError::InvalidDigestParameter("pair".to_string()))?;
    let name = raw_name.trim();
    if name.is_empty() {
        return Err(RtspAuthorizationError::InvalidDigestParameter(
            "name".to_string(),
        ));
    }
    let mut parsed_value = raw_value.trim();
    if parsed_value.starts_with('"') && parsed_value.ends_with('"') && parsed_value.len() >= 2 {
        parsed_value = &parsed_value[1..parsed_value.len() - 1];
    }
    out.push((name.to_string(), parsed_value.to_string()));
    Ok(())
}

fn required_digest_param<'a>(
    params: &'a [(String, String)],
    key: &str,
) -> Result<&'a str, RtspAuthorizationError> {
    params
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| RtspAuthorizationError::InvalidDigestParameter(key.to_string()))
}

fn parse_digest_algorithm(value: &str) -> RtspDigestAlgorithm {
    if value.eq_ignore_ascii_case("md5") {
        RtspDigestAlgorithm::Md5
    } else if value.eq_ignore_ascii_case("sha-256") || value.eq_ignore_ascii_case("sha256") {
        RtspDigestAlgorithm::Sha256
    } else {
        // Default to MD5 for unknown algorithms (backward compat)
        RtspDigestAlgorithm::Md5
    }
}

fn md5_hex(value: &str) -> String {
    format!("{:x}", md5::compute(value.as_bytes()))
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn digest_hex(algorithm: RtspDigestAlgorithm, value: &str) -> String {
    match algorithm {
        RtspDigestAlgorithm::Md5 => md5_hex(value),
        RtspDigestAlgorithm::Sha256 => sha256_hex(value),
    }
}

impl RtspDigestAlgorithm {
    /// Returns the algorithm name as used in WWW-Authenticate/Authorization headers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Md5 => "MD5",
            Self::Sha256 => "SHA-256",
        }
    }
}

impl RtspDigestChallenge {
    /// Format as a `WWW-Authenticate: Digest ...` header value.
    pub fn to_header_value(&self) -> String {
        let stale_part = if self.stale { ", stale=true" } else { "" };
        format!(
            r#"Digest realm="{}", nonce="{}", algorithm={}{stale_part}"#,
            self.realm,
            self.nonce,
            self.algorithm.as_str(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_authorization_success() {
        let header = "Basic dXNlcjpwYXNz";
        let parsed = parse_authorization_header(header).expect("parse");
        assert_eq!(
            parsed,
            RtspAuthorization::Basic {
                username: "user".to_string(),
                password: "pass".to_string()
            }
        );
    }

    #[test]
    fn parse_digest_authorization_success() {
        let header = r#"Digest username="user", realm="cheetah", nonce="abc", uri="rtsp://127.0.0.1/live/test", response="26d238c1b0db16cfff04cbc953857eed", algorithm=MD5"#;
        let parsed = parse_authorization_header(header).expect("parse");
        match parsed {
            RtspAuthorization::Digest(digest) => {
                assert_eq!(digest.username, "user");
                assert_eq!(digest.realm, "cheetah");
                assert_eq!(digest.nonce, "abc");
                assert_eq!(digest.uri, "rtsp://127.0.0.1/live/test");
                assert_eq!(digest.response, "26d238c1b0db16cfff04cbc953857eed");
                assert_eq!(digest.algorithm, RtspDigestAlgorithm::Md5);
                assert_eq!(digest.qop, None);
                assert_eq!(digest.nc, None);
                assert_eq!(digest.cnonce, None);
            }
            _ => panic!("expected digest auth"),
        }
    }

    #[test]
    fn parse_digest_authorization_sha256() {
        let header = r#"Digest username="user", realm="cheetah", nonce="abc", uri="rtsp://127.0.0.1/live/test", response="deadbeef", algorithm=SHA-256"#;
        let parsed = parse_authorization_header(header).expect("parse");
        let RtspAuthorization::Digest(digest) = parsed else {
            panic!("expected digest auth");
        };
        assert_eq!(digest.algorithm, RtspDigestAlgorithm::Sha256);
    }

    #[test]
    fn verify_digest_response_success_and_nonce_mismatch() {
        let auth = RtspDigestAuthorization {
            username: "user".to_string(),
            realm: "cheetah".to_string(),
            nonce: "abc".to_string(),
            uri: "rtsp://127.0.0.1/live/test".to_string(),
            response: "26d238c1b0db16cfff04cbc953857eed".to_string(),
            algorithm: RtspDigestAlgorithm::Md5,
            qop: None,
            nc: None,
            cnonce: None,
        };
        let challenge = RtspDigestChallenge {
            realm: "cheetah".to_string(),
            nonce: "abc".to_string(),
            algorithm: RtspDigestAlgorithm::Md5,
            stale: false,
        };
        assert!(verify_digest_response(
            &auth,
            &challenge,
            &RtspMethod::Describe,
            "pass"
        ));

        let wrong_nonce = RtspDigestChallenge {
            nonce: "def".to_string(),
            ..challenge
        };
        assert!(!verify_digest_response(
            &auth,
            &wrong_nonce,
            &RtspMethod::Describe,
            "pass"
        ));
    }

    #[test]
    fn verify_digest_response_sha256() {
        let username = "user";
        let realm = "cheetah";
        let nonce = "abc";
        let uri = "rtsp://127.0.0.1/live/test";
        let password = "pass";
        let ha1 = sha256_hex(&format!("{username}:{realm}:{password}"));
        let ha2 = sha256_hex(&format!("{}:{uri}", RtspMethod::Describe.as_str()));
        let response = sha256_hex(&format!("{ha1}:{nonce}:{ha2}"));
        let auth = RtspDigestAuthorization {
            username: username.to_string(),
            realm: realm.to_string(),
            nonce: nonce.to_string(),
            uri: uri.to_string(),
            response,
            algorithm: RtspDigestAlgorithm::Sha256,
            qop: None,
            nc: None,
            cnonce: None,
        };
        let challenge = RtspDigestChallenge {
            realm: realm.to_string(),
            nonce: nonce.to_string(),
            algorithm: RtspDigestAlgorithm::Sha256,
            stale: false,
        };
        assert!(verify_digest_response(
            &auth,
            &challenge,
            &RtspMethod::Describe,
            password
        ));
    }

    #[test]
    fn verify_digest_response_sha256_with_qop_auth() {
        let username = "user";
        let realm = "cheetah";
        let nonce = "abc";
        let uri = "rtsp://127.0.0.1/live/test";
        let password = "pass";
        let qop = "auth";
        let nc = "00000001";
        let cnonce = "deadbeef";
        let ha1 = sha256_hex(&format!("{username}:{realm}:{password}"));
        let ha2 = sha256_hex(&format!("{}:{uri}", RtspMethod::Describe.as_str()));
        let response = sha256_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}"));
        let auth = RtspDigestAuthorization {
            username: username.to_string(),
            realm: realm.to_string(),
            nonce: nonce.to_string(),
            uri: uri.to_string(),
            response,
            algorithm: RtspDigestAlgorithm::Sha256,
            qop: Some(qop.to_string()),
            nc: Some(nc.to_string()),
            cnonce: Some(cnonce.to_string()),
        };
        let challenge = RtspDigestChallenge {
            realm: realm.to_string(),
            nonce: nonce.to_string(),
            algorithm: RtspDigestAlgorithm::Sha256,
            stale: false,
        };
        assert!(verify_digest_response(
            &auth,
            &challenge,
            &RtspMethod::Describe,
            password
        ));
    }

    #[test]
    fn verify_digest_response_supports_qop_auth() {
        let username = "user";
        let realm = "cheetah";
        let nonce = "abc";
        let uri = "rtsp://127.0.0.1/live/test";
        let password = "pass";
        let qop = "auth";
        let nc = "00000001";
        let cnonce = "deadbeef";
        let ha1 = md5_hex(&format!("{username}:{realm}:{password}"));
        let ha2 = md5_hex(&format!("{}:{uri}", RtspMethod::Describe.as_str()));
        let response = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}"));
        let auth = RtspDigestAuthorization {
            username: username.to_string(),
            realm: realm.to_string(),
            nonce: nonce.to_string(),
            uri: uri.to_string(),
            response,
            algorithm: RtspDigestAlgorithm::Md5,
            qop: Some(qop.to_string()),
            nc: Some(nc.to_string()),
            cnonce: Some(cnonce.to_string()),
        };
        let challenge = RtspDigestChallenge {
            realm: realm.to_string(),
            nonce: nonce.to_string(),
            algorithm: RtspDigestAlgorithm::Md5,
            stale: false,
        };
        assert!(verify_digest_response(
            &auth,
            &challenge,
            &RtspMethod::Describe,
            password
        ));
    }

    #[test]
    fn verify_digest_response_qop_auth_requires_nc_and_cnonce() {
        let auth = RtspDigestAuthorization {
            username: "user".to_string(),
            realm: "cheetah".to_string(),
            nonce: "abc".to_string(),
            uri: "rtsp://127.0.0.1/live/test".to_string(),
            response: "ignored".to_string(),
            algorithm: RtspDigestAlgorithm::Md5,
            qop: Some("auth".to_string()),
            nc: None,
            cnonce: Some("deadbeef".to_string()),
        };
        let challenge = RtspDigestChallenge {
            realm: "cheetah".to_string(),
            nonce: "abc".to_string(),
            algorithm: RtspDigestAlgorithm::Md5,
            stale: false,
        };
        assert!(!verify_digest_response(
            &auth,
            &challenge,
            &RtspMethod::Describe,
            "pass"
        ));

        let auth_missing_cnonce = RtspDigestAuthorization {
            cnonce: None,
            nc: Some("00000001".to_string()),
            ..auth
        };
        assert!(!verify_digest_response(
            &auth_missing_cnonce,
            &challenge,
            &RtspMethod::Describe,
            "pass"
        ));
    }

    #[test]
    fn verify_digest_response_rejects_unsupported_qop() {
        let auth = RtspDigestAuthorization {
            username: "user".to_string(),
            realm: "cheetah".to_string(),
            nonce: "abc".to_string(),
            uri: "rtsp://127.0.0.1/live/test".to_string(),
            response: "ignored".to_string(),
            algorithm: RtspDigestAlgorithm::Md5,
            qop: Some("auth-int".to_string()),
            nc: Some("00000001".to_string()),
            cnonce: Some("deadbeef".to_string()),
        };
        let challenge = RtspDigestChallenge {
            realm: "cheetah".to_string(),
            nonce: "abc".to_string(),
            algorithm: RtspDigestAlgorithm::Md5,
            stale: false,
        };
        assert!(!verify_digest_response(
            &auth,
            &challenge,
            &RtspMethod::Describe,
            "pass"
        ));
    }

    #[test]
    fn verify_digest_response_rejects_algorithm_mismatch() {
        let auth = RtspDigestAuthorization {
            username: "user".to_string(),
            realm: "cheetah".to_string(),
            nonce: "abc".to_string(),
            uri: "rtsp://127.0.0.1/live/test".to_string(),
            response: "ignored".to_string(),
            algorithm: RtspDigestAlgorithm::Sha256,
            qop: None,
            nc: None,
            cnonce: None,
        };
        let challenge = RtspDigestChallenge {
            realm: "cheetah".to_string(),
            nonce: "abc".to_string(),
            algorithm: RtspDigestAlgorithm::Md5,
            stale: false,
        };
        assert!(!verify_digest_response(
            &auth,
            &challenge,
            &RtspMethod::Describe,
            "pass"
        ));
    }

    #[test]
    fn parse_digest_authorization_extracts_qop_nc_cnonce() {
        let header = r#"Digest username="user", realm="cheetah", nonce="abc", uri="rtsp://127.0.0.1/live/test", response="deadbeef", algorithm=MD5, qop="AUTH", nc=00000001, cnonce="cafebabe""#;
        let parsed = parse_authorization_header(header).expect("parse");
        let RtspAuthorization::Digest(digest) = parsed else {
            panic!("expected digest auth");
        };
        assert_eq!(digest.qop.as_deref(), Some("AUTH"));
        assert_eq!(digest.nc.as_deref(), Some("00000001"));
        assert_eq!(digest.cnonce.as_deref(), Some("cafebabe"));
    }

    #[test]
    fn compute_digest_response_md5() {
        let response = compute_digest_response(
            "user",
            "cheetah",
            "pass",
            "abc",
            &RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            RtspDigestAlgorithm::Md5,
            None,
            None,
            None,
        );
        assert_eq!(response, "26d238c1b0db16cfff04cbc953857eed");
    }

    #[test]
    fn compute_digest_response_sha256_roundtrip() {
        let response = compute_digest_response(
            "user",
            "cheetah",
            "pass",
            "nonce123",
            &RtspMethod::Setup,
            "rtsp://127.0.0.1/live/test",
            RtspDigestAlgorithm::Sha256,
            None,
            None,
            None,
        );
        let auth = RtspDigestAuthorization {
            username: "user".to_string(),
            realm: "cheetah".to_string(),
            nonce: "nonce123".to_string(),
            uri: "rtsp://127.0.0.1/live/test".to_string(),
            response,
            algorithm: RtspDigestAlgorithm::Sha256,
            qop: None,
            nc: None,
            cnonce: None,
        };
        let challenge = RtspDigestChallenge {
            realm: "cheetah".to_string(),
            nonce: "nonce123".to_string(),
            algorithm: RtspDigestAlgorithm::Sha256,
            stale: false,
        };
        assert!(verify_digest_response(
            &auth,
            &challenge,
            &RtspMethod::Setup,
            "pass"
        ));
    }

    #[test]
    fn challenge_to_header_value() {
        let challenge = RtspDigestChallenge {
            realm: "cheetah".to_string(),
            nonce: "abc123".to_string(),
            algorithm: RtspDigestAlgorithm::Sha256,
            stale: false,
        };
        assert_eq!(
            challenge.to_header_value(),
            r#"Digest realm="cheetah", nonce="abc123", algorithm=SHA-256"#
        );

        let stale_challenge = RtspDigestChallenge {
            stale: true,
            ..challenge
        };
        assert_eq!(
            stale_challenge.to_header_value(),
            r#"Digest realm="cheetah", nonce="abc123", algorithm=SHA-256, stale=true"#
        );
    }

    #[test]
    fn algorithm_as_str() {
        assert_eq!(RtspDigestAlgorithm::Md5.as_str(), "MD5");
        assert_eq!(RtspDigestAlgorithm::Sha256.as_str(), "SHA-256");
    }
}
