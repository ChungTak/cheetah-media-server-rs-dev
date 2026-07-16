use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, MonoTime, Timebase, TrackId, TrackInfo,
    TrackReadiness,
};
use cheetah_config::ConfigStore;
use cheetah_engine::{Engine, EngineBuilder};
use cheetah_media_api::command::{PublishRequest, SnapshotRequest};
use cheetah_media_api::ids::MediaKey;
use cheetah_media_api::model::SnapshotState;
use cheetah_media_api::port::{MediaFacade, MediaRequestContext, SnapshotApi};
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{
    CancellationToken, ConfigApplyApi, ConfigEffect, EngineContext, MediaFramePublisher, Module,
    ModuleCapability, ModuleConfigChange, ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext,
    ModuleManifest, ModuleState, SdkError,
};
use cheetah_snapshot_module::SnapshotModuleFactory;
use serde_json::json;
use tokio::time::timeout;

fn golden_key() -> MediaKey {
    MediaKey::with_default_vhost("live", "snap-test", None).expect("valid key")
}

fn make_jpeg_payload(width: u32, height: u32) -> Bytes {
    let img = image::RgbaImage::new(width, height);
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .expect("encode jpeg");
    Bytes::from(buf.into_inner())
}

struct FixtureModule {
    state: ModuleState,
    ctx: Option<EngineContext>,
    publisher: Option<Arc<Box<dyn MediaFramePublisher>>>,
}

impl FixtureModule {
    fn new() -> Self {
        Self {
            state: ModuleState::Created,
            ctx: None,
            publisher: None,
        }
    }
}

#[async_trait::async_trait]
impl Module for FixtureModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new("snap-fixture"),
            display_name: "Snapshot Fixture".to_string(),
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
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::MJPEG, 90_000);
        track.width = Some(320);
        track.height = Some(240);
        track.readiness = TrackReadiness::Ready;
        let publisher = engine_ctx
            .media_data_plane
            .open_frame_publisher(
                &MediaRequestContext::default(),
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
        let runtime = ctx.runtime_api.clone();
        let runtime2 = runtime.clone();
        let cancel = cancel.child_token();
        let _ = runtime.spawn(Box::pin(async move {
            let mut pts = 0i64;
            let payload = make_jpeg_payload(8, 6);
            let timebase = Timebase::new(1, 30);
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                let mut timer = runtime2
                    .sleep_until(MonoTime::from_micros(runtime2.now().as_micros() + 50_000));
                timer.wait().await;
                let mut frame = AVFrame::new(
                    TrackId(1),
                    MediaKind::Video,
                    CodecId::MJPEG,
                    FrameFormat::MjpegFrame,
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
        self.ctx = None;
        Ok(())
    }

    async fn apply_config(
        &mut self,
        _change: ModuleConfigChange,
    ) -> Result<ConfigEffect, SdkError> {
        Ok(ConfigEffect::Immediate)
    }
}

struct FixtureFactory;
impl ModuleFactory for FixtureFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new("snap-fixture"),
            display_name: "Snapshot Fixture".to_string(),
            dependencies: Vec::new(),
            config_namespace: "snap-fixture".to_string(),
            routes_prefix: String::new(),
            capabilities: vec![ModuleCapability::Publish],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(FixtureModule::new())
    }
}

async fn build_engine() -> Arc<Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({}));
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("cheetah_snap_mod_{ts}"));
    let _ = std::fs::create_dir_all(&root);
    config
        .apply_module_patch(
            &ModuleId::new("snapshot"),
            json!({ "root_path": root.to_string_lossy().to_string() }),
            ConfigEffect::Immediate,
        )
        .expect("patch snapshot config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .with_config_schema_registry(config)
        .register_module_factory(Arc::new(FixtureFactory))
        .register_module_factory(Arc::new(SnapshotModuleFactory))
        .build()
        .expect("build");
    let engine = Arc::new(engine);
    engine.start().await.expect("start");
    engine
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_module_registers_capability_and_captures_keyframe() {
    let engine = build_engine().await;
    let facade = engine.media_facade();
    assert!(
        facade
            .capabilities()
            .has(cheetah_media_api::MediaCapability::Snapshot),
        "snapshot capability must be advertised"
    );

    // Wait for fixture frames.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let snap = timeout(
        Duration::from_secs(3),
        facade.take_snapshot(
            &MediaRequestContext::default(),
            SnapshotRequest {
                media_key: golden_key(),
                timeout_ms: 2000,
                format: "jpg".to_string(),
                quality: None,
                max_width: None,
                max_height: None,
                storage_policy: Default::default(),
                capture_policy: Default::default(),
            },
        ),
    )
    .await
    .expect("timeout")
    .expect("take_snapshot");

    assert_eq!(snap.state, SnapshotState::Completed);
    assert!(!snap.path_handle.0.is_empty());
    assert_eq!(snap.media_key, golden_key());
}
