use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_media_api::command::{
    DeleteRecordRequest, DeleteSnapshotRequest, MediaQuery, RecordFileQuery, RecordPlaybackCommand,
    RecordTaskQuery, SessionQuery, SnapshotRequest, StartRecordRequest, StopRecordRequest,
};
use cheetah_media_api::ids::{MediaKey, RecordFileId, SessionId};
use cheetah_media_api::model::{CloseReason, RecordTaskState};
use cheetah_media_api::port::{MediaControlApi, MediaRequestContext, RecordApi, SnapshotApi};
use cheetah_sdk::{
    ConfigEffect, EngineContext, HttpHeader, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest, ModuleState,
    SdkError,
};

use crate::error::{zlm_error_response, AdapterError};

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
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/setRecordSpeed".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/seekRecordStamp".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/controlRecordPlay".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/loadMP4File".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/getSnap".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/api/deleteSnapDirectory".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/api/downloadFile".to_string(),
            },
        ]
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        Some(Arc::new(ZlmMediaHttpService {
            ctx: self.ctx.clone()?,
        }))
    }
}

struct ZlmMediaHttpService {
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

    fn record(&self) -> Result<Arc<dyn RecordApi>, AdapterError> {
        self.ctx.media_services.record().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "record not available",
            ))
        })
    }

    fn snapshot(&self) -> Result<Arc<dyn SnapshotApi>, AdapterError> {
        self.ctx.media_services.snapshot().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "snapshot not available",
            ))
        })
    }

    fn request_context(&self, _req: &HttpRequest) -> MediaRequestContext {
        MediaRequestContext {
            request_id: cheetah_media_api::ids::RequestId("".to_string()),
            correlation_id: None,
            principal: None,
            source_adapter: "zlm".to_string(),
            trace_context: None,
            deadline: None,
        }
    }

    fn extract_params(&self, req: &HttpRequest) -> Result<serde_json::Value, AdapterError> {
        match req.method {
            HttpMethod::Get => Ok(crate::util::query_to_json(req.query.as_deref())),
            _ if req.body.is_empty() => Ok(crate::util::query_to_json(req.query.as_deref())),
            _ => Ok(serde_json::from_slice(&req.body)?),
        }
    }

    fn parse_media_key(&self, params: &serde_json::Value) -> Result<MediaKey, AdapterError> {
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
        let record_api = self.record()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = zlm_record_format(&params["type"])?;
        let request = StartRecordRequest {
            media_key: key,
            format: format.clone(),
            template: cheetah_media_api::model::RecordTemplate::Continuous,
            segment_duration_ms: None,
            max_segments: None,
            storage_policy: cheetah_media_api::model::StoragePolicy::default(),
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
        let record_api = self.record()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = zlm_record_format(&params["type"])?;
        let query = RecordTaskQuery {
            vhost: Some(key.vhost.0.clone()),
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            state: Some(RecordTaskState::Running),
            ..Default::default()
        };
        let page = record_api.query_record_tasks(&ctx, query).await?;
        let task = page
            .items
            .into_iter()
            .find(|t| t.format == format)
            .ok_or_else(|| {
                AdapterError::Media(cheetah_media_api::error::MediaError::not_found(
                    "running record task",
                ))
            })?;
        record_api
            .stop_record(
                &ctx,
                StopRecordRequest {
                    task_id: task.task_id,
                },
            )
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn is_recording(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = zlm_record_format(&params["type"])?;
        let query = RecordTaskQuery {
            vhost: Some(key.vhost.0.clone()),
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
        let record_api = self.record()?;
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
        let record_api = self.record()?;
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

    async fn set_record_speed(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let file_id = parse_zlm_file_id(&params)?;
        let value = parse_zlm_playback_value(&params, &["speed", "scale", "value"])?;
        record_api
            .control_record_playback(
                &ctx,
                &RecordFileId(file_id),
                RecordPlaybackCommand::Scale { value },
            )
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn seek_record_stamp(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let file_id = parse_zlm_file_id(&params)?;
        let value = parse_zlm_playback_value(&params, &["stamp", "seek", "value"])?;
        record_api
            .control_record_playback(
                &ctx,
                &RecordFileId(file_id),
                RecordPlaybackCommand::Seek {
                    value: value as i64,
                },
            )
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn control_record_play(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let file_id = parse_zlm_file_id(&params)?;
        let command = parse_zlm_playback_command(&params)?;
        record_api
            .control_record_playback(&ctx, &RecordFileId(file_id), command)
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn load_mp4_file(&self, _req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        Err(AdapterError::Media(
            cheetah_media_api::error::MediaError::unsupported_capability("vod"),
        ))
    }

    async fn get_snap(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let snapshot_api = self.snapshot()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let timeout_ms = parse_zlm_timeout_ms(&params);
        let format = params["format"].as_str().unwrap_or("jpg").to_string();
        let quality = params["quality"]
            .as_u64()
            .or_else(|| params["scale"].as_u64())
            .map(|v| v.min(100) as u8);
        let request = SnapshotRequest {
            media_key: key,
            timeout_ms,
            format,
            quality,
            storage_policy: Default::default(),
            capture_policy: Default::default(),
        };
        let handle = snapshot_api.take_snapshot(&ctx, request).await?;
        Ok(zlm_response(0, "success", handle))
    }

    async fn delete_snap_directory(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let snapshot_api = self.snapshot()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let request = DeleteSnapshotRequest {
            media_key: key,
            directory: params["directory"].as_str().map(String::from),
            retain_count: params["retain_count"].as_u64().map(|v| v as u32),
        };
        snapshot_api
            .delete_snapshot_directory(&ctx, request)
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn download_file(&self, _req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        Err(AdapterError::Media(
            cheetah_media_api::error::MediaError::unsupported_capability("file download"),
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
            (HttpMethod::Post, "/api/setRecordSpeed") => self.set_record_speed(req).await,
            (HttpMethod::Post, "/api/seekRecordStamp") => self.seek_record_stamp(req).await,
            (HttpMethod::Post, "/api/controlRecordPlay") => self.control_record_play(req).await,
            (HttpMethod::Post, "/api/loadMP4File") => self.load_mp4_file(req).await,
            (HttpMethod::Get, "/api/getSnap") => self.get_snap(req).await,
            (HttpMethod::Post, "/api/deleteSnapDirectory") => self.delete_snap_directory(req).await,
            (HttpMethod::Get, "/api/downloadFile") => self.download_file(req).await,
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

fn zlm_record_format(value: &serde_json::Value) -> Result<String, AdapterError> {
    if value.is_null() {
        return Ok("mp4".to_string());
    }
    if let Some(num) = value.as_u64() {
        let format = match num {
            0 => "mp4",
            1 => "hls",
            2 => "hls",
            3 => "mp4",
            _ => "mp4",
        };
        return Ok(format.to_string());
    }
    if let Some(s) = value.as_str() {
        if s.trim().is_empty() {
            return Ok("mp4".to_string());
        }
        return Ok(s.to_lowercase());
    }
    Ok("mp4".to_string())
}

fn parse_zlm_file_id(params: &serde_json::Value) -> Result<String, AdapterError> {
    params["file_id"]
        .as_str()
        .or_else(|| params["fileId"].as_str())
        .or_else(|| params["file_path"].as_str())
        .map(String::from)
        .ok_or_else(|| AdapterError::InvalidRequest("file_id is required".to_string()))
}

fn parse_zlm_playback_value(
    params: &serde_json::Value,
    aliases: &[&str],
) -> Result<f64, AdapterError> {
    for alias in aliases {
        if let Some(v) = params[*alias].as_f64() {
            return Ok(v);
        }
        if let Some(v) = params[*alias].as_i64() {
            return Ok(v as f64);
        }
        if let Some(v) = params[*alias].as_u64() {
            return Ok(v as f64);
        }
    }
    Err(AdapterError::InvalidRequest(
        "playback value is required".to_string(),
    ))
}

fn parse_zlm_playback_command(
    params: &serde_json::Value,
) -> Result<RecordPlaybackCommand, AdapterError> {
    let command = params["command"]
        .as_str()
        .ok_or_else(|| AdapterError::InvalidRequest("command is required".to_string()))?
        .to_lowercase();
    match command.as_str() {
        "pause" => Ok(RecordPlaybackCommand::Pause),
        "resume" => Ok(RecordPlaybackCommand::Resume),
        "scale" | "speed" => {
            let value = parse_zlm_playback_value(params, &["value", "speed", "scale"])?;
            Ok(RecordPlaybackCommand::Scale { value })
        }
        "seek" | "stamp" => {
            let value = parse_zlm_playback_value(params, &["value", "stamp", "seek"])?;
            Ok(RecordPlaybackCommand::Seek {
                value: value as i64,
            })
        }
        _ => Err(AdapterError::InvalidRequest(format!(
            "unsupported playback command {command}"
        ))),
    }
}

fn parse_zlm_timeout_ms(params: &serde_json::Value) -> u64 {
    params["timeout_sec"]
        .as_u64()
        .or_else(|| params["timeout"].as_u64())
        .map(|s| s * 1000)
        .unwrap_or(10_000)
}

fn zlm_response<T: serde::Serialize>(code: i32, msg: &str, data: T) -> HttpResponse {
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
