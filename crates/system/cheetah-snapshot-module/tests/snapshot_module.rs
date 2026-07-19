use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, MonoTime, Timebase, TrackId, TrackInfo,
    TrackReadiness,
};
use cheetah_config::ConfigStore;
use cheetah_engine::{Engine, EngineBuilder};
use cheetah_media_api::auth::{MediaResourceGrant, MediaResourceSelector, Pattern};
use cheetah_media_api::command::{
    DeleteSnapshotRequest, PublishRequest, SnapshotQuery, SnapshotRequest,
};
use cheetah_media_api::ids::MediaKey;
use cheetah_media_api::model::SnapshotState;
use cheetah_media_api::port::{MediaFacade, MediaRequestContext, SnapshotApi};
use cheetah_media_api::{MediaScope, Principal};
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

fn make_jpeg_payload(_width: u32, _height: u32) -> Bytes {
    // Fixed 8x6 JPEG fixture generated with PIL; sha256 = 9208189deaa2dd9c36f36506932f3512bd1c1d30df2feb0a76c574c2ed1d8614.
    Bytes::from_static(include_bytes!("testdata/golden_8x6.jpg"))
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

async fn build_engine_with_root() -> (Arc<Engine>, PathBuf) {
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
    (engine, root)
}

async fn build_engine() -> Arc<Engine> {
    build_engine_with_root().await.0
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

fn golden_query() -> SnapshotQuery {
    SnapshotQuery {
        vhost: Some("__defaultVhost__".to_string()),
        app: Some("live".to_string()),
        stream: Some("snap-test".to_string()),
        snapshot_id: None,
        start_time_ms: None,
        end_time_ms: None,
        page: 1,
        page_size: 10,
    }
}

fn make_snapshot_request() -> SnapshotRequest {
    SnapshotRequest {
        media_key: golden_key(),
        timeout_ms: 2000,
        format: "jpg".to_string(),
        quality: None,
        max_width: None,
        max_height: None,
        storage_policy: Default::default(),
        capture_policy: Default::default(),
    }
}

fn grant_for(scope: MediaScope, key: &MediaKey) -> MediaResourceGrant {
    MediaResourceGrant {
        selector: MediaResourceSelector {
            vhost: Pattern::Exact(key.vhost.0.clone()),
            app: Pattern::Exact(key.app.0.clone()),
            stream: Pattern::Exact(key.stream.0.clone()),
        },
        scopes: vec![scope],
    }
}

fn snapshot_file_path(root: &Path, snapshot_id: &str, format: &str) -> PathBuf {
    root.join("live")
        .join("snap-test")
        .join(format!("{snapshot_id}.{format}"))
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_module_deletes_snapshots_and_removes_files() {
    let (engine, root) = build_engine_with_root().await;
    let facade = engine.media_facade();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let snap1 = facade
        .take_snapshot(&MediaRequestContext::default(), make_snapshot_request())
        .await
        .expect("take_snapshot 1");
    tokio::time::sleep(Duration::from_millis(2)).await;
    let snap2 = facade
        .take_snapshot(&MediaRequestContext::default(), make_snapshot_request())
        .await
        .expect("take_snapshot 2");

    let path1 = snapshot_file_path(&root, &snap1.snapshot_id.0, "jpg");
    let path2 = snapshot_file_path(&root, &snap2.snapshot_id.0, "jpg");
    assert!(path1.exists());
    assert!(path2.exists());

    let result = facade
        .delete_snapshots(
            &MediaRequestContext::default(),
            DeleteSnapshotRequest {
                media_key: golden_key(),
                directory: None,
                retain_count: None,
            },
        )
        .await
        .expect("delete_snapshots");

    assert_eq!(result.matched, 2);
    assert_eq!(result.deleted, 2);
    assert_eq!(result.failed, 0);
    assert!(result.failures.is_empty());

    assert!(!path1.exists());
    assert!(!path2.exists());

    let page = facade
        .query_snapshots(&MediaRequestContext::default(), golden_query())
        .await
        .unwrap();
    assert!(page.items.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_module_delete_respects_retain_count() {
    let (engine, root) = build_engine_with_root().await;
    let facade = engine.media_facade();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let _ = facade
        .take_snapshot(&MediaRequestContext::default(), make_snapshot_request())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let snap2 = facade
        .take_snapshot(&MediaRequestContext::default(), make_snapshot_request())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let snap3 = facade
        .take_snapshot(&MediaRequestContext::default(), make_snapshot_request())
        .await
        .unwrap();

    let result = facade
        .delete_snapshots(
            &MediaRequestContext::default(),
            DeleteSnapshotRequest {
                media_key: golden_key(),
                directory: None,
                retain_count: Some(1),
            },
        )
        .await
        .unwrap();

    assert_eq!(result.matched, 3);
    assert_eq!(result.deleted, 2);
    assert_eq!(result.failed, 0);

    let path3 = snapshot_file_path(&root, &snap3.snapshot_id.0, "jpg");
    let path2 = snapshot_file_path(&root, &snap2.snapshot_id.0, "jpg");
    assert!(path3.exists());
    assert!(!path2.exists());

    let page = facade
        .query_snapshots(&MediaRequestContext::default(), golden_query())
        .await
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].snapshot_id.0, snap3.snapshot_id.0);
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_module_delete_respects_ownership() {
    let (engine, _root) = build_engine_with_root().await;
    let facade = engine.media_facade();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let owner_ctx = MediaRequestContext {
        principal: Some(Principal {
            identity: "alice".to_string(),
            scopes: Vec::new(),
            resource_grants: vec![grant_for(MediaScope::MediaRead, &golden_key())],
        }),
        ..Default::default()
    };

    let _ = facade
        .take_snapshot(&owner_ctx, make_snapshot_request())
        .await
        .unwrap();

    let other_ctx = MediaRequestContext {
        principal: Some(Principal {
            identity: "bob".to_string(),
            scopes: Vec::new(),
            resource_grants: vec![grant_for(MediaScope::MediaRead, &golden_key())],
        }),
        ..Default::default()
    };

    let result = facade
        .delete_snapshots(
            &other_ctx,
            DeleteSnapshotRequest {
                media_key: golden_key(),
                directory: None,
                retain_count: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(result.matched, 1);
    assert_eq!(result.deleted, 0);
    assert_eq!(result.failed, 1);

    let page = facade
        .query_snapshots(&MediaRequestContext::default(), golden_query())
        .await
        .unwrap();
    assert_eq!(page.total, 1);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn snapshot_module_rejects_symlink_escape_during_delete() {
    use std::os::unix::fs::symlink;

    let (engine, root) = build_engine_with_root().await;
    let facade = engine.media_facade();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let snap = facade
        .take_snapshot(&MediaRequestContext::default(), make_snapshot_request())
        .await
        .unwrap();

    let safe_path = snapshot_file_path(&root, &snap.snapshot_id.0, "jpg");
    let outside = std::env::temp_dir().join(format!("cheetah_outside_{}", snap.snapshot_id.0));
    std::fs::write(&outside, b"outside").unwrap();

    std::fs::remove_file(&safe_path).unwrap();
    symlink(&outside, &safe_path).unwrap();

    let result = facade
        .delete_snapshots(
            &MediaRequestContext::default(),
            DeleteSnapshotRequest {
                media_key: golden_key(),
                directory: None,
                retain_count: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(result.matched, 1);
    assert_eq!(result.deleted, 0);
    assert_eq!(result.failed, 1);
    assert!(result.failures.iter().any(|f| f.reason.contains("symlink")));

    // The outside file must not have been deleted and the registry entry must remain.
    assert!(outside.exists());
    let page = facade
        .query_snapshots(&MediaRequestContext::default(), golden_query())
        .await
        .unwrap();
    assert_eq!(page.total, 1);

    // Cleanup.
    let _ = std::fs::remove_file(&outside);
    let _ = std::fs::remove_file(&safe_path);
}
