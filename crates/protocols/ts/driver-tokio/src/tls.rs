//! TLS support for TS driver (HTTPS/WSS).

/// Load TLS certificate and key from files.
pub fn load_tls_config(cert_path: &str, key_path: &str) -> Result<rustls::ServerConfig, String> {
    let cert_data = std::fs::read(cert_path).map_err(|e| format!("read cert {cert_path}: {e}"))?;
    let key_data = std::fs::read(key_path).map_err(|e| format!("read key {key_path}: {e}"))?;

    let certs = rustls_pemfile::certs(&mut &cert_data[..])
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("parse certs: {e}"))?;

    let key = rustls_pemfile::private_key(&mut &key_data[..])
        .map_err(|e| format!("parse key: {e}"))?
        .ok_or_else(|| "no private key found".to_string())?;

    rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("TLS config: {e}"))
}
