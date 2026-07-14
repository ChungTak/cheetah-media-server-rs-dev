use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use cheetah_media_api::error::{MediaError, Result as MediaResult};
use cheetah_media_api::ids::{MediaKey, MediaSchema};
use cheetah_media_api::model::MediaUrl;
use cheetah_media_api::port::{MediaRequestContext, MediaUrlResolverApi, UrlResolverTemplate};
use hmac::{Hmac, Mac};
use percent_encoding::{percent_encode, NON_ALPHANUMERIC};
use serde_json::Value;
use sha2::Sha256;

/// Centralized media URL resolver.
///
/// Builds playable URLs from a `MediaKey` and a set of output schemas using the
/// configured public host, port, TLS, and base path. Protocol modules can
/// override per-schema templates via `register_template`.
///
/// 集中式媒体 URL 解析器。根据配置公网 host、端口、TLS 和 base path 为
/// `MediaKey` 和输出 schema 生成可播放 URL。协议模块可通过 `register_template`
/// 覆盖每个 schema 的模板。
#[derive(Clone)]
pub struct EngineMediaUrlResolver {
    config: Arc<UrlResolverConfig>,
    templates: Arc<RwLock<HashMap<MediaSchema, UrlResolverTemplate>>>,
}

#[derive(Debug, Clone)]
struct UrlResolverConfig {
    public_host: String,
    public_port: u16,
    tls: bool,
    base_path: String,
    signing_secret: Option<Vec<u8>>,
    token_ttl_seconds: u64,
    token_required: bool,
}

impl EngineMediaUrlResolver {
    /// Build a resolver from the global config object.
    ///
    /// 从全局配置对象构建解析器。
    pub fn from_config(config: &Value) -> Self {
        let cfg = UrlResolverConfig::from_global(config);
        let mut resolver = Self {
            config: Arc::new(cfg),
            templates: Arc::new(RwLock::new(HashMap::new())),
        };
        resolver.seed_defaults();
        resolver
    }

    /// Build a resolver directly from a host/port/tls/base_path tuple.
    ///
    /// 直接使用 host/port/tls/base_path 构建解析器。
    #[allow(dead_code)]
    pub fn new(
        public_host: impl Into<String>,
        public_port: u16,
        tls: bool,
        base_path: impl Into<String>,
    ) -> Self {
        let cfg = UrlResolverConfig {
            public_host: public_host.into(),
            public_port,
            tls,
            base_path: base_path.into(),
            signing_secret: None,
            token_ttl_seconds: 3600,
            token_required: false,
        };
        let mut resolver = Self {
            config: Arc::new(cfg),
            templates: Arc::new(RwLock::new(HashMap::new())),
        };
        resolver.seed_defaults();
        resolver
    }

    fn seed_defaults(&mut self) {
        let defaults = [
            (MediaSchema::Hls, "/hls/{vhost}/{app}/{stream}/index.m3u8"),
            (MediaSchema::HttpFlv, "/flv/{vhost}/{app}/{stream}.flv"),
            (MediaSchema::Fmp4, "/fmp4/{vhost}/{app}/{stream}.fmp4"),
            (MediaSchema::Ts, "/ts/{vhost}/{app}/{stream}.ts"),
            (MediaSchema::Webrtc, "/webrtc/{vhost}/{app}/{stream}"),
            (MediaSchema::Rtmp, "/{vhost}/{app}/{stream}"),
            (MediaSchema::Rtsp, "/{vhost}/{app}/{stream}"),
        ];
        if let Ok(mut map) = self.templates.write() {
            for (schema, path) in defaults {
                map.entry(schema).or_insert_with(|| UrlResolverTemplate {
                    path_template: path.to_string(),
                    ..Default::default()
                });
            }
        }
    }

    fn resolve_one(&self, key: &MediaKey, schema: MediaSchema) -> MediaUrl {
        let template = self
            .templates
            .read()
            .ok()
            .and_then(|m| m.get(&schema).cloned());

        let Some(template) = template else {
            return MediaUrl {
                schema,
                url: String::new(),
                available: false,
                expires_at: None,
            };
        };

        let protocol = template
            .protocol
            .as_deref()
            .or_else(|| schema.default_url_protocol(self.config.tls))
            .unwrap_or("http");

        let host = template.host.as_deref().unwrap_or(&self.config.public_host);
        let port = template.port.unwrap_or(self.config.public_port);

        let path = self.fill_path(&template.path_template, key);
        let full_path = join_base_path(&self.config.base_path, &path);

        let mut url = if matches!(protocol, "srt") {
            // SRT URLs carry the stream id as a query parameter.
            format!("{protocol}://{host}:{port}{full_path}")
        } else {
            format!("{protocol}://{host}:{port}{full_path}")
        };

        let mut expires_at = None;
        let requires_token = template.requires_token || self.config.token_required;
        if requires_token {
            if let Some(secret) = &self.config.signing_secret {
                let now = now_seconds();
                let exp = now + self.config.token_ttl_seconds;
                let token = sign_url(secret, &full_path, exp);
                let sep = if url.contains('?') { '&' } else { '?' };
                url.push_str(&format!("{sep}token={token}&expires={exp}"));
                expires_at = Some(exp as i64);
            }
        }

        MediaUrl {
            schema,
            url,
            available: true,
            expires_at,
        }
    }

    fn fill_path(&self, template: &str, key: &MediaKey) -> String {
        template
            .replace("{vhost}", &encode_path_segment(&key.vhost.0))
            .replace("{app}", &encode_path_segment(&key.app.0))
            .replace("{stream}", &encode_path_segment(&key.stream.0))
    }
}

const PATH_SEGMENT_SET: percent_encoding::AsciiSet = NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn encode_path_segment(s: &str) -> String {
    percent_encode(s.as_bytes(), &PATH_SEGMENT_SET).to_string()
}

#[async_trait]
impl MediaUrlResolverApi for EngineMediaUrlResolver {
    async fn resolve_urls(
        &self,
        _ctx: &MediaRequestContext,
        key: &MediaKey,
        schemas: &[MediaSchema],
    ) -> MediaResult<Vec<MediaUrl>> {
        if schemas.is_empty() {
            return Err(MediaError::invalid_argument(
                "at least one output schema must be requested",
            ));
        }
        Ok(schemas.iter().map(|&s| self.resolve_one(key, s)).collect())
    }

    fn register_template(
        &self,
        schema: MediaSchema,
        template: UrlResolverTemplate,
    ) -> MediaResult<()> {
        if template.path_template.is_empty() {
            return Err(MediaError::invalid_argument(
                "url resolver template path cannot be empty",
            ));
        }
        if let Ok(mut map) = self.templates.write() {
            map.insert(schema, template);
        }
        Ok(())
    }

    fn supports_schema(&self, schema: MediaSchema) -> bool {
        self.templates
            .read()
            .ok()
            .map(|m| m.contains_key(&schema))
            .unwrap_or(false)
    }
}

impl UrlResolverConfig {
    fn from_global(config: &Value) -> Self {
        let ms = config.get("media_server").and_then(|v| v.as_object());

        let tls = ms
            .and_then(|m| m.get("tls"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let default_port = if tls { 443 } else { 80 };
        let public_port = ms
            .and_then(|m| m.get("public_port"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u16)
            .unwrap_or(default_port);

        let public_host = ms
            .and_then(|m| m.get("public_host"))
            .and_then(|v| v.as_str())
            .unwrap_or("localhost")
            .to_string();

        let base_path = ms
            .and_then(|m| m.get("base_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let base_path = normalize_base_path(base_path);

        let token_ttl_seconds = ms
            .and_then(|m| m.get("url_token_ttl_seconds"))
            .and_then(|v| v.as_u64())
            .unwrap_or(3600);

        let token_required = ms
            .and_then(|m| m.get("url_token_required"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let signing_secret = ms
            .and_then(|m| m.get("url_token_secret"))
            .and_then(|v| v.as_str())
            .map(|s| s.as_bytes().to_vec());

        Self {
            public_host,
            public_port,
            tls,
            base_path,
            signing_secret,
            token_ttl_seconds,
            token_required,
        }
    }
}

fn normalize_base_path(path: String) -> String {
    if path.is_empty() || path == "/" {
        return String::new();
    }
    let mut p = path;
    if !p.starts_with('/') {
        p.insert(0, '/');
    }
    while p.ends_with('/') {
        p.pop();
    }
    p
}

fn join_base_path(base: &str, path: &str) -> String {
    if base.is_empty() {
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        }
    } else if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

fn sign_url(secret: &[u8], path: &str, expires: u64) -> String {
    let msg = format!("{path}\n{expires}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(msg.as_bytes());
    let result = mac.finalize();
    hex_encode(&result.into_bytes())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_templates_resolve_http_schemas() {
        let resolver = EngineMediaUrlResolver::new("example.com", 443, true, "");
        let key = MediaKey::new("vhost", "app", "stream", None).unwrap();
        let urls = resolver
            .resolve_urls(
                &MediaRequestContext::default(),
                &key,
                &[
                    MediaSchema::Hls,
                    MediaSchema::HttpFlv,
                    MediaSchema::Webrtc,
                    MediaSchema::Rtp,
                ],
            )
            .await
            .unwrap();

        assert_eq!(urls.len(), 4);
        let hls = urls.iter().find(|u| u.schema == MediaSchema::Hls).unwrap();
        assert!(hls.available);
        assert!(hls.url.contains("/hls/vhost/app/stream/index.m3u8"));
        let rtp = urls.iter().find(|u| u.schema == MediaSchema::Rtp).unwrap();
        assert!(!rtp.available);
    }

    #[tokio::test]
    async fn signed_urls_include_token_and_expiry() {
        let cfg = UrlResolverConfig {
            public_host: "example.com".to_string(),
            public_port: 443,
            tls: true,
            base_path: String::new(),
            signing_secret: Some(b"secret".to_vec()),
            token_ttl_seconds: 120,
            token_required: true,
        };
        let resolver = EngineMediaUrlResolver {
            config: Arc::new(cfg),
            templates: Arc::new(RwLock::new(HashMap::new())),
        };
        // Do not seed defaults so we can register a token-requiring template.
        let template = UrlResolverTemplate {
            path_template: "/live/{vhost}/{app}/{stream}.flv".to_string(),
            requires_token: true,
            ..Default::default()
        };
        resolver
            .register_template(MediaSchema::HttpFlv, template)
            .unwrap();

        let key = MediaKey::new("vhost", "app", "stream", None).unwrap();
        let urls = resolver
            .resolve_urls(
                &MediaRequestContext::default(),
                &key,
                &[MediaSchema::HttpFlv],
            )
            .await
            .unwrap();

        assert_eq!(urls.len(), 1);
        let url = &urls[0].url;
        assert!(url.contains("token="));
        assert!(url.contains("expires="));
        assert!(urls[0].expires_at.is_some());
    }
}
