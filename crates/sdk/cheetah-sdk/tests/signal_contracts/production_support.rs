//! Production test support for external signal integration contracts.
//!
//! Provides a real `Engine` builder with `RtpModule`, `RecordModule`, `ProxyModule`,
//! and a fixture module that publishes a VP8 stream and registers a snapshot file.
//!
//! 外部信令集成生产测试支持。
//!
//! 提供真实的 Engine 构建器，包含 RtpModule、RecordModule、ProxyModule
//! 以及一个发布 VP8 视频流并注册快照文件的 fixture module。

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, MonoTime, Timebase, TrackId, TrackInfo,
    TrackReadiness,
};
use cheetah_config::ConfigStore;
use cheetah_engine::{Engine, EngineBuilder, EngineMediaFacade};
use cheetah_media_api::command::{
    DeleteSnapshotRequest, PublishRequest, SnapshotQuery, SnapshotRequest,
};
use cheetah_media_api::error::Result as MediaResult;
use cheetah_media_api::event::{MediaEvent, MediaEventSender};
use cheetah_media_api::ids::StreamKeyBridge;
use cheetah_media_api::ids::{FileHandle, MediaKey, SnapshotId};
use cheetah_media_api::media_file_store::FileStoreEntry;
use cheetah_media_api::model::{Page, SnapshotHandle, SnapshotInfo, SnapshotState};
use cheetah_media_api::port::{MediaRequestContext, SnapshotApi};
use cheetah_proxy_module::ProxyModuleFactory;
use cheetah_record_module::RecordModuleFactory;
use cheetah_rtp_module::RtpModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{
    CancellationToken, ConfigApplyApi, ConfigEffect, EngineContext, MediaFramePublisher, Module,
    ModuleCapability, ModuleConfigChange, ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext,
    ModuleManifest, ModuleState, ProviderRegistration, SdkError, StreamKey,
};
use serde_json::json;
use tokio::time::sleep;

static SNAPSHOT_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Default golden stream key used by production contract tests.
///
/// 生产契约测试使用的默认 golden 流键。
pub fn golden_key() -> MediaKey {
    MediaKey::with_default_vhost("live", "golden", None).expect("valid golden key")
}

/// Default golden stream key used by low-level subscriber tests.
pub fn golden_stream_key() -> StreamKey {
    let (namespace, path) = StreamKeyBridge::to_namespace_path(&golden_key());
    StreamKey::new(namespace, path)
}

/// Default empty request context.
pub fn ctx() -> MediaRequestContext {
    MediaRequestContext::default()
}

/// Return the engine media facade.
pub fn media_facade(engine: &Engine) -> Arc<EngineMediaFacade> {
    engine.media_facade()
}

/// Shared production engine builder with fixture, record, proxy and RTP modules.
///
/// 启动带 fixture、录制、代理和 RTP module 的真实 Engine。
pub async fn production_engine() -> Arc<Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({}));

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let record_dir = std::env::temp_dir().join(format!("cheetah_production_record_{ts}"));
    let _ = fs::create_dir_all(&record_dir);

    config
        .apply_module_patch(
            &ModuleId::new("record"),
            json!({ "root_path": record_dir.to_string_lossy().to_string() }),
            ConfigEffect::Immediate,
        )
        .expect("apply record root path");

    config
        .apply_module_patch(
            &ModuleId::new("rtp"),
            json!({
                "listen_udp": "127.0.0.1:0",
                "listen_tcp": "127.0.0.1:0",
                "rtcp_listen_udp": "127.0.0.1:0"
            }),
            ConfigEffect::Immediate,
        )
        .expect("apply rtp config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .with_config_schema_registry(config)
        .register_module_factory(Arc::new(ProductionFixtureModuleFactory))
        .register_module_factory(Arc::new(RecordModuleFactory))
        .register_module_factory(Arc::new(ProxyModuleFactory))
        .register_module_factory(Arc::new(RtpModuleFactory))
        .build()
        .expect("engine build");
    let engine = Arc::new(engine);
    engine.start().await.expect("engine start");
    engine
}

/// Wait a short time for background tasks to make progress.
pub async fn wait_ms(ms: u64) {
    sleep(Duration::from_millis(ms)).await;
}

/// Event sender that records all received media events.
#[derive(Debug, Default, Clone)]
pub struct RecordingEventSender {
    events: Arc<Mutex<Vec<MediaEvent>>>,
}

impl RecordingEventSender {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<MediaEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl MediaEventSender for RecordingEventSender {
    fn send(&self, event: MediaEvent) -> MediaResult<()> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event);
        Ok(())
    }

    fn lagged(&self, _dropped: u64) -> MediaResult<()> {
        Ok(())
    }
}

pub struct FakeSnapshotApi {
    file_handle: FileHandle,
    media_key: MediaKey,
    snapshot_info: SnapshotInfo,
}

impl FakeSnapshotApi {
    pub fn new(file_handle: FileHandle, media_key: MediaKey, size_bytes: u64) -> Self {
        let snapshot_id = SnapshotId(format!(
            "snap-{}",
            SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let snapshot_info = SnapshotInfo {
            snapshot_id: snapshot_id.clone(),
            media_key: media_key.clone(),
            state: SnapshotState::Completed,
            path_handle: file_handle.clone(),
            created_at: 0,
            size_bytes: Some(size_bytes),
            format: "jpg".to_string(),
        };
        Self {
            file_handle,
            media_key,
            snapshot_info,
        }
    }
}

#[async_trait]
impl SnapshotApi for FakeSnapshotApi {
    async fn take_snapshot(
        &self,
        _ctx: &MediaRequestContext,
        request: SnapshotRequest,
    ) -> MediaResult<SnapshotHandle> {
        Ok(SnapshotHandle {
            snapshot_id: self.snapshot_info.snapshot_id.clone(),
            media_key: request.media_key,
            state: SnapshotState::Completed,
            path_handle: self.file_handle.clone(),
            download_url: None,
            created_at: 0,
        })
    }

    async fn query_snapshots(
        &self,
        _ctx: &MediaRequestContext,
        query: SnapshotQuery,
    ) -> MediaResult<Page<SnapshotInfo>> {
        let mut items = Vec::new();
        let matches = query
            .vhost
            .as_ref()
            .is_none_or(|v| v == &self.media_key.vhost.0)
            && query
                .app
                .as_ref()
                .is_none_or(|a| a == &self.media_key.app.0)
            && query
                .stream
                .as_ref()
                .is_none_or(|s| s == &self.media_key.stream.0);
        if matches {
            items.push(self.snapshot_info.clone());
        }
        let total = items.len() as u64;
        Ok(Page {
            items,
            page: 1,
            page_size: 100,
            total,
            next_cursor: None,
        })
    }

    async fn delete_snapshot_directory(
        &self,
        _ctx: &MediaRequestContext,
        _request: DeleteSnapshotRequest,
    ) -> MediaResult<()> {
        Ok(())
    }
}

pub struct ProductionFixtureModule {
    state: ModuleState,
    ctx: Option<EngineContext>,
    publisher: Option<Arc<Box<dyn MediaFramePublisher>>>,
    snapshot_registration: Option<ProviderRegistration>,
}

impl ProductionFixtureModule {
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            ctx: None,
            publisher: None,
            snapshot_registration: None,
        }
    }
}

#[async_trait]
impl Module for ProductionFixtureModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new("production-fixture"),
            display_name: "Production Fixture".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, init_ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.state = ModuleState::Initialized;
        let engine_ctx = init_ctx.engine.clone();
        self.ctx = Some(engine_ctx.clone());

        // Register a public snapshot file.
        let file_path = std::env::temp_dir().join(format!(
            "cheetah_production_snapshot_{}",
            SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::write(&file_path, b"fake snapshot");
        let entry = FileStoreEntry {
            media_key: golden_key(),
            file_type: "snapshot".to_string(),
            content_type: "image/jpeg".to_string(),
            size_bytes: 13,
            created_at_ms: 0,
            expires_at_ms: None,
            absolute_path: file_path.to_string_lossy().to_string(),
            owner_principal: None,
            allowed_principals: Vec::new(),
        };
        let file_handle = engine_ctx
            .media_file_store
            .register_file(&crate::production_support::ctx(), entry)
            .map_err(|e| SdkError::Internal(e.to_string()))?;
        let snapshot_api = Arc::new(FakeSnapshotApi::new(file_handle, golden_key(), 13));
        self.snapshot_registration =
            Some(engine_ctx.media_services.register_snapshot(snapshot_api));

        // Open a VP8 publisher on the golden stream.
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::VP8, 90_000);
        track.width = Some(640);
        track.height = Some(480);
        track.readiness = TrackReadiness::Ready;
        let publisher = engine_ctx
            .media_data_plane
            .open_frame_publisher(
                &crate::production_support::ctx(),
                PublishRequest {
                    media_key: golden_key(),
                    protocol: "test".to_string(),
                    origin: None,
                    remote_endpoint: None,
                    lease_token: None,
                    auth_context: Default::default(),
                    metadata: Default::default(),
                },
            )
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;
        publisher
            .update_tracks(vec![track])
            .map_err(|e| SdkError::Internal(e.to_string()))?;
        self.publisher = Some(Arc::new(publisher));
        Ok(())
    }

    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        self.state = ModuleState::Running;
        let Some(publisher) = self.publisher.take() else {
            return Ok(());
        };
        let Some(ctx) = self.ctx.clone() else {
            return Ok(());
        };
        let runtime_api = ctx.runtime_api.clone();
        let runtime_api_for_task = runtime_api.clone();
        let cancel = cancel.child_token();
        let _ = runtime_api.spawn(Box::pin(async move {
            let mut pts = 0i64;
            let payload = Bytes::from_static(b"vp8-key");
            let timebase = Timebase::new(1, 30);
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                let mut timer = runtime_api_for_task.sleep_until(MonoTime::from_micros(
                    runtime_api_for_task.now().as_micros() + 100_000,
                ));
                timer.wait().await;
                if cancel.is_cancelled() {
                    break;
                }
                let mut frame = AVFrame::new(
                    TrackId(1),
                    MediaKind::Video,
                    CodecId::VP8,
                    FrameFormat::CanonicalVp8Frame,
                    pts,
                    pts,
                    timebase,
                    payload.clone(),
                );
                frame.flags = FrameFlags::KEY;
                if publisher.push_frame(Arc::new(frame)).is_err() {
                    break;
                }
                pts += 1;
            }
            let _ = publisher.close().await;
        }));
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        self.state = ModuleState::Stopped;
        if let Some(ctx) = self.ctx.take() {
            if let Some(reg) = self.snapshot_registration.take() {
                ctx.media_services.unregister(&reg);
            }
        }
        Ok(())
    }

    async fn apply_config(
        &mut self,
        _change: ModuleConfigChange,
    ) -> Result<ConfigEffect, SdkError> {
        Ok(ConfigEffect::Immediate)
    }
}

pub struct ProductionFixtureModuleFactory;

impl ModuleFactory for ProductionFixtureModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new("production-fixture"),
            display_name: "Production Fixture".to_string(),
            dependencies: Vec::new(),
            config_namespace: "production_fixture".to_string(),
            routes_prefix: "/".to_string(),
            capabilities: vec![ModuleCapability::Publish, ModuleCapability::Subscribe],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(ProductionFixtureModule::new())
    }
}
