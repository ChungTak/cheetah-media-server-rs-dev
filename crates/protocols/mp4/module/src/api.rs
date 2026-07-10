//! SMS-style VOD HTTP API request/response shapes.

use std::path::PathBuf;
use std::sync::Arc;

use cheetah_mp4_core::VodControlCommand;
use cheetah_mp4_driver_tokio::{
    open_file, open_files, VodDriverConfig, VodDriverEvent, VodEventStream,
};
use cheetah_sdk::{CoreAdaptersApi, RuntimeApi, StreamKey};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::config::Mp4ModuleConfig;
use crate::session_registry::{SessionError, VodSessionRecord, VodSessionRegistry};

/// Error returned by `Vod API` operations.
/// `Vod API` 操作返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum VodApiError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("session error: {0}")]
    Session(#[from] SessionError),
    #[error("driver error: {0}")]
    Driver(String),
    #[error("file not found: {0}")]
    NotFound(String),
}

/// Request for `Start Vod`.
/// `Start Vod` 的请求。
#[derive(Debug, Clone, Deserialize)]
pub struct StartVodRequest {
    pub uri: String,
    #[serde(rename = "format", default)]
    pub format: Option<String>,
    #[serde(rename = "startTime", default)]
    pub start_time_ms: Option<i64>,
    #[serde(rename = "endTime", default)]
    pub end_time_ms: Option<i64>,
    #[serde(rename = "loopCount", default)]
    pub loop_count: Option<u32>,
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
}

/// Response for `Start Vod`.
/// `Start Vod` 的响应。
#[derive(Debug, Clone, Serialize)]
pub struct StartVodResponse {
    pub code: u16,
    pub msg: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

/// Request for `Control Vod`.
/// `Control Vod` 的请求。
#[derive(Debug, Clone, Deserialize)]
pub struct ControlVodRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(default)]
    pub seek: Option<i64>,
    #[serde(default)]
    pub pause: Option<bool>,
    #[serde(default)]
    pub scale: Option<f32>,
}

/// Response for `Control Vod`.
/// `Control Vod` 的响应。
#[derive(Debug, Clone, Serialize)]
pub struct ControlVodResponse {
    pub code: u16,
    pub msg: String,
}

/// Request for `Stop Vod`.
/// `Stop Vod` 的请求。
#[derive(Debug, Clone, Deserialize)]
pub struct StopVodRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

/// Response for `Stop Vod`.
/// `Stop Vod` 的响应。
#[derive(Debug, Clone, Serialize)]
pub struct StopVodResponse {
    pub code: u16,
    pub msg: String,
}

/// Bundles a session registry + module config for HTTP handlers.
#[derive(Clone)]
pub struct VodApi {
    registry: Arc<VodSessionRegistry>,
    config: Arc<Mp4ModuleConfig>,
    /// Optional engine adapter for publishing VOD frames as a live engine stream.
    /// When `Some`, every started session's frames are bridged to RTSP/RTMP/etc
    /// subscribers via the engine stream key `file/<session_id>`.
    core_adapters: Option<Arc<dyn CoreAdaptersApi>>,
    /// Runtime handle used to spawn the event-bridge task. Required whenever
    /// `core_adapters` is set; the module obtains it from `EngineContext`.
    runtime_api: Option<Arc<dyn RuntimeApi>>,
}

impl VodApi {
    /// Creates a new `VodApi` instance.
    /// 创建新的 `VodApi` 实例。
    pub fn new(registry: Arc<VodSessionRegistry>, config: Arc<Mp4ModuleConfig>) -> Self {
        Self {
            registry,
            config,
            core_adapters: None,
            runtime_api: None,
        }
    }

    /// Returns a copy with `engine bridge` set.
    /// 返回将 `engine bridge` 设置后的副本。
    pub fn with_engine_bridge(
        registry: Arc<VodSessionRegistry>,
        config: Arc<Mp4ModuleConfig>,
        core_adapters: Arc<dyn CoreAdaptersApi>,
        runtime_api: Arc<dyn RuntimeApi>,
    ) -> Self {
        Self {
            registry,
            config,
            core_adapters: Some(core_adapters),
            runtime_api: Some(runtime_api),
        }
    }

    /// `registry` function of `VodApi`.
    /// `VodApi` 的 `registry` 函数。
    pub fn registry(&self) -> Arc<VodSessionRegistry> {
        self.registry.clone()
    }

    /// Starts the service or background task.
    /// 启动服务或后台任务。
    pub async fn start(&self, req: StartVodRequest) -> Result<StartVodResponse, VodApiError> {
        if req.uri.is_empty() {
            return Err(VodApiError::InvalidRequest("uri must not be empty".into()));
        }

        // Expand ZLM-style `;`-separated URI lists. Each segment must
        // resolve to an in-bounds path under the configured root; the
        // driver layer concatenates them into a single VOD timeline.
        let parts = crate::zlm_compat::expand_uri_list(&req.uri);
        let mut paths = Vec::with_capacity(parts.len());
        for part in &parts {
            paths.push(self.resolve_path(part)?);
        }

        let session_id = req
            .session_id
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("vod-{}", short_id(&req.uri)));
        let stream_stem = paths
            .first()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("anon")
            .to_string();
        let record = VodSessionRecord {
            session_id: session_id.clone(),
            source_uri: req.uri.clone(),
            // Stored as "file/<stem>" so the API surface mirrors ZLM's
            // virtual stream URL conventions; the engine stream key uses
            // namespace=`file`, path=`<stem>` (without the `file/` prefix
            // which is provided by the namespace).
            stream_key: format!("file/{stream_stem}"),
            paused: false,
            scale: 1.0,
            state: "starting".to_string(),
            reader_count: 0,
            remote_ip: None,
            remote_port: None,
            network_type: None,
            params: None,
        };
        let engine_path = stream_stem.clone();
        // ABL semantics: `loop_count` becomes `read_count`. `None` defaults
        // to one playback. `Some(0)` is rejected as "no playback".
        // `Some(u32::MAX)` is treated as the legacy "infinite" sentinel
        // (used by ZLM `file_repeat=true`); other values pass through.
        let read_count = match req.loop_count {
            None => 1,
            Some(0) => {
                return Err(VodApiError::InvalidRequest(
                    "loopCount=0 is not allowed".to_string(),
                ))
            }
            Some(u32::MAX) => -1,
            Some(n) if n > i32::MAX as u32 => -1,
            Some(n) => n as i32,
        };
        let driver_config = VodDriverConfig {
            read_chunk_bytes: self.config.read_chunk_bytes,
            idle_timeout_ms: self.config.idle_timeout_ms,
            reader_config: cheetah_codec::Mp4ReaderConfig {
                max_box_bytes: self.config.max_box_bytes,
                max_top_level_scan: 8 * 1024 * 1024,
            },
            read_count,
            ..Default::default()
        };
        let driver = if paths.len() == 1 {
            open_file(paths.into_iter().next().unwrap(), driver_config)
                .await
                .map_err(|e| VodApiError::Driver(e.to_string()))?
        } else {
            open_files(paths, driver_config)
                .await
                .map_err(|e| VodApiError::Driver(e.to_string()))?
        };
        let driver_arc = Arc::new(driver);

        // Insert into the registry *before* spawning the bridge task so
        // that a capacity / duplicate failure cleanly tears down the
        // driver instead of leaving an orphan task running.
        if let Err(e) = self.registry.insert(record, driver_arc.clone()) {
            let _ = driver_arc.send_control(VodControlCommand::Stop);
            return Err(e.into());
        }

        // If an engine bridge is configured, drain VOD events into the engine
        // stream so RTSP/RTMP/HTTP-FLV subscribers can play the file as if it
        // were a live source.
        if let (Some(core_adapters), Some(runtime_api)) =
            (self.core_adapters.clone(), self.runtime_api.clone())
        {
            if let Some(events) = driver_arc.take_events() {
                let stream_key = StreamKey::new("file", &engine_path);
                runtime_api.spawn(Box::pin(bridge_events(events, core_adapters, stream_key)));
            }
        }
        Ok(StartVodResponse {
            code: 200,
            msg: "success".to_string(),
            session_id,
        })
    }

    /// `control` function of `VodApi`.
    /// `VodApi` 的 `control` 函数。
    pub fn control(&self, req: ControlVodRequest) -> Result<ControlVodResponse, VodApiError> {
        let handle = self
            .registry
            .handle(&req.session_id)
            .ok_or_else(|| SessionError::NotFound(req.session_id.clone()))?;
        if let Some(seek) = req.seek {
            handle
                .send_control(VodControlCommand::Seek {
                    position_us: seek * 1000,
                })
                .map_err(|e| VodApiError::Driver(e.to_string()))?;
        }
        if let Some(p) = req.pause {
            handle
                .send_control(VodControlCommand::Pause(p))
                .map_err(|e| VodApiError::Driver(e.to_string()))?;
        }
        if let Some(s) = req.scale {
            handle
                .send_control(VodControlCommand::Scale(s))
                .map_err(|e| VodApiError::Driver(e.to_string()))?;
        }
        Ok(ControlVodResponse {
            code: 200,
            msg: "success".to_string(),
        })
    }

    /// Stops the service or background task.
    /// 停止服务或后台任务。
    pub fn stop(&self, req: StopVodRequest) -> Result<StopVodResponse, VodApiError> {
        // Send Stop *before* removing the registry record so the driver
        // task gracefully unwinds. Dropping the registry's Arc would also
        // close the channel, but emitting Stop first lets the driver flush
        // a final `Closed` event with a useful reason string.
        if let Some(handle) = self.registry.handle(&req.session_id) {
            let _ = handle.send_control(VodControlCommand::Stop);
        }
        self.registry.remove(&req.session_id)?;
        Ok(StopVodResponse {
            code: 200,
            msg: "success".to_string(),
        })
    }

    fn resolve_path(&self, uri: &str) -> Result<PathBuf, VodApiError> {
        // Accept ZLM-style `mp4:` / `flv:` prefixes, plus the cheetah
        // `file/` and `record/` namespace prefixes, before resolving against
        // the configured root.
        let normalized = crate::zlm_compat::normalize_rtmp_mp4_uri(uri);
        let trimmed = normalized
            .trim_start_matches("file/")
            .trim_start_matches("record/")
            .to_string();
        if trimmed.contains("..") || trimmed.starts_with('/') {
            return Err(VodApiError::InvalidRequest(
                "uri contains path traversal or absolute path".to_string(),
            ));
        }
        let mut buf = PathBuf::from(&self.config.root_path);
        buf.push(&trimmed);
        Ok(buf)
    }
}

fn short_id(input: &str) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    input.hash(&mut h);
    format!("{:x}", h.finish() & 0xFFFF_FFFF)
}

/// Drains driver events into engine streams so RTSP/RTMP/HTTP-FLV/WS-FLV
/// subscribers can play the VOD source through their existing live-stream code
/// paths. This is the cross-protocol bridge required by Phase 04.
async fn bridge_events(
    mut events: VodEventStream,
    core_adapters: Arc<dyn CoreAdaptersApi>,
    stream_key: StreamKey,
) {
    while let Some(event) = events.next().await {
        match event {
            VodDriverEvent::Tracks(tracks) => {
                if let Err(e) = core_adapters
                    .update_tracks(stream_key.clone(), tracks)
                    .await
                {
                    warn!("vod bridge update_tracks failed: {e}");
                }
            }
            VodDriverEvent::Frame(frame) => {
                if let Err(e) = core_adapters
                    .publish_frame(stream_key.clone(), Arc::new(frame))
                    .await
                {
                    warn!("vod bridge publish_frame failed: {e}");
                    break;
                }
            }
            VodDriverEvent::Diagnostic(diag) => {
                // Diagnostics are best-effort audit events; record them in
                // the trace log so operators can correlate seek failures
                // and similar control-plane errors.
                tracing::debug!(?diag, "vod bridge received diagnostic");
            }
            VodDriverEvent::Closed { .. } => {
                let _ = core_adapters.close_stream(&stream_key).await;
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_path_rejects_traversal() {
        let api = VodApi::new(
            Arc::new(VodSessionRegistry::new(8)),
            Arc::new(Mp4ModuleConfig::default()),
        );
        let err = api.resolve_path("../../../etc/passwd").unwrap_err();
        assert!(matches!(err, VodApiError::InvalidRequest(_)));
    }

    #[test]
    fn resolve_path_strips_namespace_prefixes() {
        let api = VodApi::new(
            Arc::new(VodSessionRegistry::new(8)),
            Arc::new(Mp4ModuleConfig::default()),
        );
        let p = api.resolve_path("file/test.mp4").unwrap();
        assert!(p.to_str().unwrap().ends_with("test.mp4"));
    }

    #[test]
    fn resolve_path_normalizes_rtmp_mp4_prefix() {
        let api = VodApi::new(
            Arc::new(VodSessionRegistry::new(8)),
            Arc::new(Mp4ModuleConfig::default()),
        );
        let p = api.resolve_path("mp4:0").unwrap();
        assert!(p.to_str().unwrap().ends_with("0.mp4"));
    }

    #[test]
    fn resolve_path_rejects_absolute_paths() {
        let api = VodApi::new(
            Arc::new(VodSessionRegistry::new(8)),
            Arc::new(Mp4ModuleConfig::default()),
        );
        let err = api.resolve_path("/etc/passwd").unwrap_err();
        assert!(matches!(err, VodApiError::InvalidRequest(_)));
    }
}
