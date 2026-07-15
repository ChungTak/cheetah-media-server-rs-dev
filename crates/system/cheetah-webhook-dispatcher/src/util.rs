use std::collections::HashMap;

/// Sign a webhook body with HMAC-SHA256 and return the base64 signature.
///
/// 使用 HMAC-SHA256 对 webhook body 签名并返回 base64 签名值。
pub fn sign_body(body: &[u8], secret: &str) -> Result<String, hmac::digest::InvalidLength> {
    use base64::Engine;
    use hmac::Mac;
    use sha2::Sha256;

    type HmacSha256 = hmac::Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())?;
    mac.update(body);
    let result = mac.finalize().into_bytes();
    Ok(base64::engine::general_purpose::STANDARD.encode(result))
}

/// True if an HTTP status code indicates success.
pub fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

/// True if an HTTP status code is a client error (no retry).
pub fn is_client_error(status: u16) -> bool {
    (400..500).contains(&status)
}

/// Build the common headers for a webhook POST.
pub fn webhook_headers(event_id: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("X-Event-Id".to_string(), event_id.to_string());
    headers
}
