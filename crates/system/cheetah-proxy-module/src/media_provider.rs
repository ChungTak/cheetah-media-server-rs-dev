use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::command::{
    FfmpegProxyRequest, ProxyQuery, PullProxyRequest, PushProxyRequest,
};
use cheetah_media_api::error::{MediaError, Result};
use cheetah_media_api::ids::{MediaKey, ProxyId};
use cheetah_media_api::model::{Page, ProxyInfo, ProxyKind, ProxyState};
use cheetah_media_api::port::{MediaRequestContext, ProxyApi};
use cheetah_sdk::{EngineContext, SdkError};
use tracing::{debug, warn};

use crate::config::ProxyModuleConfig;
use crate::registry::{ProxyEntry, ProxyRegistry};
use crate::ssrf::{resolve_and_validate_url, IpNetwork};
use crate::task::{spawn_proxy_task, validate_ffmpeg_options, ProxySessionSpec};

/// Bridge that exposes the proxy registry as a [`ProxyApi`] provider.
///
/// 将代理注册表暴露为 [`ProxyApi`] provider 的桥接。
#[derive(Clone)]
pub struct ProxyMediaProvider {
    ctx: EngineContext,
    registry: Arc<ProxyRegistry>,
    config: ProxyModuleConfig,
    ssrf_allowlist: Arc<Vec<IpNetwork>>,
}

impl ProxyMediaProvider {
    /// Create a provider backed by an engine context and registry.
    ///
    /// 使用引擎上下文和注册表创建 provider。
    pub fn new(
        ctx: EngineContext,
        registry: Arc<ProxyRegistry>,
        config: ProxyModuleConfig,
    ) -> std::result::Result<Self, SdkError> {
        let ssrf_allowlist = Arc::new(
            crate::ssrf::parse_allowlist(&config.ssrf_allowlist_cidrs).map_err(|e| {
                SdkError::InvalidArgument(format!("invalid ssrf_allowlist_cidrs: {e}"))
            })?,
        );
        Ok(Self {
            ctx,
            registry,
            config,
            ssrf_allowlist,
        })
    }
}

#[async_trait]
impl ProxyApi for ProxyMediaProvider {
    async fn create_pull_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: PullProxyRequest,
    ) -> Result<ProxyInfo> {
        let resolved_source_url = resolve_and_validate_url(
            &request.source_url,
            &self.ssrf_allowlist,
            &self.ctx.runtime_api,
        )
        .await?;

        let proxy_id = self.ensure_idempotency_or_create_id(
            ctx,
            ProxyKind::Pull,
            &request.source_url,
            &request.destination,
        )?;

        if let Some(existing) = self.registry.get(&proxy_id) {
            return Ok(existing.info);
        }

        if self.registry.is_full() {
            return Err(MediaError::unavailable("proxy capacity exceeded"));
        }

        let info = build_proxy_info(
            proxy_id.clone(),
            ProxyKind::Pull,
            request.source_url,
            request.destination,
        );

        let entry = ProxyEntry {
            info: info.clone(),
            cancel: None,
        };

        if self.registry.insert(entry).is_some() {
            warn!(proxy_id = %proxy_id.0, "proxy id collision after idempotency check");
        }

        let cancel = spawn_proxy_task(
            self.ctx.clone(),
            self.registry.clone(),
            proxy_id.clone(),
            self.config.clone(),
            ProxySessionSpec::Pull {
                source_url: resolved_source_url,
                destination: info.destination.clone(),
            },
        )
        .map_err(|e| MediaError::internal(format!("failed to spawn proxy task: {e}")))?;
        self.registry.set_cancel(&proxy_id, cancel);

        debug!(proxy_id = %info.proxy_id.0, "created pull proxy");
        Ok(info)
    }

    async fn delete_pull_proxy(&self, _ctx: &MediaRequestContext, id: &ProxyId) -> Result<()> {
        delete_proxy_of_kind(&self.registry, id, ProxyKind::Pull)
    }

    async fn list_pull_proxies(
        &self,
        _ctx: &MediaRequestContext,
        mut query: ProxyQuery,
    ) -> Result<Page<ProxyInfo>> {
        query.clamp_page_size();
        list_proxies(&self.registry, query, ProxyKind::Pull)
    }

    async fn get_pull_proxy(&self, _ctx: &MediaRequestContext, id: &ProxyId) -> Result<ProxyInfo> {
        self.registry
            .get(id)
            .filter(|e| e.info.kind == ProxyKind::Pull)
            .map(|e| e.info)
            .ok_or_else(|| MediaError::not_found(format!("pull proxy not found: {}", id.0)))
    }

    async fn list_push_proxies(
        &self,
        _ctx: &MediaRequestContext,
        mut query: ProxyQuery,
    ) -> Result<Page<ProxyInfo>> {
        query.clamp_page_size();
        list_proxies(&self.registry, query, ProxyKind::Push)
    }

    async fn get_push_proxy(&self, _ctx: &MediaRequestContext, id: &ProxyId) -> Result<ProxyInfo> {
        self.registry
            .get(id)
            .filter(|e| e.info.kind == ProxyKind::Push)
            .map(|e| e.info)
            .ok_or_else(|| MediaError::not_found(format!("push proxy not found: {}", id.0)))
    }

    async fn create_push_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: PushProxyRequest,
    ) -> Result<ProxyInfo> {
        let resolved_destination_url = resolve_and_validate_url(
            &request.destination_url,
            &self.ssrf_allowlist,
            &self.ctx.runtime_api,
        )
        .await?;

        let proxy_id = self.ensure_idempotency_or_create_id(
            ctx,
            ProxyKind::Push,
            &request.destination_url,
            &request.source_media_key,
        )?;

        if let Some(existing) = self.registry.get(&proxy_id) {
            return Ok(existing.info);
        }

        if self.registry.is_full() {
            return Err(MediaError::unavailable("proxy capacity exceeded"));
        }

        // The ProxyInfo type stores the URL in `source` and the MediaKey in
        // `destination` consistently with the other fake/provider implementations.
        let info = build_proxy_info(
            proxy_id.clone(),
            ProxyKind::Push,
            request.destination_url,
            request.source_media_key,
        );

        let entry = ProxyEntry {
            info: info.clone(),
            cancel: None,
        };

        if self.registry.insert(entry).is_some() {
            warn!(proxy_id = %proxy_id.0, "proxy id collision after idempotency check");
        }

        let cancel = spawn_proxy_task(
            self.ctx.clone(),
            self.registry.clone(),
            proxy_id.clone(),
            self.config.clone(),
            ProxySessionSpec::Push {
                source_media_key: info.destination.clone(),
                destination_url: resolved_destination_url,
                protocol: request.protocol.clone(),
            },
        )
        .map_err(|e| MediaError::internal(format!("failed to spawn proxy task: {e}")))?;
        self.registry.set_cancel(&proxy_id, cancel);

        debug!(proxy_id = %info.proxy_id.0, "created push proxy");
        Ok(info)
    }

    async fn delete_push_proxy(&self, _ctx: &MediaRequestContext, id: &ProxyId) -> Result<()> {
        delete_proxy_of_kind(&self.registry, id, ProxyKind::Push)
    }

    async fn create_ffmpeg_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: FfmpegProxyRequest,
    ) -> Result<ProxyInfo> {
        let resolved_source_url = resolve_and_validate_url(
            &request.source_url,
            &self.ssrf_allowlist,
            &self.ctx.runtime_api,
        )
        .await?;
        validate_ffmpeg_options(&request.input_options, &request.output_options)
            .map_err(MediaError::invalid_argument)?;

        let proxy_id = self.ensure_idempotency_or_create_id(
            ctx,
            ProxyKind::Ffmpeg,
            &request.source_url,
            &request.destination,
        )?;

        if let Some(existing) = self.registry.get(&proxy_id) {
            return Ok(existing.info);
        }

        if self.registry.is_full() {
            return Err(MediaError::unavailable("proxy capacity exceeded"));
        }

        let info = build_proxy_info(
            proxy_id.clone(),
            ProxyKind::Ffmpeg,
            request.source_url.clone(),
            request.destination.clone(),
        );

        let entry = ProxyEntry {
            info: info.clone(),
            cancel: None,
        };

        if self.registry.insert(entry).is_some() {
            warn!(proxy_id = %proxy_id.0, "proxy id collision after idempotency check");
        }

        let job_id = format!("ffmpeg-{}", proxy_id.0);
        let cancel = spawn_proxy_task(
            self.ctx.clone(),
            self.registry.clone(),
            proxy_id.clone(),
            self.config.clone(),
            ProxySessionSpec::Ffmpeg {
                source_url: resolved_source_url,
                destination: request.destination,
                input_options: request.input_options,
                output_options: request.output_options,
                job_id,
            },
        )
        .map_err(|e| MediaError::internal(format!("failed to spawn ffmpeg proxy task: {e}")))?;
        self.registry.set_cancel(&proxy_id, cancel);

        debug!(proxy_id = %info.proxy_id.0, "created ffmpeg proxy");
        Ok(info)
    }

    async fn delete_ffmpeg_proxy(&self, _ctx: &MediaRequestContext, id: &ProxyId) -> Result<()> {
        delete_proxy_of_kind(&self.registry, id, ProxyKind::Ffmpeg)
    }

    async fn get_ffmpeg_proxy(
        &self,
        _ctx: &MediaRequestContext,
        id: &ProxyId,
    ) -> Result<ProxyInfo> {
        self.registry
            .get(id)
            .filter(|e| e.info.kind == ProxyKind::Ffmpeg)
            .map(|e| e.info)
            .ok_or_else(|| MediaError::not_found(format!("ffmpeg proxy not found: {}", id.0)))
    }

    async fn list_ffmpeg_proxies(
        &self,
        _ctx: &MediaRequestContext,
        mut query: ProxyQuery,
    ) -> Result<Page<ProxyInfo>> {
        query.clamp_page_size();
        list_proxies(&self.registry, query, ProxyKind::Ffmpeg)
    }
}

impl ProxyMediaProvider {
    fn ensure_idempotency_or_create_id(
        &self,
        ctx: &MediaRequestContext,
        kind: ProxyKind,
        source: &str,
        destination: &MediaKey,
    ) -> Result<ProxyId> {
        if let Some(key) = ctx.idempotency_key.as_ref().filter(|k| !k.is_empty()) {
            let id = ProxyId(key.clone());
            if let Some(existing) = self.registry.get(&id) {
                if existing.info.kind != kind
                    || existing.info.source != source
                    || existing.info.destination != *destination
                {
                    return Err(MediaError::already_exists(format!(
                        "idempotency key {id} reused with different parameters",
                        id = id.0
                    )));
                }
            }
            Ok(id)
        } else {
            Ok(self.registry.generate_id())
        }
    }
}

fn list_proxies(
    registry: &ProxyRegistry,
    query: ProxyQuery,
    kind: ProxyKind,
) -> Result<Page<ProxyInfo>> {
    let mut q = query.clone();
    q.kind = Some(kind);
    let (items, total) = registry.query(&q);

    Ok(Page {
        items,
        page: query.page.max(1),
        page_size: query.page_size,
        total,
        next_cursor: None,
    })
}

fn delete_proxy(registry: &ProxyRegistry, id: &ProxyId) -> Result<()> {
    if !registry.cancel(id) {
        return Err(MediaError::not_found(format!("proxy not found: {}", id.0)));
    }
    registry.remove(id);
    debug!(proxy_id = %id.0, "deleted proxy");
    Ok(())
}

fn delete_proxy_of_kind(registry: &ProxyRegistry, id: &ProxyId, kind: ProxyKind) -> Result<()> {
    if registry.get(id).filter(|e| e.info.kind == kind).is_none() {
        return Err(MediaError::not_found(format!(
            "{kind:?} proxy not found: {}",
            id.0
        )));
    }
    delete_proxy(registry, id)
}

fn build_proxy_info(
    proxy_id: ProxyId,
    kind: ProxyKind,
    source: String,
    destination: MediaKey,
) -> ProxyInfo {
    let now = now_unix_millis();
    ProxyInfo {
        proxy_id,
        kind,
        source,
        destination,
        state: ProxyState::Created,
        retry_count: 0,
        last_error: None,
        created_at: now,
        updated_at: now,
        output_urls: Vec::new(),
    }
}

/// Returns the current unix timestamp in milliseconds.
fn now_unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
