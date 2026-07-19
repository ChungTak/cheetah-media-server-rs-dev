//! Native snapshot HTTP download and delete error mapping tests.
//!
//! 这些集成测试验证 native HTTP 端点能够：
//! - 通过快照 ID 下载已注册的文件（带正确 MIME、Content-Length、安全文件名）。
//! - 对未知快照返回 404。
//! - 对非法删除请求返回 400。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_api::command::{DeleteSnapshotRequest, SnapshotQuery, SnapshotRequest};
use cheetah_media_api::ids::{FileHandle, MediaKey, SnapshotId};
use cheetah_media_api::media_file_store::{DeleteBatchResult, FileStoreEntry};
use cheetah_media_api::model::{Page, SnapshotHandle, SnapshotInfo, SnapshotState};
use cheetah_media_api::port::{MediaRequestContext, SnapshotApi};
use cheetah_media_module::NativeMediaModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{
    ConfigEffect, HttpMethod, HttpRequest, HttpResponse, HttpRouteDescriptor, Module,
    ModuleCapability, ModuleConfigChange, ModuleFactory, ModuleHttpService, ModuleId, ModuleInfo,
    ModuleInitContext, ModuleManifest, ModuleState, SdkError,
};
use serde_json::json;

static FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

fn make_jpeg_payload(_width: u32, _height: u32) -> Bytes {
    // Fixed 8x6 JPEG fixture generated with PIL; sha256 = 9208189deaa2dd9c36f36506932f3512bd1c1d30df2feb0a76c574c2ed1d8614.
    Bytes::from_static(include_bytes!("testdata/golden_8x6.jpg"))
}

struct FakeSnapshotApi {
    handle: Arc<Mutex<Option<FileHandle>>>,
}

#[async_trait::async_trait]
impl SnapshotApi for FakeSnapshotApi {
    async fn take_snapshot(
        &self,
        _ctx: &MediaRequestContext,
        request: SnapshotRequest,
    ) -> cheetah_media_api::error::Result<SnapshotHandle> {
        let handle = self
            .handle
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| FileHandle("missing".to_string()));
        Ok(SnapshotHandle {
            snapshot_id: SnapshotId("native-snap".to_string()),
            media_key: request.media_key,
            state: SnapshotState::Completed,
            path_handle: handle,
            download_url: None,
            created_at: 0,
        })
    }

    async fn query_snapshots(
        &self,
        _ctx: &MediaRequestContext,
        query: SnapshotQuery,
    ) -> cheetah_media_api::error::Result<Page<SnapshotInfo>> {
        let handle = self.handle.lock().unwrap().clone();
        let items = if query.snapshot_id.as_deref() == Some("native-snap") {
            handle
                .map(|h| SnapshotInfo {
                    snapshot_id: SnapshotId("native-snap".to_string()),
                    media_key: MediaKey::with_default_vhost("live", "snap-test", None)
                        .expect("valid key"),
                    state: SnapshotState::Completed,
                    path_handle: h,
                    created_at: 0,
                    size_bytes: Some(0),
                    format: "jpg".to_string(),
                    width: 8,
                    height: 6,
                })
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };
        Ok(Page {
            items,
            total: 0,
            page: 0,
            page_size: 0,
            next_cursor: None,
        })
    }

    async fn delete_snapshots(
        &self,
        _ctx: &MediaRequestContext,
        _request: DeleteSnapshotRequest,
    ) -> cheetah_media_api::error::Result<DeleteBatchResult> {
        Ok(DeleteBatchResult {
            matched: 1,
            deleted: 1,
            failed: 0,
            failures: Vec::new(),
        })
    }
}

struct SnapshotFixtureModule {
    state: ModuleState,
    handle: Arc<Mutex<Option<FileHandle>>>,
}

impl SnapshotFixtureModule {
    fn new() -> Self {
        Self {
            state: ModuleState::Created,
            handle: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait::async_trait]
impl Module for SnapshotFixtureModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new("native-snapshot-fixture"),
            display_name: "Native Snapshot Fixture".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        let ctx_req = MediaRequestContext::default();

        let n = FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut file_path = std::env::temp_dir();
        file_path.push(format!("cheetah_native_snapshot_{n}.jpg"));
        let contents = make_jpeg_payload(8, 6);
        std::fs::write(&file_path, &contents).map_err(|e| SdkError::Internal(e.to_string()))?;

        let entry = FileStoreEntry {
            media_key: MediaKey::with_default_vhost("live", "snap-test", None)
                .map_err(|e| SdkError::InvalidArgument(e.to_string()))?,
            file_type: "snapshot".to_string(),
            content_type: "image/jpeg".to_string(),
            size_bytes: contents.len() as u64,
            created_at_ms: 0,
            expires_at_ms: None,
            absolute_path: file_path.to_string_lossy().to_string(),
            owner_principal: None,
            allowed_principals: Vec::new(),
        };
        let file_handle = ctx
            .engine
            .media_file_store
            .register_file(&ctx_req, entry)
            .map_err(|e| SdkError::Internal(e.to_string()))?;
        *self.handle.lock().unwrap() = Some(file_handle);

        let snapshot = Arc::new(FakeSnapshotApi {
            handle: self.handle.clone(),
        });
        ctx.engine.media_services.register_snapshot(snapshot);

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
        Vec::new()
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        None
    }
}

struct SnapshotFixtureModuleFactory;

impl ModuleFactory for SnapshotFixtureModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new("native-snapshot-fixture"),
            display_name: "Native Snapshot Fixture".to_string(),
            dependencies: Vec::new(),
            config_namespace: "native-snapshot-fixture".to_string(),
            routes_prefix: String::new(),
            capabilities: vec![ModuleCapability::BackgroundJob],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(SnapshotFixtureModule::new())
    }

    fn config_schema(&self) -> Option<cheetah_sdk::ModuleSchemaRegistration> {
        None
    }
}

fn make_engine() -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({
        "media": {
            "native": { "auth": { "mode": "none" } }
        }
    }));

    let runtime = Arc::new(TokioRuntime::new());
    let schema_registry = config.clone();
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .with_config_schema_registry(schema_registry)
        .register_module_factory(Arc::new(SnapshotFixtureModuleFactory))
        .register_module_factory(Arc::new(NativeMediaModuleFactory))
        .build()
        .expect("engine build");
    Arc::new(engine)
}

fn get(path: &str, query: Option<String>) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        path: path.to_string(),
        query,
        headers: vec![],
        body: Bytes::new(),
    }
}

fn delete(path: &str, body: serde_json::Value) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Delete,
        path: path.to_string(),
        query: None,
        headers: vec![],
        body: Bytes::from(serde_json::to_vec(&body).unwrap()),
    }
}

fn body_json(resp: &HttpResponse) -> serde_json::Value {
    serde_json::from_slice(&resp.body).unwrap_or_else(|_| json!({}))
}

async fn native_service(engine: &cheetah_engine::Engine) -> Arc<dyn ModuleHttpService> {
    let mount = engine
        .module_manager_api()
        .http_mounts()
        .into_iter()
        .find(|m| m.module_id.0 == "media-http-native")
        .expect("native mount");
    mount.service.clone()
}

#[tokio::test(flavor = "current_thread")]
async fn native_snapshot_download_returns_image_bytes() {
    let engine = make_engine();
    engine.start().await.expect("engine start");
    let service = native_service(&engine).await;

    let resp = service
        .handle(get("/snapshots/native-snap/download", None))
        .await
        .unwrap();
    assert_eq!(resp.status, 200);

    let content_type = resp
        .headers
        .iter()
        .find(|h| h.name.to_lowercase() == "content-type")
        .map(|h| h.value.clone())
        .unwrap();
    assert_eq!(content_type, "image/jpeg");

    let content_length = resp
        .headers
        .iter()
        .find(|h| h.name.to_lowercase() == "content-length")
        .map(|h| h.value.parse::<usize>().unwrap())
        .unwrap();
    assert_eq!(content_length, resp.body.len());

    let disposition = resp
        .headers
        .iter()
        .find(|h| h.name.to_lowercase() == "content-disposition")
        .map(|h| h.value.clone())
        .unwrap();
    assert!(disposition.contains("native-snap.jpg"));

    assert!(resp.body.starts_with(b"\xff\xd8"));
}

#[tokio::test(flavor = "current_thread")]
async fn native_snapshot_download_unknown_returns_404() {
    let engine = make_engine();
    engine.start().await.expect("engine start");
    let service = native_service(&engine).await;

    let resp = service
        .handle(get("/snapshots/unknown/download", None))
        .await
        .unwrap();
    assert_eq!(resp.status, 404);
    let body = body_json(&resp);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test(flavor = "current_thread")]
async fn native_snapshot_delete_invalid_request_returns_400() {
    let engine = make_engine();
    engine.start().await.expect("engine start");
    let service = native_service(&engine).await;

    let resp = service
        .handle(delete("/snapshots/directories", json!({ "invalid": true })))
        .await
        .unwrap();
    assert_eq!(resp.status, 400);
    let body = body_json(&resp);
    assert_eq!(body["error"]["code"], "invalid_argument");
}
