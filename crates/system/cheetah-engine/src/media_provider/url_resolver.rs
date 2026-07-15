//! Config-backed playable URL resolver for StreamInfo / getStreamUrl.
//!
//! 基于配置的可播放 URL 解析器，供 StreamInfo / getStreamUrl 使用。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cheetah_media_api::error::{MediaError, Result};
use cheetah_media_api::ids::{MediaKey, MediaSchema};
use cheetah_media_api::model::MediaUrl;
use cheetah_media_api::port::{MediaRequestContext, MediaUrlResolverApi};
use cheetah_sdk::ConfigProvider;
use serde_json::Value;

/// Default public-facing URL templates derived from engine config.
///
/// Config keys under the global root (optional):
/// - `media.public_host` (default `127.0.0.1`)
/// - `media.public_rtmp_port` (default `1935`)
/// - `media.public_http_port` (default `80`)
/// - `media.public_rtsp_port` (default `554`)
/// - `media.public_https` (default `false`)
/// - `media.url_sign_secret` (optional; enables short-lived signed URLs)
/// - `media.url_ttl_secs` (default `3600`)
///
/// 从引擎配置派生的默认公网 URL 模板。
#[derive(Clone)]
pub struct EngineMediaUrlResolver {
    config: Arc<dyn ConfigProvider>,
}

impl EngineMediaUrlResolver {
    pub fn new(config: Arc<dyn ConfigProvider>) -> Self {
        Self { config }
    }

    fn media_section(&self) -> Value {
        let global = self.config.global();
        global
            .get("media")
            .cloned()
            .unwrap_or(Value::Object(Default::default()))
    }

    fn string_field(section: &Value, key: &str, default: &str) -> String {
        section
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(default)
            .to_string()
    }

    fn u16_field(section: &Value, key: &str, default: u16) -> u16 {
        section
            .get(key)
            .and_then(|v| v.as_u64())
            .and_then(|v| u16::try_from(v).ok())
            .unwrap_or(default)
    }

    fn bool_field(section: &Value, key: &str, default: bool) -> bool {
        section
            .get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }

    fn app_stream_path(key: &MediaKey) -> String {
        format!("{}/{}", key.app.0, key.stream.0)
    }

    fn build_url(&self, schema: MediaSchema, key: &MediaKey) -> Option<MediaUrl> {
        let media = self.media_section();
        let host = Self::string_field(&media, "public_host", "127.0.0.1");
        let https = Self::bool_field(&media, "public_https", false);
        let path = Self::app_stream_path(key);

        let (url, available) = match schema {
            MediaSchema::Rtmp => {
                let port = Self::u16_field(&media, "public_rtmp_port", 1935);
                (format!("rtmp://{host}:{port}/{path}"), true)
            }
            MediaSchema::Rtsp => {
                let port = Self::u16_field(&media, "public_rtsp_port", 554);
                (format!("rtsp://{host}:{port}/{path}"), true)
            }
            MediaSchema::HttpFlv => {
                let port = Self::u16_field(&media, "public_http_port", 80);
                let scheme = if https { "https" } else { "http" };
                (format!("{scheme}://{host}:{port}/{path}.live.flv"), true)
            }
            MediaSchema::Hls => {
                let port = Self::u16_field(&media, "public_http_port", 80);
                let scheme = if https { "https" } else { "http" };
                (format!("{scheme}://{host}:{port}/{path}/hls.m3u8"), true)
            }
            MediaSchema::Webrtc => {
                let port = Self::u16_field(&media, "public_http_port", 80);
                let scheme = if https { "https" } else { "http" };
                (
                    format!(
                        "{scheme}://{host}:{port}/index/api/webrtc?app={}&stream={}",
                        key.app.0, key.stream.0
                    ),
                    true,
                )
            }
            MediaSchema::Ts | MediaSchema::Fmp4 | MediaSchema::Srt | MediaSchema::Rtp => {
                return None;
            }
            _ => return None,
        };

        let secret = Self::string_field(&media, "url_sign_secret", "");
        let (url, expires_at) = if secret.is_empty() {
            (url, None)
        } else {
            let ttl = media
                .get("url_ttl_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(3600)
                .max(1);
            let exp = now_secs() + ttl as i64;
            let signed = sign_url(&url, exp, &secret);
            (signed, Some(exp * 1000))
        };

        Some(MediaUrl {
            schema,
            url,
            available,
            expires_at,
        })
    }
}

#[async_trait]
impl MediaUrlResolverApi for EngineMediaUrlResolver {
    async fn resolve_urls(
        &self,
        _ctx: &MediaRequestContext,
        key: &MediaKey,
        schemas: &[MediaSchema],
    ) -> Result<Vec<MediaUrl>> {
        let requested: Vec<MediaSchema> = if schemas.is_empty() {
            vec![
                MediaSchema::Rtmp,
                MediaSchema::Rtsp,
                MediaSchema::HttpFlv,
                MediaSchema::Hls,
                MediaSchema::Webrtc,
            ]
        } else {
            schemas.to_vec()
        };

        let mut out = Vec::with_capacity(requested.len());
        for schema in requested {
            if let Some(url) = self.build_url(schema, key) {
                out.push(url);
            }
        }
        if out.is_empty() && !schemas.is_empty() {
            return Err(MediaError::unsupported(
                "none of the requested media schemas are supported by the url resolver",
            ));
        }
        Ok(out)
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Lightweight non-cryptographic token for short-lived URL markers.
/// Production deployments should replace this with HMAC via a secrets service.
///
/// 短时 URL 标记的轻量 token；生产环境应由密钥服务的 HMAC 替换。
fn sign_url(base: &str, exp: i64, secret: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in secret.bytes().chain(base.bytes()).chain(exp.to_le_bytes()) {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    let sig = format!("{hash:016x}");
    let sep = if base.contains('?') { '&' } else { '?' };
    format!("{base}{sep}exp={exp}&sign={sig}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_media_api::ids::{AppName, StreamName, VhostName};
    use serde_json::json;
    use std::any::Any;

    struct StaticConfig(Value);

    impl ConfigProvider for StaticConfig {
        fn global(&self) -> Value {
            self.0.clone()
        }
        fn module(&self, _module_id: &cheetah_sdk::ModuleId) -> Value {
            Value::Null
        }
        fn version(&self) -> u64 {
            1
        }
    }

    // silence unused Any import warning path
    fn _assert_object_safe(_: &dyn Any) {}

    #[tokio::test]
    async fn resolves_default_urls_without_trusting_host_header() {
        let resolver = EngineMediaUrlResolver::new(Arc::new(StaticConfig(json!({
            "media": {
                "public_host": "play.example.com",
                "public_rtmp_port": 1935,
                "public_http_port": 8080
            }
        }))));
        let key = MediaKey {
            vhost: VhostName("__defaultVhost__".into()),
            app: AppName("live".into()),
            stream: StreamName("cam1".into()),
            schema: None,
        };
        let urls = resolver
            .resolve_urls(&MediaRequestContext::default(), &key, &[])
            .await
            .expect("resolve");
        assert!(urls.iter().any(|u| u.url.contains("play.example.com")));
        assert!(urls
            .iter()
            .any(|u| u.schema == MediaSchema::Rtmp && u.url.starts_with("rtmp://")));
        assert!(urls
            .iter()
            .any(|u| u.schema == MediaSchema::HttpFlv && u.url.contains(":8080/")));
    }

    #[tokio::test]
    async fn signed_urls_include_exp_and_sign() {
        let resolver = EngineMediaUrlResolver::new(Arc::new(StaticConfig(json!({
            "media": {
                "public_host": "cdn.example",
                "url_sign_secret": "s3cr3t",
                "url_ttl_secs": 60
            }
        }))));
        let key = MediaKey {
            vhost: VhostName("__defaultVhost__".into()),
            app: AppName("live".into()),
            stream: StreamName("cam1".into()),
            schema: None,
        };
        let urls = resolver
            .resolve_urls(&MediaRequestContext::default(), &key, &[MediaSchema::Rtmp])
            .await
            .expect("resolve");
        assert_eq!(urls.len(), 1);
        assert!(urls[0].url.contains("exp="));
        assert!(urls[0].url.contains("sign="));
        assert!(urls[0].expires_at.is_some());
    }
}
