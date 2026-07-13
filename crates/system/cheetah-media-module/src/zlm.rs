use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_media_api::command::{
    DeleteRecordRequest, FfmpegProxyRequest, MediaQuery, ProxyQuery, PullProxyRequest,
    RecordFileQuery, RecordTaskQuery, SessionQuery, StartRecordRequest, StopRecordRequest,
};
use cheetah_media_api::ids::{MediaKey, ProxyId, RecordTaskId, SessionId, StreamKeyBridge};
use cheetah_media_api::model::{
    CloseReason, ProxyKind, RecordTaskState, RecordTemplate, ServerConfig, StoragePolicy,
};
use cheetah_media_api::port::{
    MediaControlApi, MediaRequestContext, ProxyApi, RtpApi, ServerAdminApi,
};
use cheetah_sdk::{
    ConfigEffect, EngineContext, HttpHeader, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest, ModuleState,
    SdkError,
};

use crate::error::{zlm_error_response, AdapterError};

mod rtp;

const MODULE_ID: &str = "media-http-zlm";

/// Factory for the ZLMediaKit-compatible HTTP adapter module.
///
/// ZLMediaKit 兼容 HTTP adapter 模块工厂。
pub struct ZlmMediaModuleFactory;

impl ModuleFactory for ZlmMediaModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "ZLM Media HTTP Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "media.zlm".to_string(),
            routes_prefix: "/index".to_string(),
            capabilities: vec![ModuleCapability::HttpApi],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(ZlmMediaModule::new())
    }
}

/// ZLMediaKit-compatible media HTTP adapter module.
///
/// ZLMediaKit 兼容媒体 HTTP adapter 模块。
pub struct ZlmMediaModule {
    state: ModuleState,
    ctx: Option<EngineContext>,
}

impl ZlmMediaModule {
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            ctx: None,
        }
    }
}

impl Default for ZlmMediaModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for ZlmMediaModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "ZLM Media HTTP Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.ctx = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, _cancel: cheetah_sdk::CancellationToken) -> Result<(), SdkError> {
        self.state = ModuleState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(
        &mut self,
        _change: ModuleConfigChange,
    ) -> Result<ConfigEffect, SdkError> {
        Ok(ConfigEffect::Immediate)
    }

    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        vec![
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/getMediaList".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/isMediaOnline".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/getMediaInfo".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/getAllSession".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/close_stream".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/kick_session".to_string(),
            },
            // Record endpoints; detailed implementation in record module / future media provider.
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/startRecord".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/stopRecord".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/isRecording".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/getMP4RecordFile".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/deleteRecordDirectory".to_string(),
            },
            // RTP endpoints
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/openRtpServer".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/closeRtpServer".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/startSendRtp".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/stopSendRtp".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/getRtpInfo".to_string(),
            },
            // Proxy endpoints
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/addStreamProxy".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/delStreamProxy".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/getAllStreamProxy".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/addFFmpegSource".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/delFFmpegSource".to_string(),
            },
            // Server ops endpoints
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/getServerLoad".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/getWorkThreadsLoad".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/getServerConfig".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/setServerConfig".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/restartServer".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/shutdownServer".to_string(),
            },
        ]
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        Some(Arc::new(ZlmMediaHttpService {
            ctx: self.ctx.clone()?,
        }))
    }
}

pub(crate) struct ZlmMediaHttpService {
    ctx: EngineContext,
}

impl ZlmMediaHttpService {
    fn control(&self) -> Result<Arc<dyn MediaControlApi>, AdapterError> {
        self.ctx.media_services.control().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "media control not available",
            ))
        })
    }

    fn proxy(&self) -> Result<Arc<dyn ProxyApi>, AdapterError> {
        self.ctx.media_services.proxy().ok_or_else(|| {
            AdapterError::Media(
                cheetah_media_api::error::MediaError::unsupported_capability("proxy"),
            )
        })
    }

    pub(crate) fn rtp(&self) -> Result<Arc<dyn RtpApi>, AdapterError> {
        self.ctx.media_services.rtp().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "rtp not available",
            ))
        })
    }

    fn server_admin(&self) -> Result<Arc<dyn ServerAdminApi>, AdapterError> {
        self.ctx.media_services.server_admin().ok_or_else(|| {
            AdapterError::Media(
                cheetah_media_api::error::MediaError::unsupported_capability("server_admin"),
            )
        })
    }

    pub(crate) fn require_principal(&self, ctx: &MediaRequestContext) -> Result<(), AdapterError> {
        let global = self.ctx.config_provider.global();
        let Some(expected) = global
            .get("media")
            .and_then(|m| m.get("api_secret"))
            .and_then(serde_json::Value::as_str)
        else {
            return Err(AdapterError::Media(
                cheetah_media_api::error::MediaError::unauthenticated(
                    "server admin authentication not configured",
                ),
            ));
        };
        match ctx.principal.as_deref() {
            Some(token) if crate::util::constant_time_eq(token, expected) => Ok(()),
            _ => Err(AdapterError::Media(
                cheetah_media_api::error::MediaError::unauthenticated(
                    "server admin requires valid authentication",
                ),
            )),
        }
    }

    pub(crate) fn request_context(&self, req: &HttpRequest) -> MediaRequestContext {
        let principal = req
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("authorization"))
            .and_then(|h| {
                let value = h.value.trim();
                value.strip_prefix("Bearer ").map(|t| t.trim().to_string())
            });
        MediaRequestContext {
            request_id: cheetah_media_api::ids::RequestId("".to_string()),
            correlation_id: None,
            principal,
            source_adapter: "zlm".to_string(),
            trace_context: None,
            deadline: None,
        }
    }

    pub(crate) fn extract_params(
        &self,
        req: &HttpRequest,
    ) -> Result<serde_json::Value, AdapterError> {
        match req.method {
            HttpMethod::Get => Ok(crate::util::query_to_json(req.query.as_deref())),
            _ if req.body.is_empty() => Ok(crate::util::query_to_json(req.query.as_deref())),
            _ => Ok(serde_json::from_slice(&req.body)?),
        }
    }

    pub(crate) fn parse_media_key(
        &self,
        params: &serde_json::Value,
    ) -> Result<MediaKey, AdapterError> {
        let vhost = params["vhost"].as_str().unwrap_or("__defaultVhost__");
        let app = params["app"]
            .as_str()
            .ok_or_else(|| AdapterError::InvalidRequest("app is required".to_string()))?;
        let stream = params["stream"]
            .as_str()
            .or_else(|| params["stream_id"].as_str())
            .ok_or_else(|| AdapterError::InvalidRequest("stream is required".to_string()))?;
        MediaKey::new(vhost, app, stream, None).map_err(AdapterError::Media)
    }

    async fn get_media_list(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let query = MediaQuery {
            vhost: params["vhost"].as_str().map(String::from),
            app: params["app"].as_str().map(String::from),
            stream: params["stream"].as_str().map(String::from),
            schema: params["schema"].as_str().map(String::from),
            ..Default::default()
        };
        let page = self.control()?.get_media_list(&ctx, query).await?;
        Ok(zlm_response(0, "success", page))
    }

    async fn is_media_online(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let online = self.control()?.is_media_online(&ctx, &key).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({ "online": online == cheetah_media_api::model::OnlineState::Online }),
        ))
    }

    async fn get_media_info(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let info = self.control()?.get_media(&ctx, &key).await?;
        Ok(zlm_response(0, "success", info))
    }

    async fn get_all_session(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let query = SessionQuery {
            vhost: params["vhost"].as_str().map(String::from),
            app: params["app"].as_str().map(String::from),
            stream: params["stream"].as_str().map(String::from),
            ..Default::default()
        };
        let page = self.control()?.list_sessions(&ctx, query).await?;
        Ok(zlm_response(0, "success", page))
    }

    async fn close_stream(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let _ = self
            .control()?
            .kick_stream(&ctx, &key, CloseReason::Kicked)
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn kick_session(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let id = params["id"]
            .as_str()
            .ok_or_else(|| AdapterError::InvalidRequest("id is required".to_string()))?;
        self.control()?
            .kick_session(&ctx, &SessionId(id.to_string()), CloseReason::Kicked)
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn record_start(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let record_api = self.ctx.media_services.record().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "record not available",
            ))
        })?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = params["type"].as_str().unwrap_or("mp4");
        let request = StartRecordRequest {
            media_key: key,
            format: format.to_string(),
            template: RecordTemplate::Continuous,
            segment_duration_ms: None,
            max_segments: None,
            storage_policy: StoragePolicy::default(),
            idempotency_key: None,
        };
        let task = record_api.start_record(&ctx, request).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true, "taskId": task.task_id.0}),
        ))
    }

    async fn record_stop(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let record_api = self.ctx.media_services.record().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "record not available",
            ))
        })?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = params["type"].as_str().unwrap_or("mp4");
        let request = StopRecordRequest {
            task_id: RecordTaskId(format!("{format}-{}-{}", key.app.0, key.stream.0)),
        };
        let _ = record_api.stop_record(&ctx, request).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn is_recording(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let record_api = self.ctx.media_services.record().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "record not available",
            ))
        })?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = params["type"].as_str().unwrap_or("mp4");
        let query = RecordTaskQuery {
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            ..Default::default()
        };
        let page = record_api.query_record_tasks(&ctx, query).await?;
        let recording = page.items.iter().any(|t| {
            t.format == format
                && matches!(t.state, RecordTaskState::Running | RecordTaskState::Pending)
        });
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"status": recording}),
        ))
    }

    async fn get_mp4_files(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let record_api = self.ctx.media_services.record().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "record not available",
            ))
        })?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let query = RecordFileQuery {
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            format: Some("mp4".to_string()),
            ..Default::default()
        };
        let page = record_api.query_record_files(&ctx, query).await?;
        let paths: Vec<String> = page.items.iter().map(|f| f.path_handle.0.clone()).collect();
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"paths": paths, "rootPath": ""}),
        ))
    }

    async fn delete_record_directory(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.ctx.media_services.record().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "record not available",
            ))
        })?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let query = RecordFileQuery {
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            page_size: RecordFileQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        let mut total_deleted = 0usize;
        let mut total_failed = 0usize;
        loop {
            let page = record_api.query_record_files(&ctx, query.clone()).await?;
            if page.items.is_empty() {
                break;
            }
            let mut page_deleted = 0usize;
            for f in &page.items {
                match record_api
                    .delete_record_file(
                        &ctx,
                        DeleteRecordRequest {
                            file_id: f.file_id.clone(),
                        },
                    )
                    .await
                {
                    Ok(()) => {
                        page_deleted += 1;
                        total_deleted += 1;
                    }
                    Err(_) => {
                        total_failed += 1;
                    }
                }
            }
            if (page.items.len() as u64) < query.page_size || page_deleted == 0 {
                break;
            }
        }
        let result = total_failed == 0;
        let data = serde_json::json!({
            "result": result,
            "deleted": total_deleted,
            "failed": total_failed,
        });
        Ok(zlm_response(
            0,
            if result { "success" } else { "partial success" },
            data,
        ))
    }

    async fn get_all_stream_proxy(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let mut query = ProxyQuery {
            kind: Some(ProxyKind::Pull),
            ..Default::default()
        };
        query.page_size =
            crate::util::parse_json_u64(&params["page_size"]).unwrap_or(ProxyQuery::MAX_PAGE_SIZE);
        query.page = crate::util::parse_json_u64(&params["page"]).unwrap_or(0);
        query.clamp_page_size();
        let page = proxy_api.list_proxies(&ctx, query).await?;
        Ok(zlm_response(0, "success", page.items))
    }

    async fn add_stream_proxy(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let source_url = params["url"]
            .as_str()
            .ok_or_else(|| AdapterError::InvalidRequest("url is required".to_string()))?;
        crate::util::validate_ffmpeg_url(source_url)?;
        let request = PullProxyRequest {
            source_url: source_url.to_string(),
            destination: key.clone(),
            retry_policy: Default::default(),
            heartbeat_ms: crate::util::parse_json_u64(&params["heartbeat_ms"]),
            timeout_ms: crate::util::parse_json_u64(&params["timeout_ms"]).unwrap_or(10_000),
            transcode_policy: Default::default(),
            output_policy: Default::default(),
            record_policy: None,
        };
        let info = proxy_api.create_pull_proxy(&ctx, request).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({
                "key": zlm_key_string(&key),
                "proxy_id": info.proxy_id.0,
                "result": true,
            }),
        ))
    }

    async fn del_stream_proxy(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let proxy_id = ProxyId(zlm_key_string(&key));
        proxy_api.delete_pull_proxy(&ctx, &proxy_id).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn add_ffmpeg_source(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let (source_url, input_options, output_options) = crate::util::parse_ffmpeg_request(
            params["ffmpeg_cmd"].as_str(),
            params["src_url"].as_str(),
        )?;
        crate::util::validate_ffmpeg_options(&input_options)?;
        crate::util::validate_ffmpeg_options(&output_options)?;
        let request = FfmpegProxyRequest {
            source_url,
            destination: key.clone(),
            timeout_ms: crate::util::parse_json_u64(&params["timeout_ms"]).unwrap_or(0),
            input_options,
            output_options,
            transcode_policy: Default::default(),
            output_policy: Default::default(),
        };
        let info = proxy_api.create_ffmpeg_proxy(&ctx, request).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({
                "key": zlm_key_string(&key),
                "proxy_id": info.proxy_id.0,
                "result": true,
            }),
        ))
    }

    async fn del_ffmpeg_source(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let proxy_id = ProxyId(zlm_key_string(&key));
        proxy_api.delete_ffmpeg_proxy(&ctx, &proxy_id).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn get_server_load(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let info = api.server_info(&ctx).await?;
        let data = serde_json::json!({
            "cpu": info.load.cpu_percent,
            "mem": info.load.memory_bytes,
            "net_in": info.load.network_in,
            "net_out": info.load.network_out,
        });
        Ok(zlm_response(0, "success", data))
    }

    async fn get_work_threads_load(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let info = api.server_info(&ctx).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({ "threads": info.load.threads }),
        ))
    }

    async fn get_server_config(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let mut config = api.server_config(&ctx).await?;
        crate::util::filter_sensitive_config_values(&mut config.values);
        Ok(zlm_response(0, "success", config.values))
    }

    async fn set_server_config(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let params = self.extract_params(&req)?;
        let mut values = std::collections::HashMap::new();
        if let (Some(key), Some(value)) = (params["key"].as_str(), params["value"].as_str()) {
            if !crate::util::is_sensitive_config_key(key) {
                values.insert(key.to_string(), value.to_string());
            }
        } else if let Some(obj) = params.as_object() {
            for (k, v) in obj {
                if k == "restart" || crate::util::is_sensitive_config_key(k) {
                    continue;
                }
                if let Some(s) = v.as_str() {
                    values.insert(k.clone(), s.to_string());
                }
            }
        }
        let config = ServerConfig { values };
        api.set_server_config(&ctx, config).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn restart_server(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        api.restart_server(&ctx).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn shutdown_server(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        api.shutdown_server(&ctx).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }
}

#[async_trait]
impl ModuleHttpService for ZlmMediaHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let result = match (req.method, req.path.as_str()) {
            (HttpMethod::Get, "/api/getMediaList") => self.get_media_list(req).await,
            (HttpMethod::Get, "/api/isMediaOnline") => self.is_media_online(req).await,
            (HttpMethod::Get, "/api/getMediaInfo") => self.get_media_info(req).await,
            (HttpMethod::Get, "/api/getAllSession") => self.get_all_session(req).await,
            (HttpMethod::Post, "/api/close_stream") => self.close_stream(req).await,
            (HttpMethod::Post, "/api/kick_session") => self.kick_session(req).await,
            (HttpMethod::Post, "/api/startRecord") => self.record_start(req).await,
            (HttpMethod::Post, "/api/stopRecord") => self.record_stop(req).await,
            (HttpMethod::Get, "/api/isRecording") => self.is_recording(req).await,
            (HttpMethod::Get, "/api/getMP4RecordFile") => self.get_mp4_files(req).await,
            (HttpMethod::Post, "/api/deleteRecordDirectory") => {
                self.delete_record_directory(req).await
            }
            (HttpMethod::Post, "/api/openRtpServer") => self.open_rtp_server(req).await,
            (HttpMethod::Post, "/api/closeRtpServer") => self.close_rtp_server(req).await,
            (HttpMethod::Post, "/api/startSendRtp") => self.start_send_rtp(req).await,
            (HttpMethod::Post, "/api/stopSendRtp") => self.stop_send_rtp(req).await,
            (HttpMethod::Get, "/api/getRtpInfo") => self.get_rtp_info(req).await,
            (HttpMethod::Post, "/api/addStreamProxy") => self.add_stream_proxy(req).await,
            (HttpMethod::Post, "/api/delStreamProxy") => self.del_stream_proxy(req).await,
            (HttpMethod::Get, "/api/getAllStreamProxy") => self.get_all_stream_proxy(req).await,
            (HttpMethod::Post, "/api/addFFmpegSource") => self.add_ffmpeg_source(req).await,
            (HttpMethod::Post, "/api/delFFmpegSource") => self.del_ffmpeg_source(req).await,
            (HttpMethod::Get, "/api/getServerLoad") => self.get_server_load(req).await,
            (HttpMethod::Get, "/api/getWorkThreadsLoad") => self.get_work_threads_load(req).await,
            (HttpMethod::Get, "/api/getServerConfig") => self.get_server_config(req).await,
            (HttpMethod::Post, "/api/setServerConfig") => self.set_server_config(req).await,
            (HttpMethod::Post, "/api/restartServer") => self.restart_server(req).await,
            (HttpMethod::Post, "/api/shutdownServer") => self.shutdown_server(req).await,
            _ => Err(AdapterError::InvalidRequest("not found".to_string())),
        };

        match result {
            Ok(resp) => Ok(resp),
            Err(AdapterError::Media(err)) => {
                let body = zlm_error_response(&err);
                Ok(zlm_json_response(body))
            }
            Err(AdapterError::InvalidRequest(msg)) => {
                let body = zlm_error_response(
                    &cheetah_media_api::error::MediaError::invalid_argument(msg),
                );
                Ok(zlm_json_response(body))
            }
            Err(AdapterError::Serialization(msg)) => {
                let body = zlm_error_response(&cheetah_media_api::error::MediaError::internal(msg));
                Ok(zlm_json_response(body))
            }
        }
    }
}

pub(crate) fn parse_zlm_u16(
    params: &serde_json::Value,
    key: &str,
) -> Result<Option<u16>, AdapterError> {
    if params[key].is_null() {
        return Ok(None);
    }
    let v = crate::util::parse_json_u64(&params[key])
        .ok_or_else(|| AdapterError::InvalidRequest(format!("{key} is not a valid number")))?;
    u16::try_from(v)
        .map(Some)
        .map_err(|_| AdapterError::InvalidRequest(format!("{key} is out of range")))
}

pub(crate) fn parse_zlm_u32(
    params: &serde_json::Value,
    key: &str,
) -> Result<Option<u32>, AdapterError> {
    if params[key].is_null() {
        return Ok(None);
    }
    let v = crate::util::parse_json_u64(&params[key])
        .ok_or_else(|| AdapterError::InvalidRequest(format!("{key} is not a valid number")))?;
    u32::try_from(v)
        .map(Some)
        .map_err(|_| AdapterError::InvalidRequest(format!("{key} is out of range")))
}

pub(crate) fn parse_zlm_u8(
    params: &serde_json::Value,
    key: &str,
) -> Result<Option<u8>, AdapterError> {
    if params[key].is_null() {
        return Ok(None);
    }
    let v = crate::util::parse_json_u64(&params[key])
        .ok_or_else(|| AdapterError::InvalidRequest(format!("{key} is not a valid number")))?;
    u8::try_from(v)
        .map(Some)
        .map_err(|_| AdapterError::InvalidRequest(format!("{key} is out of range")))
}

pub(crate) fn zlm_response<T: serde::Serialize>(code: i32, msg: &str, data: T) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![HttpHeader {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        }],
        body: Bytes::from(
            serde_json::to_vec(&serde_json::json!({
                "code": code,
                "msg": msg,
                "data": data,
            }))
            .unwrap_or_default(),
        ),
    }
}

fn zlm_json_response(params: serde_json::Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![HttpHeader {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        }],
        body: Bytes::from(serde_json::to_vec(&params).unwrap_or_default()),
    }
}

pub(crate) fn zlm_key_string(key: &MediaKey) -> String {
    let (namespace, path) = StreamKeyBridge::to_namespace_path(key);
    format!("{namespace}/{path}")
}
