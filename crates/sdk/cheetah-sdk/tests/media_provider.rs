use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::media_api::capability::{CapabilityState, MediaCapability, MediaCapabilitySet};
use cheetah_sdk::media_api::command::{MediaQuery, SessionQuery};
use cheetah_sdk::media_api::error::Result as MediaResult;
use cheetah_sdk::media_api::ids::{MediaKey, SessionId};
use cheetah_sdk::media_api::model::{
    CloseReason, CloseReport, OnlineState, Page, SessionInfo, StreamInfo,
};
use cheetah_sdk::media_api::port::{MediaControlApi, MediaRequestContext};
use cheetah_sdk::module::MediaServices;

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
