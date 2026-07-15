//! Runtime-driven playable URL resolver backed by the output endpoint registry.
//!
//! 基于输出端点注册表的运行时驱动可播放 URL 解析器。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cheetah_media_api::error::{MediaError, Result};
use cheetah_media_api::ids::{MediaKey, MediaSchema};
use cheetah_media_api::model::{MediaUrl, OnlineState};
use cheetah_media_api::output::EndpointState;
use cheetah_media_api::port::{MediaRequestContext, MediaUrlResolverApi};
use cheetah_sdk::{ConfigProvider, MediaServices};
use serde_json::Value;

/// Public URL resolver driven by the output endpoint registry.
///
/// Uses active endpoint snapshots to decide which schemas are available,
/// falls back to configured public host/ports, and signs URLs when a secret is
/// configured.
///
/// 由输出端点注册表驱动的公网 URL 解析器。
#[derive(Clone)]
pub struct EngineMediaUrlResolver {
    media_services: MediaServices,
    config: Arc<dyn ConfigProvider>,
}

impl EngineMediaUrlResolver {
    pub fn new(media_services: MediaServices, config: Arc<dyn ConfigProvider>) -> Self {
        Self {
            media_services,
            config,
        }
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

    fn optional_u16(section: &Value, key: &str) -> Option<u16> {
        section
            .get(key)
            .and_then(|v| v.as_u64())
            .and_then(|v| u16::try_from(v).ok())
    }

    fn public_port(schema: MediaSchema, section: &Value, endpoint_port: u16) -> u16 {
        let configured = match schema {
            MediaSchema::Rtmp => Self::optional_u16(section, "public_rtmp_port"),
            MediaSchema::Rtsp => Self::optional_u16(section, "public_rtsp_port"),
            MediaSchema::HttpFlv
            | MediaSchema::Hls
            | MediaSchema::Webrtc
            | MediaSchema::Ts
            | MediaSchema::Fmp4
            | MediaSchema::Rtp => Self::optional_u16(section, "public_http_port"),
            MediaSchema::Srt => None,
            _ => None,
        };
        configured.unwrap_or(endpoint_port)
    }

    fn scheme(schema: MediaSchema, tls: bool) -> Option<&'static str> {
        match schema {
            MediaSchema::Rtmp => Some(if tls { "rtmps" } else { "rtmp" }),
            MediaSchema::Rtsp => Some(if tls { "rtsps" } else { "rtsp" }),
            MediaSchema::HttpFlv
            | MediaSchema::Hls
            | MediaSchema::Webrtc
            | MediaSchema::Ts
            | MediaSchema::Fmp4
            | MediaSchema::Rtp => Some(if tls { "https" } else { "http" }),
            MediaSchema::Srt => Some("srt"),
            _ => None,
        }
    }

    fn render_template(template: &str, key: &MediaKey) -> String {
        template
            .replace("{vhost}", &key.vhost.0)
            .replace("{app}", &key.app.0)
            .replace("{stream}", &key.stream.0)
    }

    fn encode_url(scheme: &str, host: &str, port: u16, path: &str) -> Result<String> {
        let base = format!("{scheme}://{host}:{port}");
        let mut url = url::Url::parse(&base)
            .map_err(|e| MediaError::invalid_argument(format!("invalid url base: {e}")))?;
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        {
            let mut ps = url
                .path_segments_mut()
                .map_err(|_| MediaError::internal("url path not available"))?;
            ps.extend(segments);
        }
        Ok(url.to_string())
    }

    async fn is_media_online(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> Result<Option<OnlineState>> {
        match self.media_services.control() {
            Some(control) => control.is_media_online(ctx, key).await.map(Some),
            None => Ok(None),
        }
    }

    async fn build_url(
        &self,
        key: &MediaKey,
        online: Option<OnlineState>,
        endpoint: &cheetah_media_api::output::MediaOutputEndpoint,
    ) -> Result<MediaUrl> {
        let media = self.media_section();
        let host = Self::string_field(&media, "public_host", &endpoint.public_host);
        let port = Self::public_port(endpoint.schema, &media, endpoint.port);
        let scheme = Self::scheme(endpoint.schema, endpoint.tls).ok_or_else(|| {
            MediaError::unsupported(format!(
                "schema {} cannot be expressed as a playable URL",
                endpoint.schema
            ))
        })?;
        let raw_path = Self::render_template(&endpoint.path_template, key);
        let base_url = Self::encode_url(scheme, &host, port, &raw_path)?;

        let available = online == Some(OnlineState::Online);

        let secret = Self::string_field(&media, "url_sign_secret", "");
        let (url, expires_at) = if secret.is_empty() {
            (base_url, None)
        } else {
            let ttl = media
                .get("url_ttl_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(3600)
                .max(1);
            let exp = now_secs() + ttl as i64;
            (sign_url(&base_url, exp, &secret), Some(exp * 1000))
        };

        Ok(MediaUrl {
            schema: endpoint.schema,
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
        ctx: &MediaRequestContext,
        key: &MediaKey,
        schemas: &[MediaSchema],
    ) -> Result<Vec<MediaUrl>> {
        let Some(registry) = self.media_services.output_registry() else {
            if schemas.is_empty() {
                return Ok(Vec::new());
            }
            return Err(MediaError::unsupported(
                "no output endpoint registry is available",
            ));
        };

        let endpoints = registry.snapshot().await?;
        let active: Vec<_> = endpoints
            .into_iter()
            .filter(|e| e.state == EndpointState::Active)
            .collect();

        if active.is_empty() {
            if schemas.is_empty() {
                return Ok(Vec::new());
            }
            return Err(MediaError::unsupported(
                "no active output endpoints for the requested schemas",
            ));
        }

        let online = self.is_media_online(ctx, key).await?;

        if schemas.is_empty() {
            let mut out = Vec::with_capacity(active.len());
            for endpoint in active {
                out.push(self.build_url(key, online, &endpoint).await?);
            }
            return Ok(out);
        }

        let mut out = Vec::with_capacity(schemas.len());
        for schema in schemas {
            let matches: Vec<_> = active.iter().filter(|e| e.schema == *schema).collect();
            if matches.is_empty() {
                return Err(MediaError::unsupported(format!(
                    "schema {schema} has no active output endpoint"
                )));
            }
            for endpoint in matches {
                out.push(self.build_url(key, online, endpoint).await?);
            }
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
    use cheetah_media_api::command::{MediaQuery, SessionQuery};
    use cheetah_media_api::ids::{
        AppName, MediaKey, MediaSchema, SessionId, StreamName, VhostName,
    };
    use cheetah_media_api::model::{
        CloseReason, CloseReport, OnlineState, Page, SessionInfo, StreamInfo,
    };
    use cheetah_media_api::output::MediaOutputEndpoint;
    use cheetah_media_api::port::{MediaControlApi, MediaOutputRegistryApi, MediaRequestContext};
    use cheetah_sdk::module::MediaServices;
    use cheetah_sdk::output::InMemoryMediaOutputRegistry;
    use serde_json::json;
    use std::any::Any;
    use std::sync::Arc;

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

    struct AlwaysOnlineControl;

    #[async_trait]
    impl MediaControlApi for AlwaysOnlineControl {
        async fn get_media_list(
            &self,
            _ctx: &MediaRequestContext,
            _query: MediaQuery,
        ) -> Result<Page<StreamInfo>> {
            unimplemented!()
        }
        async fn get_media(
            &self,
            _ctx: &MediaRequestContext,
            _key: &cheetah_media_api::ids::MediaKey,
        ) -> Result<StreamInfo> {
            unimplemented!()
        }
        async fn is_media_online(
            &self,
            _ctx: &MediaRequestContext,
            _key: &cheetah_media_api::ids::MediaKey,
        ) -> Result<OnlineState> {
            Ok(OnlineState::Online)
        }
        async fn list_sessions(
            &self,
            _ctx: &MediaRequestContext,
            _query: SessionQuery,
        ) -> Result<Page<SessionInfo>> {
            unimplemented!()
        }
        async fn kick_session(
            &self,
            _ctx: &MediaRequestContext,
            _id: &SessionId,
            _reason: CloseReason,
        ) -> Result<()> {
            unimplemented!()
        }
        async fn kick_stream(
            &self,
            _ctx: &MediaRequestContext,
            _key: &cheetah_media_api::ids::MediaKey,
            _reason: CloseReason,
        ) -> Result<CloseReport> {
            unimplemented!()
        }
        async fn request_keyframe(
            &self,
            _ctx: &MediaRequestContext,
            _key: &cheetah_media_api::ids::MediaKey,
        ) -> Result<()> {
            unimplemented!()
        }
    }

    async fn services_with_endpoints(endpoints: Vec<MediaOutputEndpoint>) -> MediaServices {
        let services = MediaServices::unavailable();
        let registry = Arc::new(InMemoryMediaOutputRegistry::new());
        services.register_output_registry(registry.clone());
        for ep in endpoints {
            let _ = registry.register_endpoint(ep).await;
        }
        services.register_control(Arc::new(AlwaysOnlineControl));
        services
    }

    // silence unused Any import warning path
    fn _assert_object_safe(_: &dyn Any) {}

    #[tokio::test]
    async fn resolves_default_urls_without_trusting_host_header() {
        let config = Arc::new(StaticConfig(json!({
            "media": {
                "public_host": "play.example.com",
                "public_rtmp_port": 1935,
                "public_http_port": 8080
            }
        })));
        let endpoints = vec![
            MediaOutputEndpoint::new(
                "rtmp",
                MediaSchema::Rtmp,
                "127.0.0.1",
                1935,
                false,
                "{app}/{stream}",
            ),
            MediaOutputEndpoint::new(
                "http-flv",
                MediaSchema::HttpFlv,
                "127.0.0.1",
                8080,
                false,
                "{app}/{stream}.live.flv",
            ),
        ];
        let services = services_with_endpoints(endpoints).await;
        let resolver = EngineMediaUrlResolver::new(services, config);
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
        let config = Arc::new(StaticConfig(json!({
            "media": {
                "public_host": "cdn.example",
                "url_sign_secret": "s3cr3t",
                "url_ttl_secs": 60
            }
        })));
        let endpoints = vec![MediaOutputEndpoint::new(
            "rtmp",
            MediaSchema::Rtmp,
            "127.0.0.1",
            1935,
            false,
            "{app}/{stream}",
        )];
        let services = services_with_endpoints(endpoints).await;
        let resolver = EngineMediaUrlResolver::new(services, config);
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

    #[tokio::test]
    async fn unknown_schema_returns_unsupported() {
        let config = Arc::new(StaticConfig(json!({
            "media": { "public_host": "play.example.com" }
        })));
        let endpoints = vec![MediaOutputEndpoint::new(
            "rtmp",
            MediaSchema::Rtmp,
            "127.0.0.1",
            1935,
            false,
            "{app}/{stream}",
        )];
        let services = services_with_endpoints(endpoints).await;
        let resolver = EngineMediaUrlResolver::new(services, config);
        let key = MediaKey {
            vhost: VhostName("__defaultVhost__".into()),
            app: AppName("live".into()),
            stream: StreamName("cam1".into()),
            schema: None,
        };
        let err = resolver
            .resolve_urls(&MediaRequestContext::default(), &key, &[MediaSchema::Hls])
            .await
            .unwrap_err();
        assert_eq!(
            err.code,
            cheetah_media_api::error::MediaErrorCode::Unsupported
        );
    }

    #[tokio::test]
    async fn path_template_is_percent_encoded() {
        let config = Arc::new(StaticConfig(json!({
            "media": { "public_host": "play.example.com" }
        })));
        let endpoints = vec![MediaOutputEndpoint::new(
            "http-flv",
            MediaSchema::HttpFlv,
            "127.0.0.1",
            8080,
            false,
            "{app}/{stream}.live.flv",
        )];
        let services = services_with_endpoints(endpoints).await;
        let resolver = EngineMediaUrlResolver::new(services, config);
        let key = MediaKey {
            vhost: VhostName("__defaultVhost__".into()),
            app: AppName("live app".into()),
            stream: StreamName("cam 1".into()),
            schema: None,
        };
        let urls = resolver
            .resolve_urls(
                &MediaRequestContext::default(),
                &key,
                &[MediaSchema::HttpFlv],
            )
            .await
            .expect("resolve");
        assert!(urls[0].url.contains("live%20app"));
        assert!(urls[0].url.contains("cam%201"));
    }
}
