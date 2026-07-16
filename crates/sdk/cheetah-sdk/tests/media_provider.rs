use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::media_api::capability::{CapabilityState, MediaCapability, MediaCapabilitySet};
use cheetah_sdk::media_api::command::{
    MediaQuery, OpenPlaybackRequest, PlaybackControl, PlaybackQuery, SessionQuery,
};
use cheetah_sdk::media_api::error::Result as MediaResult;
use cheetah_sdk::media_api::ids::{MediaKey, MediaSchema, PlaybackSessionId, SessionId};
use cheetah_sdk::media_api::model::{
    CloseReason, CloseReport, OnlineState, Page, PlaybackSession, PlaybackSessionState,
    SessionInfo, StreamInfo,
};
use cheetah_sdk::media_api::output::MediaOutputEndpoint;
use cheetah_sdk::media_api::port::{
    MediaAdmissionApi, MediaControlApi, MediaRequestContext, PlaybackApi,
};
use cheetah_sdk::media_api::{AdmissionRequest, Decision};
use cheetah_sdk::module::MediaServices;
use cheetah_sdk::output::InMemoryMediaOutputRegistry;

struct DummyControl;

#[async_trait]
impl MediaControlApi for DummyControl {
    async fn get_media_list(
        &self,
        _ctx: &MediaRequestContext,
        _query: MediaQuery,
    ) -> MediaResult<Page<StreamInfo>> {
        unimplemented!()
    }

    async fn get_media(
        &self,
        _ctx: &MediaRequestContext,
        _key: &MediaKey,
    ) -> MediaResult<StreamInfo> {
        unimplemented!()
    }

    async fn is_media_online(
        &self,
        _ctx: &MediaRequestContext,
        _key: &MediaKey,
    ) -> MediaResult<OnlineState> {
        unimplemented!()
    }

    async fn list_sessions(
        &self,
        _ctx: &MediaRequestContext,
        _query: SessionQuery,
    ) -> MediaResult<Page<SessionInfo>> {
        unimplemented!()
    }

    async fn kick_session(
        &self,
        _ctx: &MediaRequestContext,
        _id: &SessionId,
        _reason: CloseReason,
    ) -> MediaResult<()> {
        unimplemented!()
    }

    async fn kick_stream(
        &self,
        _ctx: &MediaRequestContext,
        _key: &MediaKey,
        _reason: CloseReason,
    ) -> MediaResult<CloseReport> {
        unimplemented!()
    }

    async fn request_keyframe(
        &self,
        _ctx: &MediaRequestContext,
        _key: &MediaKey,
    ) -> MediaResult<()> {
        unimplemented!()
    }
}

struct DummyAdmission;

#[async_trait]
impl MediaAdmissionApi for DummyAdmission {
    async fn authorize(
        &self,
        _ctx: &MediaRequestContext,
        _request: AdmissionRequest,
    ) -> MediaResult<Decision> {
        Ok(Decision::Allow)
    }
}

struct DummyPlayback;

#[async_trait]
impl PlaybackApi for DummyPlayback {
    async fn open_playback(
        &self,
        _ctx: &MediaRequestContext,
        request: OpenPlaybackRequest,
    ) -> MediaResult<PlaybackSession> {
        Ok(PlaybackSession {
            session_id: PlaybackSessionId("pb-1".to_string()),
            media_key: request.media_key,
            file_handle: request.file_handle,
            state: PlaybackSessionState::Pending,
            duration_ms: 0,
            position_ms: request.start_position_ms,
            scale: request.scale,
            generation: 1,
            output_key: None,
            last_error: None,
            created_at: 0,
            updated_at: 0,
        })
    }

    async fn get_playback(
        &self,
        _ctx: &MediaRequestContext,
        _id: &PlaybackSessionId,
    ) -> MediaResult<PlaybackSession> {
        unimplemented!()
    }

    async fn list_playbacks(
        &self,
        _ctx: &MediaRequestContext,
        _query: PlaybackQuery,
    ) -> MediaResult<Page<PlaybackSession>> {
        unimplemented!()
    }

    async fn control_playback(
        &self,
        _ctx: &MediaRequestContext,
        _id: &PlaybackSessionId,
        _command: PlaybackControl,
    ) -> MediaResult<PlaybackSession> {
        unimplemented!()
    }

    async fn stop_playback(
        &self,
        _ctx: &MediaRequestContext,
        _id: &PlaybackSessionId,
    ) -> MediaResult<()> {
        unimplemented!()
    }
}

struct DummyAdmission;

#[async_trait]
impl MediaAdmissionApi for DummyAdmission {
    async fn authorize(
        &self,
        _ctx: &MediaRequestContext,
        _request: AdmissionRequest,
    ) -> MediaResult<Decision> {
        Ok(Decision::Allow)
    }
}

#[test]
fn default_capabilities_are_empty() {
    let services = MediaServices::unavailable();
    let caps = services.capabilities();
    assert!(caps.capabilities.is_empty());
}

#[test]
fn register_control_updates_capabilities() {
    let services = MediaServices::unavailable();
    services.register_control(Arc::new(DummyControl));
    let caps = services.capabilities();
    assert!(caps.has(MediaCapability::Query));
    assert!(caps.has(MediaCapability::SessionControl));
}

#[test]
fn register_playback_updates_capabilities() {
    let services = MediaServices::unavailable();
    services.register_playback(Arc::new(DummyPlayback));
    let caps = services.capabilities();
    assert!(caps.has(MediaCapability::Playback));
    assert!(services.playback().is_some());
}

#[test]
fn unregister_with_stale_registration_is_noop() {
    let services = MediaServices::unavailable();
    let reg = services.register_control(Arc::new(DummyControl));
    services.register_control(Arc::new(DummyControl));
    assert!(
        !services.unregister(&reg),
        "stale registration must not remove newer provider"
    );
    assert!(services.control().is_some());
}

#[test]
fn unregister_with_current_registration_removes_provider() {
    let services = MediaServices::unavailable();
    let reg = services.register_control(Arc::new(DummyControl));
    assert!(services.unregister(&reg));
    assert!(services.control().is_none());
    assert!(!services.capabilities().has(MediaCapability::Query));
}

#[test]
fn capability_report_contains_provider_id_and_default_operations() {
    let services = MediaServices::unavailable();
    let mut caps = MediaCapabilitySet::empty();
    caps.add(MediaCapability::Query, 2);
    services.register_control_with_capabilities(Arc::new(DummyControl), caps);
    let report = services.capability_report();
    assert!(report.generation > 0);
    let query = report
        .descriptors
        .iter()
        .find(|d| d.capability == MediaCapability::Query)
        .expect("query descriptor");
    assert!(query.provider_id.starts_with("control:"));
    assert_eq!(query.version, 2);
    assert_eq!(query.state, CapabilityState::Available);
    assert!(!query.operations.is_empty());
}

#[test]
fn capability_report_generation_advances_on_register_and_unregister() {
    let services = MediaServices::unavailable();
    let gen0 = services.capability_report().generation;
    let reg = services.register_control(Arc::new(DummyControl));
    let gen1 = services.capability_report().generation;
    assert!(gen1 > gen0);
    let reg2 = services.register_control(Arc::new(DummyControl));
    let gen2 = services.capability_report().generation;
    assert!(gen2 > gen1);
    services.unregister(&reg2);
    let gen3 = services.capability_report().generation;
    assert!(gen3 > gen2);
    assert!(!services.unregister(&reg));
}

#[test]
fn capability_report_descriptors_sorted_by_capability_then_provider_id() {
    let services = MediaServices::unavailable();
    let mut caps = MediaCapabilitySet::empty();
    caps.add(MediaCapability::SessionControl, 1);
    caps.add(MediaCapability::Query, 1);
    services.register_control_with_capabilities(Arc::new(DummyControl), caps);
    let report = services.capability_report();
    let keys: Vec<_> = report
        .descriptors
        .iter()
        .map(|d| (d.capability, d.provider_id.clone()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    assert_eq!(keys, sorted);
}

#[test]
fn output_registry_is_none_until_registered() {
    let services = MediaServices::unavailable();
    assert!(services.output_registry().is_none());
}

#[tokio::test]
async fn output_registry_register_and_unregister_lifecycle() {
    let services = MediaServices::unavailable();
    let registry = Arc::new(InMemoryMediaOutputRegistry::new());
    let reg = services.register_output_registry(registry.clone());
    assert!(services.output_registry().is_some());

    let out = services.output_registry().unwrap();
    let id = out
        .register_endpoint(MediaOutputEndpoint::new(
            "rtmp",
            MediaSchema::Rtmp,
            "127.0.0.1",
            1935,
            false,
            "{app}/{stream}",
        ))
        .await
        .unwrap();
    assert!(!id.is_empty());

    assert!(services.unregister_output_registry(&reg));
    assert!(services.output_registry().is_none());
    assert!(!services.unregister_output_registry(&reg));
}

#[test]
fn register_admission_updates_capabilities() {
    let services = MediaServices::unavailable();
    services.register_admission(Arc::new(DummyAdmission));
    let caps = services.capabilities();
    assert!(caps.has(MediaCapability::Admission));
    assert!(services.admission().is_some());
}

#[test]
fn unregister_admission_removes_provider() {
    let services = MediaServices::unavailable();
    let reg = services.register_admission(Arc::new(DummyAdmission));
    assert!(services.unregister(&reg));
    assert!(services.admission().is_none());
    assert!(!services.capabilities().has(MediaCapability::Admission));
}
