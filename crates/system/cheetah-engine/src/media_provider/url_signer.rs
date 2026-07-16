//! HMAC-SHA256 URL signer with key rotation support.
//!
//! 支持密钥轮换的 HMAC-SHA256 URL 签名器。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

const DEFAULT_KEY_ID: &str = "0";

#[derive(Debug, Clone)]
struct SignKey {
    id: String,
    secret: Vec<u8>,
    /// Unix seconds after which this key may no longer be used for
    /// verification. `None` means the key never expires.
    valid_until: Option<i64>,
}

/// HMAC-SHA256 URL signer supporting multiple keys for rotation.
///
/// Keys are configured under `media.url_sign_keys` as an array of objects:
/// `{ "id": "kid", "secret": "..." }`. The first key is used for signing;
/// all configured keys are accepted for verification so old URLs remain valid
/// during rotation.
///
/// When `media.url_sign_previous_key_ttl_secs` is set, verification keys after
/// the first expire after that many seconds. This lets old URLs work during a
/// rotation window without keeping superseded keys valid indefinitely.
///
/// Fallback: `media.url_sign_secret` creates a single key with id `"0"`.
///
/// HMAC-SHA256 URL 签名器，支持多密钥轮换。
#[derive(Debug, Clone)]
pub struct UrlSigner {
    keys: Vec<SignKey>,
}

impl UrlSigner {
    /// Builds a signer from the `media` config section.
    pub fn from_config(media: &Value) -> Option<Self> {
        let previous_key_ttl = media
            .get("url_sign_previous_key_ttl_secs")
            .and_then(|v| v.as_u64());

        if let Some(list) = media.get("url_sign_keys").and_then(|v| v.as_array()) {
            let now = now_secs();
            let mut keys = Vec::with_capacity(list.len());
            for (idx, item) in list.iter().enumerate() {
                let (Some(id), Some(secret)) = (
                    item.get("id").and_then(|v| v.as_str()),
                    item.get("secret").and_then(|v| v.as_str()),
                ) else {
                    continue;
                };
                let id = id.to_string();
                let secret = secret.as_bytes().to_vec();
                if id.is_empty() || secret.is_empty() {
                    continue;
                }
                let valid_until = if idx == 0 {
                    None
                } else {
                    previous_key_ttl.map(|ttl| now + ttl as i64)
                };
                keys.push(SignKey {
                    id,
                    secret,
                    valid_until,
                });
            }
            if !keys.is_empty() {
                return Some(Self { keys });
            }
        }

        media
            .get("url_sign_secret")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| Self {
                keys: vec![SignKey {
                    id: DEFAULT_KEY_ID.to_string(),
                    secret: s.as_bytes().to_vec(),
                    valid_until: None,
                }],
            })
    }

    /// Signs a base URL, returning the signed URL and the expiration timestamp
    /// in seconds.
    ///
    /// Appends `?kid=...&exp=...&sign=...` (or `&` if the URL already has a
    /// query string).
    pub fn sign(&self, base: &str, ttl_secs: u64) -> Option<(String, i64)> {
        let key = self.keys.first()?;
        let exp = now_secs() + ttl_secs as i64;

        let mut url = url::Url::parse(base).ok()?;
        let canonical = canonical_string(&url, exp, &key.id);
        let sig = hmac_signature(&key.secret, &canonical);
        let sig_b64 = URL_SAFE_NO_PAD.encode(&sig);

        {
            let mut query = url.query_pairs_mut();
            query.append_pair("kid", &key.id);
            query.append_pair("exp", &exp.to_string());
            query.append_pair("sign", &sig_b64);
        }

        Some((url.to_string(), exp))
    }

    /// Verifies a signed URL. Returns `true` only if the signature is valid,
    /// the key id is known, and the URL has not expired.
    #[allow(dead_code)] // Used by HTTP playback/signature verification routes.
    pub fn verify(&self, url: &str) -> bool {
        let parsed = match url::Url::parse(url) {
            Ok(u) => u,
            Err(_) => return false,
        };

        let mut kid = None;
        let mut exp = None;
        let mut sig_b64 = None;
        for (k, v) in parsed.query_pairs() {
            match k.as_ref() {
                "kid" => kid = Some(v.into_owned()),
                "exp" => exp = v.parse::<i64>().ok(),
                "sign" => sig_b64 = Some(v.into_owned()),
                _ => {}
            }
        }

        let (Some(kid), Some(exp), Some(sig_b64)) = (kid, exp, sig_b64) else {
            return false;
        };

        let now = now_secs();
        if now > exp {
            return false;
        }

        let key = match self.keys.iter().find(|k| k.id == kid) {
            Some(k) => k,
            None => return false,
        };

        if let Some(valid_until) = key.valid_until {
            if now > valid_until {
                return false;
            }
        }

        let canonical = canonical_string(&parsed, exp, &kid);
        let expected = hmac_signature(&key.secret, &canonical);
        let provided = match URL_SAFE_NO_PAD.decode(sig_b64.as_bytes()) {
            Ok(v) => v,
            Err(_) => return false,
        };

        if expected.len() != provided.len() {
            return false;
        }
        expected.as_slice().ct_eq(&provided).into()
    }
}

fn canonical_string(url: &url::Url, exp: i64, kid: &str) -> String {
    let scheme = url.scheme();
    let host = url.host_str().unwrap_or("");
    let authority = match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    };
    let path = url.path();
    let query = canonical_query(url);
    if query.is_empty() {
        format!("{scheme}\n{authority}\n{path}\n{exp}\n{kid}")
    } else {
        format!("{scheme}\n{authority}\n{path}\n{query}\n{exp}\n{kid}")
    }
}

fn canonical_query(url: &url::Url) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in url.query_pairs() {
        if !matches!(k.as_ref(), "kid" | "exp" | "sign") {
            serializer.append_pair(k.as_ref(), v.as_ref());
        }
    }
    serializer.finish()
}

fn hmac_signature(secret: &[u8], canonical: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC key length is valid");
    mac.update(canonical.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn signer_with_secret(secret: &str) -> UrlSigner {
        UrlSigner::from_config(&json!({"url_sign_secret": secret})).unwrap()
    }

    #[test]
    fn no_signer_when_no_secret() {
        assert!(UrlSigner::from_config(&json!({})).is_none());
    }

    #[test]
    fn signing_adds_kid_exp_and_sign() {
        let signer = signer_with_secret("s3cr3t");
        let (url, exp) = signer
            .sign("rtmp://cdn.example:1935/live/cam1", 60)
            .unwrap();
        assert!(url.contains("kid=0"));
        assert!(url.contains(&format!("exp={exp}")));
        assert!(url.contains("sign="));
    }

    #[test]
    fn signed_url_verifies_with_same_secret() {
        let signer = signer_with_secret("s3cr3t");
        let (url, _exp) = signer
            .sign("rtmp://cdn.example:1935/live/cam1", 60)
            .unwrap();
        assert!(signer.verify(&url));
    }

    #[test]
    fn tampered_url_fails_verification() {
        let signer = signer_with_secret("s3cr3t");
        let (mut url, _exp) = signer
            .sign("rtmp://cdn.example:1935/live/cam1", 60)
            .unwrap();
        url.push_str("&extra=1");
        assert!(!signer.verify(&url));
    }

    #[test]
    fn expired_url_fails_verification() {
        let signer = signer_with_secret("s3cr3t");
        let (url, _exp) = signer.sign("rtmp://cdn.example:1935/live/cam1", 1).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert!(!signer.verify(&url));
    }

    #[test]
    fn old_key_still_verifies_during_rotation() {
        let signer = UrlSigner::from_config(&json!({
            "url_sign_keys": [
                {"id": "new", "secret": "new-secret"},
                {"id": "old", "secret": "old-secret"}
            ]
        }))
        .unwrap();

        // Sign with the current (first) key.
        let (url, _exp) = signer
            .sign("rtmp://cdn.example:1935/live/cam1", 60)
            .unwrap();
        assert!(url.contains("kid=new"));
        assert!(signer.verify(&url));

        // URLs signed with the old key (simulate by creating a separate signer
        // that only has the old key) are accepted because the old key is still
        // in the rotation list.
        let old_signer = UrlSigner::from_config(&json!({
            "url_sign_keys": [
                {"id": "old", "secret": "old-secret"}
            ]
        }))
        .unwrap();
        let (old_url, _exp) = old_signer
            .sign("rtmp://cdn.example:1935/live/cam1", 60)
            .unwrap();
        assert!(signer.verify(&old_url));
    }

    #[test]
    fn key_id_with_special_chars_is_encoded_and_verifies() {
        let signer = UrlSigner::from_config(&json!({
            "url_sign_keys": [
                {"id": "key&x=y", "secret": "secret"}
            ]
        }))
        .unwrap();
        let (url, _exp) = signer
            .sign("rtmp://cdn.example:1935/live/cam1", 60)
            .unwrap();
        assert!(url.contains("kid=key%26x%3Dy"));
        assert!(signer.verify(&url));
    }

    #[test]
    fn unknown_key_id_fails_verification() {
        let signer = UrlSigner::from_config(&json!({
            "url_sign_keys": [
                {"id": "a", "secret": "secret"}
            ]
        }))
        .unwrap();
        let (mut url, _exp) = signer
            .sign("rtmp://cdn.example:1935/live/cam1", 60)
            .unwrap();
        url = url.replace("kid=a", "kid=b");
        assert!(!signer.verify(&url));
    }

    #[test]
    fn previous_key_verifies_within_ttl_and_expires_after() {
        let old_signer = UrlSigner::from_config(&json!({
            "url_sign_keys": [
                {"id": "old", "secret": "old-secret"}
            ]
        }))
        .unwrap();
        let (old_url, _exp) = old_signer
            .sign("rtmp://cdn.example:1935/live/cam1", 60)
            .unwrap();

        // A signer loaded with the new key plus the old key accepts the old
        // URL when the previous-key TTL has not expired.
        let signer = UrlSigner::from_config(&json!({
            "url_sign_previous_key_ttl_secs": 2,
            "url_sign_keys": [
                {"id": "new", "secret": "new-secret"},
                {"id": "old", "secret": "old-secret"}
            ]
        }))
        .unwrap();
        assert!(signer.verify(&old_url));

        // After the TTL, the old key is rejected even though the URL itself
        // has not expired.
        std::thread::sleep(std::time::Duration::from_secs(3));
        assert!(!signer.verify(&old_url));
    }
}
