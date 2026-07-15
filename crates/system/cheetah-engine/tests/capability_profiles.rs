use std::sync::Arc;

use async_trait::async_trait;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_api::command::{
    DeleteRecordRequest, RecordFileQuery, RecordPlaybackCommand, RecordTaskQuery, RtpConnectRequest,
    RtpQuery, RtpReceiverRequest, RtpSenderRequest, StartRecordRequest, StopRecordRequest,
    UpdateRtpRequest,
};
use cheetah_media_api::error::Result as MediaResult;
use cheetah_media_api::ids::{RecordFileId, RtpSessionId};
use cheetah_media_api::model::{Page, RecordFile, RecordTask, RtpSession};
use cheetah_media_api::port::{MediaFacade, MediaRequestContext, RecordApi, RtpApi};
use cheetah_media_api::{MediaCapability, MediaCapabilitySet};
use cheetah_media_module::{NativeMediaModuleFactory, ZlmMediaModuleFactory};
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{
    CancellationToken, ConfigEffect, Module, ModuleFactory, ModuleId, ModuleInfo,
    ModuleInitContext, ModuleManifest, ModuleState, SdkError,
};
use serde_json::json;

fn make_engine_with_optional_modules(
    extra: Vec<Arc<dyn ModuleFactory>>,
) -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({}));
    let runtime = Arc::new(TokioRuntime::new());
    let mut builder = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(NativeMediaModuleFactory))
        .register_module_factory(Arc::new(ZlmMediaModuleFactory));
    for factory in extra {
        builder = builder.register_module_factory(factory);
    }
    Arc::new(builder.build().expect("engine build"))
}

#[tokio::test(flavor = "current_thread")]
async fn default_profile_does_not_claim_optional_capabilities() {
    let engine = make_engine_with_optional_modules(vec![]);
    engine.start().await.expect("engine start");

    let caps = engine.media_facade().capabilities();
    assert!(caps.has(MediaCapability::Query));
    assert!(caps.has(MediaCapability::SessionControl));
    assert!(caps.has(MediaCapability::Publish));
    assert!(caps.has(MediaCapability::Subscribe));
    assert!(
        !caps.has(MediaCapability::Record),
        "default profile should not advertise record"
    );
    assert!(
        !caps.has(MediaCapability::Rtp),
        "default profile should not advertise rtp"
    );
    assert!(
        !caps.has(MediaCapability::Snapshot),
        "default profile should not advertise snapshot"
    );
    assert!(
        !caps.has(MediaCapability::Proxy),
        "default profile should not advertise proxy"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn full_profile_advertises_optional_capabilities_when_providers_registered() {
    let engine = make_engine_with_optional_modules(vec![Arc::new(CapabilityInjectorFactory)]);
    engine.start().await.expect("engine start");

    let caps = engine.media_facade().capabilities();
    assert!(caps.has(MediaCapability::Query));
    assert!(caps.has(MediaCapability::SessionControl));
    assert!(caps.has(MediaCapability::Publish));
    assert!(caps.has(MediaCapability::Subscribe));
    assert!(
        caps.has(MediaCapability::Record),
        "full profile should advertise record when a record provider is registered"
    );
    assert!(
        caps.has(MediaCapability::Rtp),
        "full profile should advertise rtp when an rtp provider is registered"
    );
    assert!(
        !caps.has(MediaCapability::Snapshot),
        "snapshot is still unavailable without a snapshot provider"
    );
    assert!(
        !caps.has(MediaCapability::Proxy),
        "proxy is still unavailable without a proxy provider"
    );
}

struct DummyRecordApi;
struct DummyRtpApi;
struct CapabilityInjectorModule;
struct CapabilityInjectorFactory;

#[async_trait]
impl RecordApi for DummyRecordApi {
    async fn start_record(
        &self,
        _ctx: &MediaRequestContext,
        _request: StartRecordRequest,
    ) -> MediaResult<RecordTask> {
        unimplemented!("dummy record provider")
    }

    async fn stop_record(
        &self,
        _ctx: &MediaRequestContext,
        _request: StopRecordRequest,
    ) -> MediaResult<RecordTask> {
        unimplemented!("dummy record provider")
    }

    async fn query_record_tasks(
        &self,
        _ctx: &MediaRequestContext,
        _query: RecordTaskQuery,
    ) -> MediaResult<Page<RecordTask>> {
        unimplemented!("dummy record provider")
    }

    async fn query_record_files(
        &self,
        _ctx: &MediaRequestContext,
        _query: RecordFileQuery,
    ) -> MediaResult<Page<RecordFile>> {
        unimplemented!("dummy record provider")
    }

    async fn delete_record_file(
        &self,
        _ctx: &MediaRequestContext,
        _request: DeleteRecordRequest,
    ) -> MediaResult<()> {
        unimplemented!("dummy record provider")
    }

    async fn control_record_playback(
        &self,
        _ctx: &MediaRequestContext,
        _file_id: &RecordFileId,
        _command: RecordPlaybackCommand,
    ) -> MediaResult<()> {
        unimplemented!("dummy record provider")
    }
}

#[async_trait]
impl RtpApi for DummyRtpApi {
    async fn open_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        _request: RtpReceiverRequest,
    ) -> MediaResult<RtpSession> {
        unimplemented!("dummy rtp provider")
    }

    async fn connect_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        _request: RtpConnectRequest,
    ) -> MediaResult<RtpSession> {
        unimplemented!("dummy rtp provider")
    }

    async fn open_rtp_sender(
        &self,
        _ctx: &MediaRequestContext,
        _request: RtpSenderRequest,
    ) -> MediaResult<RtpSession> {
        unimplemented!("dummy rtp provider")
    }

    async fn stop_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &RtpSessionId,
    ) -> MediaResult<()> {
        unimplemented!("dummy rtp provider")
    }

    async fn list_rtp_sessions(
        &self,
        _ctx: &MediaRequestContext,
        _query: RtpQuery,
    ) -> MediaResult<Page<RtpSession>> {
        unimplemented!("dummy rtp provider")
    }

    async fn update_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        _request: UpdateRtpRequest,
    ) -> MediaResult<RtpSession> {
        unimplemented!("dummy rtp provider")
    }

    async fn get_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &RtpSessionId,
    ) -> MediaResult<RtpSession> {
        unimplemented!("dummy rtp provider")
    }
}

#[async_trait]
impl Module for CapabilityInjectorModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new("capability-injector"),
            display_name: "Capability Injector".to_string(),
            state: ModuleState::Initialized,
        }
    }

    fn state(&self) -> ModuleState {
        ModuleState::Initialized
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        let mut record_caps = MediaCapabilitySet::empty();
        record_caps.add(MediaCapability::Record, 1);
        ctx.engine
            .media_services
            .register_record_with_capabilities(Arc::new(DummyRecordApi), record_caps);

        let mut rtp_caps = MediaCapabilitySet::empty();
        rtp_caps.add(MediaCapability::Rtp, 1);
        ctx.engine
            .media_services
            .register_rtp_with_capabilities(Arc::new(DummyRtpApi), rtp_caps);

        Ok(())
    }

    async fn start(&mut self, _cancel: CancellationToken) -> Result<(), SdkError> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        Ok(())
    }

    async fn apply_config(
        &mut self,
        _change: cheetah_sdk::ModuleConfigChange,
    ) -> Result<ConfigEffect, SdkError> {
        Ok(ConfigEffect::Immediate)
    }
}

impl ModuleFactory for CapabilityInjectorFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new("capability-injector"),
            display_name: "Capability Injector".to_string(),
            dependencies: Vec::new(),
            config_namespace: "capability-injector".to_string(),
            routes_prefix: "/api/v1/injector".to_string(),
            capabilities: Vec::new(),
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(CapabilityInjectorModule)
    }
}
