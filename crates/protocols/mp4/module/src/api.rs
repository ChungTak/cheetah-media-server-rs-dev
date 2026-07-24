//! SMS-style VOD HTTP API request/response shapes.
//!
//! SMS 风格 VOD HTTP API 请求/响应结构。

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

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
/// Errors returned by the VOD API layer.
///
/// VOD API 层返回的错误。
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

#[derive(Debug, Clone, Deserialize)]
/// Request body to start a VOD session.
///
/// 启动 VOD 会话的请求体。
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

#[derive(Debug, Clone, Serialize)]
/// Response returned by `start`.
///
/// `start` 返回的响应。
pub struct StartVodResponse {
    pub code: u16,
    pub msg: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
/// Request body to control a running VOD session.
///
/// 控制运行中 VOD 会话的请求体。
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

#[derive(Debug, Clone, Serialize)]
/// Response returned by `control`.
///
/// `control` 返回的响应。
pub struct ControlVodResponse {
    pub code: u16,
    pub msg: String,
}

#[derive(Debug, Clone, Deserialize)]
/// Request body to stop a VOD session.
///
/// 停止 VOD 会话的请求体。
pub struct StopVodRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize)]
/// Response returned by `stop`.
///
/// `stop` 返回的响应。
pub struct StopVodResponse {
    pub code: u16,
    pub msg: String,
}

/// Bundles a session registry + module config for HTTP handlers.
///
/// 为 HTTP 处理器封装会话注册表与模块配置。
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

/// `VodApi` constructors and request handlers.
///
/// `VodApi` 构造与请求处理器。
impl VodApi {
    pub fn new(registry: Arc<VodSessionRegistry>, config: Arc<Mp4ModuleConfig>) -> Self {
        Self {
            registry,
            config,
            core_adapters: None,
            runtime_api: None,
        }
    }

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

    /// Return a clone of the session registry.
    ///
    /// 返回会话注册表的克隆。
    pub fn registry(&self) -> Arc<VodSessionRegistry> {
        self.registry.clone()
    }

    /// Start a VOD session, optionally bridging events into the engine.
    ///
    /// 启动 VOD 会话，可选择将事件桥接到引擎。
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
            let path = paths
                .into_iter()
                .next()
                .ok_or_else(|| VodApiError::InvalidRequest("missing vod path".to_string()))?;
            open_file(path, driver_config)
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
                runtime_api.spawn(Box::pin(bridge_events(
                    events,
                    core_adapters,
                    stream_key,
                    session_id.clone(),
                    None,
                )));
            }
        }
        Ok(StartVodResponse {
            code: 200,
            msg: "success".to_string(),
            session_id,
        })
    }

    /// Send seek/pause/scale commands to a running session.
    ///
    /// 向运行中的会话发送 seek/pause/scale 命令。
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

    /// Start playback from an absolute, already-authorized path.
    ///
    /// Used by `PlaybackApi`. Path traversal checks are the caller's
    /// responsibility (file-store resolve). Frames are published to `stream_key`.
    /// When `playback_sessions` is set, driver Ready/Frame/Closed events update
    /// the live `PlaybackSession` row (duration, position, EOF state).
    ///
    /// 从已授权的绝对路径启动回放（供 `PlaybackApi` 使用）。
    #[allow(clippy::too_many_arguments)]
    pub async fn start_absolute(
        &self,
        session_id: String,
        absolute_path: PathBuf,
        source_uri: String,
        stream_key: StreamKey,
        start_position_ms: i64,
        scale: f32,
        playback_sessions: Option<
            Arc<
                parking_lot::RwLock<
                    std::collections::HashMap<String, cheetah_media_api::model::PlaybackSession>,
                >,
            >,
        >,
    ) -> Result<StartVodResponse, VodApiError> {
        if session_id.is_empty() {
            return Err(VodApiError::InvalidRequest(
                "session_id must not be empty".into(),
            ));
        }
        if !absolute_path.is_absolute() {
            return Err(VodApiError::InvalidRequest(
                "absolute_path must be absolute".into(),
            ));
        }
        let stream_key_display = format!("{}/{}", stream_key.namespace, stream_key.path);
        let record = VodSessionRecord {
            session_id: session_id.clone(),
            source_uri,
            stream_key: stream_key_display,
            paused: false,
            scale,
            state: "starting".to_string(),
            reader_count: 0,
            remote_ip: None,
            remote_port: None,
            network_type: None,
            params: None,
        };
        let driver_config = VodDriverConfig {
            read_chunk_bytes: self.config.read_chunk_bytes,
            idle_timeout_ms: self.config.idle_timeout_ms,
            reader_config: cheetah_codec::Mp4ReaderConfig {
                max_box_bytes: self.config.max_box_bytes,
                max_top_level_scan: 8 * 1024 * 1024,
            },
            read_count: 1,
            ..Default::default()
        };
        let driver = open_file(absolute_path, driver_config)
            .await
            .map_err(|e| VodApiError::Driver(e.to_string()))?;
        let driver_arc = Arc::new(driver);
        if let Err(e) = self.registry.insert(record, driver_arc.clone()) {
            let _ = driver_arc.send_control(VodControlCommand::Stop);
            return Err(e.into());
        }
        if let (Some(core_adapters), Some(runtime_api)) =
            (self.core_adapters.clone(), self.runtime_api.clone())
        {
            if let Some(events) = driver_arc.take_events() {
                runtime_api.spawn(Box::pin(bridge_events(
                    events,
                    core_adapters,
                    stream_key,
                    session_id.clone(),
                    playback_sessions,
                )));
            }
        }
        if start_position_ms > 0 {
            let _ = driver_arc.send_control(VodControlCommand::Seek {
                position_us: start_position_ms.saturating_mul(1_000),
            });
        }
        if (scale - 1.0).abs() > f32::EPSILON {
            let _ = driver_arc.send_control(VodControlCommand::Scale(scale));
        }
        Ok(StartVodResponse {
            code: 200,
            msg: "success".to_string(),
            session_id,
        })
    }

    /// Stop a VOD session and remove it from the registry.
    ///
    /// 停止 VOD 会话并从注册表移除。
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

    /// Resolve a URI against the configured root, rejecting path traversal.
    ///
    /// 将 URI 解析到配置根目录，拒绝路径遍历。
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

/// Generate a short hash ID for an input string.
///
/// 为输入字符串生成短哈希 ID。
fn short_id(input: &str) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    input.hash(&mut h);
    format!("{:x}", h.finish() & 0xFFFF_FFFF)
}

/// Drains driver events into engine streams so RTSP/RTMP/HTTP-FLV/WS-FLV
/// subscribers can play the VOD source through their existing live-stream code
/// paths. This is the cross-protocol bridge required by Phase 04.
///
/// 将驱动事件消耗到引擎流，使 RTSP/RTMP/HTTP-FLV/WS-FLV 订阅者可以通过现有直播流代码路径播放 VOD 源。
/// 这是 Phase 04 要求的跨协议桥接。
async fn bridge_events(
    mut events: VodEventStream,
    core_adapters: Arc<dyn CoreAdaptersApi>,
    stream_key: StreamKey,
    session_id: String,
    playback_sessions: Option<
        Arc<
            parking_lot::RwLock<
                std::collections::HashMap<String, cheetah_media_api::model::PlaybackSession>,
            >,
        >,
    >,
) {
    use cheetah_media_api::model::PlaybackSessionState;
    while let Some(event) = events.next().await {
        match event {
            VodDriverEvent::Ready { duration_us } => {
                if let Some(sessions) = playback_sessions.as_ref() {
                    if let Some(s) = sessions.write().get_mut(&session_id) {
                        s.duration_ms = (duration_us.max(0) / 1000) as u64;
                        s.updated_at = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(s.updated_at);
                    }
                }
            }
            VodDriverEvent::Tracks(tracks) => {
                if let Err(e) = core_adapters
                    .update_tracks(stream_key.clone(), tracks)
                    .await
                {
                    warn!("vod bridge update_tracks failed: {e}");
                }
            }
            VodDriverEvent::Frame(frame) => {
                if let Some(sessions) = playback_sessions.as_ref() {
                    if let Some(s) = sessions.write().get_mut(&session_id) {
                        if frame.pts_us >= 0 {
                            s.position_ms = frame.pts_us / 1000;
                            s.updated_at = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as i64)
                                .unwrap_or(s.updated_at);
                        }
                    }
                }
                if let Err(e) = core_adapters
                    .publish_frame(stream_key.clone(), Arc::new(frame))
                    .await
                {
                    warn!("vod bridge publish_frame failed: {e}");
                    if let Some(sessions) = playback_sessions.as_ref() {
                        if let Some(s) = sessions.write().get_mut(&session_id) {
                            s.state = PlaybackSessionState::Failed;
                            s.last_error = Some(e.to_string());
                        }
                    }
                    break;
                }
            }
            VodDriverEvent::Diagnostic(diag) => {
                tracing::debug!(?diag, "vod bridge received diagnostic");
            }
            VodDriverEvent::Closed { reason } => {
                let _ = core_adapters.close_stream(&stream_key).await;
                if let Some(sessions) = playback_sessions.as_ref() {
                    if let Some(s) = sessions.write().get_mut(&session_id) {
                        let eof = reason.is_empty()
                            || reason.contains("eof")
                            || reason.contains("EOF")
                            || reason.contains("end")
                            || reason.contains("CloseSession")
                            || reason.contains("completed");
                        if eof {
                            s.state = PlaybackSessionState::Completed;
                            s.last_error = None;
                        } else {
                            s.state = PlaybackSessionState::Failed;
                            s.last_error = Some(reason);
                        }
                        s.updated_at = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(s.updated_at);
                    }
                }
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
