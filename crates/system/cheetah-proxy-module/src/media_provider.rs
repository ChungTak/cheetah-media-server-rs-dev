//! `ProxyApi` implementation backed by the in-memory proxy registry.
//!
//! 由内存代理注册表支持的 `ProxyApi` 实现。

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::command::{
    FfmpegProxyRequest, ProxyQuery, PullProxyRequest, PushProxyRequest,
};
use cheetah_media_api::error::{MediaError, MediaErrorCode, Result};
use cheetah_media_api::event::{EventHeader, MediaEvent, MediaEventSender};
use cheetah_media_api::ids::{MediaKey, ProxyId};
use cheetah_media_api::model::Page;
use cheetah_media_api::model::{ProxyInfo, ProxyKind, ProxyState};
use cheetah_media_api::port::{MediaRequestContext, ProxyApi};
use cheetah_sdk::connector::ConnectorDirection;
use cheetah_sdk::{ConnectorApi, EngineContext};

use crate::registry::ProxyRegistry;

/// Provider that implements the media-domain `ProxyApi`.
///
/// 实现媒体领域 `ProxyApi` 的 Provider。
pub struct ProxyMediaProvider {
    registry: ProxyRegistry,
    connector_api: Option<Arc<dyn ConnectorApi>>,
    media_event_sender: Option<Arc<dyn MediaEventSender>>,
}

impl ProxyMediaProvider {
    /// Create a provider from the engine context and config.
    pub fn new(ctx: &EngineContext, config: &crate::config::ProxyModuleConfig) -> Self {
        Self {
            registry: ProxyRegistry::new(config.max_total_proxies),
            connector_api: ctx.connector_api.clone(),
            media_event_sender: Some(ctx.media_event_sender.clone()),
        }
    }

    /// Create from an explicit registry and connector for tests.
    #[cfg(test)]
    fn with_registry(
        registry: ProxyRegistry,
        connector_api: Option<Arc<dyn ConnectorApi>>,
    ) -> Self {
        Self {
            registry,
            connector_api,
            media_event_sender: None,
        }
    }

    fn validate_url(url: &str) -> Result<()> {
        if url.is_empty() {
            return Err(MediaError::invalid_argument("source URL is empty"));
        }
        if !url.contains("://") {
            return Err(MediaError::invalid_argument(format!(
                "invalid URL, missing scheme: {url}"
            )));
        }
        Ok(())
    }

    fn connector(&self) -> Result<&dyn ConnectorApi> {
        self.connector_api
            .as_deref()
            .ok_or_else(|| MediaError::unavailable("connector api not configured"))
    }

    fn check_protocol_support(&self, url: &str, direction: ConnectorDirection) -> Result<()> {
        let scheme = url.split("://").next().unwrap_or("").to_ascii_lowercase();
        let connector = self.connector()?;
        if connector.supports(&scheme, direction) {
            Ok(())
        } else {
            Err(MediaError::new(
                MediaErrorCode::Unsupported,
                format!("unsupported proxy protocol: {scheme}"),
            ))
        }
    }

    fn build_proxy_info(
        &self,
        kind: ProxyKind,
        source: &str,
        destination: &MediaKey,
    ) -> Result<ProxyInfo> {
        let now = now_ms();
        Ok(ProxyInfo {
            proxy_id: ProxyId(format!("proxy-{}", generate_id())),
            kind,
            source: source.to_string(),
            destination: destination.clone(),
            state: ProxyState::Created,
            retry_count: 0,
            last_error: None,
            created_at: now,
            updated_at: now,
            output_urls: Vec::new(),
        })
    }

    fn upsert_and_emit(&self, info: ProxyInfo) -> ProxyInfo {
        let inserted = self.registry.upsert_idempotent(info.clone());
        if inserted.proxy_id == info.proxy_id {
            self.emit_state_changed(&inserted);
        }
        inserted
    }

    fn emit_state_changed(&self, info: &ProxyInfo) {
        if let Some(sender) = &self.media_event_sender {
            let header = EventHeader {
                event_id: generate_id(),
                occurred_at: now_ms(),
                sequence: None,
                media_key: Some(info.destination.clone()),
                source: info.source.clone(),
                correlation_id: None,
            };
            let _ = sender.send(MediaEvent::ProxyStateChanged(
                cheetah_media_api::event::ProxyStateChanged {
                    header,
                    proxy_id: info.proxy_id.clone(),
                    state: info.state,
                    last_error: info.last_error.clone(),
                },
            ));
        }
    }
}

#[async_trait]
impl ProxyApi for ProxyMediaProvider {
    async fn create_pull_proxy(
        &self,
        _ctx: &MediaRequestContext,
        request: PullProxyRequest,
    ) -> Result<ProxyInfo> {
        Self::validate_url(&request.source_url)?;
        self.check_protocol_support(&request.source_url, ConnectorDirection::Pull)?;
        let info =
            self.build_proxy_info(ProxyKind::Pull, &request.source_url, &request.destination)?;
        Ok(self.upsert_and_emit(info))
    }

    async fn delete_pull_proxy(&self, _ctx: &MediaRequestContext, id: &ProxyId) -> Result<()> {
        if self.registry.delete(id) {
            Ok(())
        } else {
            Err(MediaError::not_found(format!("pull proxy {id}")))
        }
    }

    async fn list_pull_proxies(
        &self,
        _ctx: &MediaRequestContext,
        mut query: ProxyQuery,
    ) -> Result<Page<ProxyInfo>> {
        query.clamp_page_size();
        let all = self.registry.list(query.kind, query.state);
        let total = all.len() as u64;
        let page = query.page.max(1);
        let start = ((page - 1).saturating_mul(query.page_size)) as usize;
        let items = all
            .into_iter()
            .skip(start)
            .take(query.page_size as usize)
            .collect();
        Ok(Page {
            items,
            total,
            page,
            page_size: query.page_size,
            next_cursor: None,
        })
    }

    async fn create_push_proxy(
        &self,
        _ctx: &MediaRequestContext,
        request: PushProxyRequest,
    ) -> Result<ProxyInfo> {
        Self::validate_url(&request.destination_url)?;
        self.check_protocol_support(&request.destination_url, ConnectorDirection::Push)?;
        let info = self.build_proxy_info(
            ProxyKind::Push,
            &request.destination_url,
            &request.source_media_key,
        )?;
        Ok(self.upsert_and_emit(info))
    }

    async fn delete_push_proxy(&self, _ctx: &MediaRequestContext, id: &ProxyId) -> Result<()> {
        if self.registry.delete(id) {
            Ok(())
        } else {
            Err(MediaError::not_found(format!("push proxy {id}")))
        }
    }

    async fn create_ffmpeg_proxy(
        &self,
        _ctx: &MediaRequestContext,
        request: FfmpegProxyRequest,
    ) -> Result<ProxyInfo> {
        Self::validate_url(&request.source_url)?;
        self.check_protocol_support(&request.source_url, ConnectorDirection::Pull)?;
        let info =
            self.build_proxy_info(ProxyKind::Ffmpeg, &request.source_url, &request.destination)?;
        Ok(self.upsert_and_emit(info))
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn generate_id() -> String {
    let mut buf = [0u8; 8];
    if getrandom::getrandom(&mut buf).is_err() {
        // Fall back to a timestamp + counter based id if getrandom fails.
        let now = now_ms() as u64;
        return format!("{:x}{:x}", now, buf[0]);
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_media_api::ids::MediaKey;

    fn fake_key(stream: &str) -> MediaKey {
        MediaKey::with_default_vhost("live", stream, None).unwrap()
    }

    fn provider() -> ProxyMediaProvider {
        ProxyMediaProvider::with_registry(ProxyRegistry::new(10), None)
    }

    #[tokio::test]
    async fn create_pull_proxy_rejects_invalid_url() {
        let p = provider();
        let req = PullProxyRequest {
            source_url: "not-a-url".to_string(),
            destination: fake_key("s"),
            retry_policy: Default::default(),
            heartbeat_ms: None,
            timeout_ms: 30_000,
            transcode_policy: Default::default(),
            output_policy: Default::default(),
            record_policy: None,
        };
        let ctx = MediaRequestContext::default();
        let err = p.create_pull_proxy(&ctx, req).await.unwrap_err();
        assert_eq!(err.code, MediaErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn create_pull_proxy_without_connector_is_unavailable() {
        let p = provider();
        let req = PullProxyRequest {
            source_url: "rtsp://example/stream".to_string(),
            destination: fake_key("s"),
            retry_policy: Default::default(),
            heartbeat_ms: None,
            timeout_ms: 30_000,
            transcode_policy: Default::default(),
            output_policy: Default::default(),
            record_policy: None,
        };
        let ctx = MediaRequestContext::default();
        let err = p.create_pull_proxy(&ctx, req).await.unwrap_err();
        assert_eq!(err.code, MediaErrorCode::Unavailable);
    }
}
