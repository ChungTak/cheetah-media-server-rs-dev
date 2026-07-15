//! Golden fixtures for ZLMediaKit-compatible routes.
//!
//! These integration tests exercise the full engine path for the ZLM adapter:
//! stream/session directory, record provider, RTP orchestrator, fake snapshot
//! provider, and the public file store. They verify that the endpoint-specific
//! DTOs produced in S8-T6 serialize correctly over the real `ModuleHttpService`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_api::command::{
    DeleteSnapshotRequest, PublishRequest, SnapshotQuery, SnapshotRequest,
};
use cheetah_media_api::ids::{FileHandle, MediaKey, SnapshotId};
use cheetah_media_api::media_file_store::FileStoreEntry;
use cheetah_media_api::model::{Page, SnapshotHandle, SnapshotInfo, SnapshotState};
use cheetah_media_api::port::{MediaRequestContext, SnapshotApi};
use cheetah_media_module::ZlmMediaModuleFactory;
use cheetah_proxy_module::ProxyModuleFactory;
use cheetah_record_module::RecordModuleFactory;
use cheetah_rtp_module::RtpModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{
    ConfigApplyApi, ConfigEffect, HttpMethod, HttpRequest, HttpResponse, HttpRouteDescriptor,
    Module, ModuleCapability, ModuleConfigChange, ModuleFactory, ModuleHttpService, ModuleId,
    ModuleInfo, ModuleInitContext, ModuleManifest, ModuleState, SdkError,
};
use serde_json::json;

static FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Test-only snapshot provider that returns a pre-registered file handle.
struct FakeSnapshotApi {
    handle: Arc<Mutex<Option<FileHandle>>>,
}

#[async_trait]
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
            snapshot_id: SnapshotId("golden-snap".to_string()),
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
        Ok(Page {
            items: Vec::new(),
            page: query.page,
            page_size: query.page_size,
            total: 0,
            next_cursor: None,
        })
    }

    async fn delete_snapshot_directory(
        &self,
        _ctx: &MediaRequestContext,
        _request: DeleteSnapshotRequest,
    ) -> cheetah_media_api::error::Result<()> {
        Ok(())
    }
}

/// Module that seeds the engine with a golden publisher, public file, and fake
/// snapshot provider so that ZLM routes have real objects to query.
struct GoldenFixtureModule {
    state: ModuleState,
    handle: Arc<Mutex<Option<FileHandle>>>,
}

#[async_trait]
impl Module for GoldenFixtureModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new("golden-fixtures"),
            display_name: "Golden Fixtures".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        let ctx_req = MediaRequestContext::default();

        // Register a public download file.
        let n = FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut file_path = std::env::temp_dir();
        file_path.push(format!("cheetah_zlm_golden_download_{n}.bin"));
        let contents = b"golden file contents".to_vec();
        std::fs::write(&file_path, &contents).map_err(|e| SdkError::Internal(e.to_string()))?;
        let entry = FileStoreEntry {
            media_key: MediaKey::with_default_vhost("live", "download", None)
                .map_err(|e| SdkError::InvalidArgument(e.to_string()))?,
            file_type: "download".to_string(),
            content_type: "application/octet-stream".to_string(),
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

        // Register the fake snapshot provider so /api/getSnap can be exercised.
        let snapshot = Arc::new(FakeSnapshotApi {
            handle: self.handle.clone(),
        });
        ctx.engine.media_services.register_snapshot(snapshot);

        // Acquire a golden publisher so media/session endpoints have a real stream.
        let request = PublishRequest {
            media_key: MediaKey::with_default_vhost("live", "golden", None)
                .map_err(|e| SdkError::InvalidArgument(e.to_string()))?,
            protocol: "test".to_string(),
            origin: None,
            remote_endpoint: None,
            lease_token: None,
            auth_context: HashMap::new(),
            metadata: HashMap::new(),
        };
        let publish_api = ctx
            .engine
            .media_services
            .publish_subscribe()
            .ok_or_else(|| SdkError::Unavailable("publish_subscribe missing".to_string()))?;
        publish_api
            .acquire_publisher(&ctx_req, request)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

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

struct GoldenFixtureModuleFactory;

impl ModuleFactory for GoldenFixtureModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new("golden-fixtures"),
            display_name: "Golden Fixtures".to_string(),
            dependencies: Vec::new(),
            config_namespace: "golden-fixtures".to_string(),
            routes_prefix: String::new(),
            capabilities: vec![ModuleCapability::BackgroundJob],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(GoldenFixtureModule {
            state: ModuleState::Created,
            handle: Arc::new(Mutex::new(None)),
        })
    }

    fn config_schema(&self) -> Option<cheetah_sdk::ModuleSchemaRegistration> {
        None
    }
}

fn make_engine() -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({
        "media": {
            "zlm": {
                "auth": { "mode": "none" }
            }
        }
    }));

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let record_dir = std::env::temp_dir().join(format!("cheetah_zlm_golden_record_{ts}"));
    let _ = std::fs::create_dir_all(&record_dir);
    config
        .apply_module_patch(
            &ModuleId::new("record"),
            json!({ "root_path": record_dir.to_string_lossy().to_string() }),
            ConfigEffect::Immediate,
        )
        .expect("apply record root path");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .with_config_schema_registry(config)
        .register_module_factory(Arc::new(GoldenFixtureModuleFactory))
        .register_module_factory(Arc::new(RecordModuleFactory))
        .register_module_factory(Arc::new(ProxyModuleFactory))
        .register_module_factory(Arc::new(RtpModuleFactory))
        .register_module_factory(Arc::new(ZlmMediaModuleFactory))
        .build()
        .expect("engine build");
    Arc::new(engine)
}

async fn zlm_service(engine: &cheetah_engine::Engine) -> Arc<dyn ModuleHttpService> {
    let mount = engine
        .module_manager_api()
        .http_mounts()
        .into_iter()
        .find(|m| m.module_id.0 == "media-http-zlm")
        .expect("zlm mount");
    mount.service.clone()
}

fn get(path: &str, query: Option<String>) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        path: path.to_string(),
        query,
        headers: Vec::new(),
        body: Bytes::new(),
    }
}

fn post(path: &str, body: serde_json::Value) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Post,
        path: path.to_string(),
        query: None,
        headers: Vec::new(),
        body: Bytes::from(serde_json::to_vec(&body).unwrap()),
    }
}

fn body_json(resp: &HttpResponse) -> serde_json::Value {
    serde_json::from_slice(&resp.body).unwrap_or_else(|_| json!({}))
}

fn query_for(stream: &str) -> Option<String> {
    Some(format!("vhost=__defaultVhost__&app=live&stream={stream}"))
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_golden_media_and_session_endpoints() {
    let engine = make_engine();
    engine.start().await.expect("engine start");
    let service = zlm_service(&engine).await;

    // getAllSession returns the golden publisher session.
    let resp = service
        .handle(get("/api/getAllSession", None))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    let sessions = body["data"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    let session_id = sessions[0]["id"].as_str().unwrap().to_string();

    // getMediaList contains the golden stream.
    let resp = service
        .handle(get("/api/getMediaList", query_for("golden")))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    let list = body["data"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["app"], "live");
    assert_eq!(list[0]["stream"], "golden");

    // getMediaPlayerList is empty because no subscribers.
    let resp = service
        .handle(get("/api/getMediaPlayerList", query_for("golden")))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert!(body["data"].as_array().unwrap().is_empty());

    // isMediaOnline is true while the publisher is held.
    let resp = service
        .handle(get("/api/isMediaOnline", query_for("golden")))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["online"], true);

    // getMediaInfo returns the flattened MediaItem.
    let resp = service
        .handle(get("/api/getMediaInfo", query_for("golden")))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["app"], "live");
    assert_eq!(body["stream"], "golden");

    // kick_session removes the golden publisher.
    let resp = service
        .handle(post("/api/kick_session", json!({ "id": session_id })))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["msg"], "success");

    // getAllSession is now empty.
    let resp = service
        .handle(get("/api/getAllSession", None))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert!(body["data"].as_array().unwrap().is_empty());

    // isMediaOnline is false after the publisher has been kicked.
    let resp = service
        .handle(get("/api/isMediaOnline", query_for("golden")))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["online"], false);

    // close_stream returns the ZLM close result shape.
    let resp = service
        .handle(post(
            "/api/close_stream",
            json!({ "app": "live", "stream": "golden" }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["result"], 0);

    // close_streams on an absent stream returns zero hits.
    let resp = service
        .handle(post(
            "/api/close_streams",
            json!({ "app": "live", "stream": "other" }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["count_hit"], 0);
    assert_eq!(body["count_closed"], 0);

    // kick_sessions on an absent stream returns zero hits.
    let resp = service
        .handle(post(
            "/api/kick_sessions",
            json!({ "app": "live", "stream": "other" }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["msg"], "success");
    assert_eq!(body["count_hit"], 0);
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_golden_record_endpoints() {
    let engine = make_engine();
    engine.start().await.expect("engine start");
    let service = zlm_service(&engine).await;

    // startRecord begins a record task.
    let resp = service
        .handle(post(
            "/api/startRecord",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "record-test",
                "type": "mp4"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["result"], true);
    assert!(body["taskId"].as_str().is_some());

    // isRecording reflects the running task.
    let resp = service
        .handle(get("/api/isRecording", query_for("record-test")))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["status"], true);

    // getMP4RecordFile returns the typed Mp4FilesData envelope.
    let resp = service
        .handle(get("/api/getMP4RecordFile", query_for("record-test")))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert!(body["data"]["paths"].is_array());
    assert_eq!(body["data"]["rootPath"], "");

    // startRecordTask with explicit id.
    let resp = service
        .handle(post(
            "/api/startRecordTask",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "record-task",
                "type": "mp4",
                "id": "task-1"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["taskId"], "task-1");

    // stopRecord stops the record-test task.
    let resp = service
        .handle(post(
            "/api/stopRecord",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "record-test",
                "type": "mp4"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["result"], true);

    // deleteRecordDirectory returns counts.
    let resp = service
        .handle(post(
            "/api/deleteRecordDirectory",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "record-test"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["result"], true);
    assert_eq!(body["deleted"], 0);
    assert_eq!(body["failed"], 0);

    // L2 playback controls error path: missing file returns -500.
    let resp = service
        .handle(post(
            "/api/setRecordSpeed",
            json!({ "file_id": "missing", "speed": 1.0 }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], -500);

    let resp = service
        .handle(post(
            "/api/seekRecordStamp",
            json!({ "file_id": "missing", "seek": 0 }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], -500);

    let resp = service
        .handle(post(
            "/api/controlRecordPlay",
            json!({ "file_id": "missing", "command": "pause" }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], -500);
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_golden_rtp_endpoints() {
    let engine = make_engine();
    engine.start().await.expect("engine start");
    let service = zlm_service(&engine).await;

    // Open a UDP RTP receiver.
    let resp = service
        .handle(post(
            "/api/openRtpServer",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "rtp-udp"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert!(body["port"].as_u64().is_some());
    assert_eq!(body["session_id"].as_str().unwrap(), "recv/live/rtp-udp");

    // listRtpServer returns the receiver.
    let resp = service
        .handle(get("/api/listRtpServer", None))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    let servers = body["data"].as_array().unwrap();
    assert_eq!(servers.len(), 1);

    // closeRtpServer returns hit:1.
    let resp = service
        .handle(post(
            "/api/closeRtpServer",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "rtp-udp"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["hit"], 1);

    // Active TCP receiver followed by connect and talk.
    let resp = service
        .handle(post(
            "/api/openRtpServer",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "rtp-recv",
                "tcp_mode": "active"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);

    let resp = service
        .handle(post(
            "/api/connectRtpServer",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "rtp-recv",
                "dst_url": "127.0.0.1:5000"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["session_id"].as_str().unwrap(), "recv/live/rtp-recv");

    let resp = service
        .handle(post(
            "/api/startSendRtpTalk",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "rtp-recv"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["session_id"].as_str().unwrap(), "recv/live/rtp-recv");

    let resp = service
        .handle(get("/api/getRtpInfo", query_for("rtp-recv")))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["exist"], true);

    let resp = service
        .handle(post(
            "/api/closeRtpServer",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "rtp-recv"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["hit"], 1);

    // Active UDP sender lifecycle.
    let resp = service
        .handle(post(
            "/api/startSendRtp",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "rtp-send",
                "dst_url": "127.0.0.1:5001"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["session_id"].as_str().unwrap(), "send/live/rtp-send");

    let resp = service
        .handle(get("/api/listRtpSender", None))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);

    let resp = service
        .handle(get("/api/getRtpInfo", query_for("rtp-send")))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["exist"], true);
    assert_eq!(body["peerIp"], "127.0.0.1");
    assert_eq!(body["peerPort"], 5001);

    let resp = service
        .handle(post(
            "/api/stopSendRtp",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "rtp-send"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);

    let resp = service
        .handle(get("/api/getRtpInfo", query_for("rtp-send")))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["exist"], false);

    // Passive UDP sender.
    let resp = service
        .handle(post(
            "/api/startSendRtpPassive",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "rtp-passive",
                "dst_url": "127.0.0.1:5002"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(
        body["session_id"].as_str().unwrap(),
        "send/live/rtp-passive"
    );

    let resp = service
        .handle(get("/api/listRtpSender", None))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);

    let resp = service
        .handle(post(
            "/api/stopSendRtp",
            json!({
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "rtp-passive"
            }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_golden_snapshot_and_download() {
    let engine = make_engine();
    engine.start().await.expect("engine start");
    let service = zlm_service(&engine).await;

    // getSnap returns a SnapshotHandle pointing at the pre-registered file.
    let resp = service
        .handle(get("/api/getSnap", query_for("snap-test")))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    let handle = body["data"]["path_handle"].as_str().unwrap().to_string();
    assert_eq!(body["data"]["state"], "completed");

    // downloadFile streams the registered file back.
    let resp = service
        .handle(get(
            "/api/downloadFile",
            Some(format!("file_path={handle}")),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, Bytes::from_static(b"golden file contents"));
    let content_length = resp
        .headers
        .iter()
        .find(|h| h.name.to_lowercase() == "content-length")
        .map(|h| h.value.parse::<usize>().unwrap())
        .unwrap();
    assert_eq!(content_length, resp.body.len());

    // deleteSnapDirectory returns success.
    let resp = service
        .handle(post(
            "/api/deleteSnapDirectory",
            json!({ "vhost": "__defaultVhost__", "app": "live", "stream": "snap-test" }),
        ))
        .await
        .unwrap();
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
    assert_eq!(body["result"], true);
}
