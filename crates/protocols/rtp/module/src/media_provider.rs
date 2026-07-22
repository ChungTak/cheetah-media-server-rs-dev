//! Bridge `cheetah_media_api::port::RtpApi` and the typed `RtpSessionApi` to the
//! module's shared `RtpSessionOrchestrator`.
//!
//! 由模块共享的 `RtpSessionOrchestrator` 支撑的 `RtpApi` / `RtpSessionApi` provider。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_rtp_driver_tokio::RtpDriverHandle;
use cheetah_sdk::media_api::command::{
    RtpConnectRequest, RtpQuery, RtpReceiverRequest, RtpSenderMode, RtpSenderRequest,
    UpdateRtpRequest,
};
use cheetah_sdk::media_api::error::{EffectOutcome, MediaError, Result};
use cheetah_sdk::media_api::fencing::{ControlledResourceRef, ResourceOrigin};
use cheetah_sdk::media_api::ids::{
    MediaBindingId, MediaKey, MediaNodeInstanceEpoch, MediaSessionId, OwnerEpoch,
    ResourceGeneration, RtpSessionId, StreamKeyBridge, TenantId,
};
use cheetah_sdk::media_api::model::{
    Page, RtpSession, RtpSessionKind, RtpSessionState as OldRtpSessionState, RtpTcpMode,
};
use cheetah_sdk::media_api::port::{MediaRequestContext, RtpApi};
use cheetah_sdk::media_api::rtp_session::{
    GbMediaCompatibilityProfile, MediaContainer, OpenRtpReceiver, OpenRtpSender, OpenRtpTalk,
    RtpDirection, RtpEndpoints, RtpFraming, RtpPayloadBinding, RtpSessionApi, RtpSessionDescriptor,
    RtpSessionGeneration, RtpSessionParams, RtpSessionQuery, RtpSessionRef, RtpSessionState,
    RtpTransport, StopRtpSession, TcpRole, UpdateRtpSession,
};
use cheetah_sdk::{CancellationToken, Deadline, EngineContext, StreamKey};
use parking_lot::Mutex;

use crate::config::RtpModuleConfig;
use crate::egress::{run_egress_session, ActiveEgressMap, EgressCleanup};
use crate::orchestrator::RtpSessionOrchestrator;

/// Media-domain `RtpApi` and `RtpSessionApi` provider.
///
/// `RtpApi` / `RtpSessionApi` provider。
pub struct RtpMediaProvider {
    orchestrator: Arc<RtpSessionOrchestrator>,
    engine: EngineContext,
    module_cancel: CancellationToken,
    config: RtpModuleConfig,
    /// Active sender egress tasks keyed by session key so `stop_rtp_session` can cancel them.
    active_senders: ActiveEgressMap,
    /// Per-session typed descriptors carrying parameters not present in the legacy `RtpSession`.
    rtp_descriptors: Arc<Mutex<HashMap<RtpSessionId, RtpSessionDescriptor>>>,
}

impl RtpMediaProvider {
    /// Create a provider backed by the shared orchestrator.
    ///
    /// 创建由共享编排器支撑的 provider。
    pub fn new(
        orchestrator: Arc<RtpSessionOrchestrator>,
        engine: EngineContext,
        module_cancel: CancellationToken,
        config: RtpModuleConfig,
    ) -> Self {
        Self {
            orchestrator,
            engine,
            module_cancel,
            config,
            active_senders: ActiveEgressMap::default(),
            rtp_descriptors: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Return the orchestrator so the module can share the same instance with
    /// the HTTP service.
    ///
    /// 返回编排器，以便模块将它与 HTTP 服务共享。
    pub fn orchestrator(&self) -> Arc<RtpSessionOrchestrator> {
        self.orchestrator.clone()
    }

    fn driver(&self) -> Result<Arc<RtpDriverHandle>> {
        self.orchestrator.driver()
    }

    /// Enforce per-module capability/profile and session limits before allocating a session.
    fn check_profile_and_limits(&self, params: &RtpSessionParams) -> Result<()> {
        if !self.config.enabled_profiles.is_empty()
            && !self.config.enabled_profiles.contains(&params.profile)
        {
            return Err(MediaError::unsupported(format!(
                "profile {:?} is not enabled",
                params.profile
            )));
        }
        if self.orchestrator.session_count() >= self.config.max_sessions {
            return Err(MediaError::unavailable("rtp session limit reached"));
        }
        Ok(())
    }

    /// Build the `StreamKey` that the engine uses for a given `MediaKey`.
    fn stream_key_for_media_key(media_key: &MediaKey) -> StreamKey {
        let (namespace, path) = StreamKeyBridge::to_namespace_path(media_key);
        StreamKey::new(namespace, path)
    }

    fn container_to_codec_hint(container: MediaContainer) -> Option<String> {
        match container {
            MediaContainer::Ps => Some("ps".to_string()),
            MediaContainer::Ts => Some("ts".to_string()),
            MediaContainer::ElementaryStream => Some("es".to_string()),
            MediaContainer::AutoDetect | _ => None,
        }
    }

    fn payload_type_from_bindings(bindings: &[RtpPayloadBinding]) -> Option<u8> {
        bindings.first().map(|b| b.payload_type)
    }

    fn codec_hint_from_params(params: &RtpSessionParams) -> Option<String> {
        if params.container != MediaContainer::AutoDetect {
            return Self::container_to_codec_hint(params.container);
        }
        params
            .payload_bindings
            .first()
            .and_then(|b| match b.codec.to_lowercase().as_str() {
                "ps" => Some("ps".to_string()),
                "ts" => Some("ts".to_string()),
                "es" => Some("es".to_string()),
                _ => None,
            })
    }

    fn build_old_receiver_request(&self, req: OpenRtpReceiver) -> Result<RtpReceiverRequest> {
        let params = req.params;
        let (ip, port) = params
            .local_endpoint_hint
            .map(|a| (Some(a.ip().to_string()), Some(a.port())))
            .unwrap_or((None, None));
        let tcp_mode = match params.transport {
            RtpTransport::Udp => None,
            RtpTransport::Tcp => Some(match params.tcp_role.unwrap_or(TcpRole::Passive) {
                TcpRole::Active => RtpTcpMode::Active,
                TcpRole::Passive => RtpTcpMode::Passive,
                _ => RtpTcpMode::Passive,
            }),
            _ => None,
        };
        let payload_type = Self::payload_type_from_bindings(&params.payload_bindings);
        let codec_hint = Self::codec_hint_from_params(&params);
        Ok(RtpReceiverRequest {
            media_key: params.media_key,
            port,
            ip,
            ssrc: params.ssrc,
            enable_rtcp: true,
            tcp_mode,
            payload_type,
            codec_hint,
            reuse_port: false,
            timeout_ms: 0,
        })
    }

    fn build_old_sender_request(
        &self,
        req: OpenRtpSender,
        mode: RtpSenderMode,
    ) -> Result<RtpSenderRequest> {
        let params = req.params;
        let destination_endpoint = params
            .remote_endpoint
            .map(|a| a.to_string())
            .unwrap_or_else(|| "0.0.0.0:0".to_string());
        let mut transport_options = HashMap::new();
        if params.transport == RtpTransport::Tcp {
            transport_options.insert("tcp".to_string(), "true".to_string());
        }
        let payload_type = Self::payload_type_from_bindings(&params.payload_bindings);
        let codec_hint = Self::codec_hint_from_params(&params);
        Ok(RtpSenderRequest {
            media_key: params.media_key,
            destination_endpoint,
            ssrc: params.ssrc,
            payload_type,
            codec_hint,
            mode,
            transport_options,
        })
    }

    fn map_old_session_state(state: OldRtpSessionState) -> RtpSessionState {
        match state {
            OldRtpSessionState::Created | OldRtpSessionState::Listening => RtpSessionState::Ready,
            OldRtpSessionState::Connected
            | OldRtpSessionState::Bound
            | OldRtpSessionState::Paused => RtpSessionState::Active,
            OldRtpSessionState::Stopping => RtpSessionState::Draining,
            OldRtpSessionState::Stopped => RtpSessionState::Stopped,
            OldRtpSessionState::TimedOut | OldRtpSessionState::Failed => RtpSessionState::Failed,
        }
    }

    fn resource_ref_from_context(
        &self,
        ctx: &MediaRequestContext,
        session_id: &RtpSessionId,
        generation: u64,
    ) -> ControlledResourceRef {
        let (
            tenant_id,
            owner_epoch,
            node_instance_epoch,
            media_session_id,
            media_binding_id,
            origin,
        ) = if let Some(mutation) = &ctx.mutation {
            (
                mutation.tenant_id.clone(),
                mutation.owner_epoch,
                mutation.target_media_node_instance_epoch,
                mutation.media_session_id.clone(),
                mutation.media_binding_id.clone(),
                ResourceOrigin::Cluster,
            )
        } else {
            (
                TenantId::new("default").unwrap_or_else(|_| TenantId::new("cheetah").unwrap()),
                OwnerEpoch(0),
                MediaNodeInstanceEpoch(0),
                None::<MediaSessionId>,
                None::<MediaBindingId>,
                ResourceOrigin::Local,
            )
        };
        ControlledResourceRef {
            tenant_id,
            media_session_id,
            media_binding_id,
            resource_kind: "rtp_session".to_string(),
            resource_handle: session_id.0.clone(),
            owner_epoch,
            node_instance_epoch,
            generation: ResourceGeneration(generation),
            origin,
        }
    }

    /// Enrich a media error with the controlled resource reference for a session.
    fn enrich_error(
        &self,
        err: MediaError,
        ctx: &MediaRequestContext,
        session_ref: &RtpSessionRef,
    ) -> MediaError {
        err.with_resource_ref(self.resource_ref_from_context(
            ctx,
            &session_ref.session_id,
            session_ref.expected_generation.0,
        ))
    }

    fn build_descriptor(
        &self,
        ctx: &MediaRequestContext,
        session: &RtpSession,
        stored: Option<RtpSessionDescriptor>,
    ) -> Result<RtpSessionDescriptor> {
        let direction = match session.kind {
            RtpSessionKind::Receiver => RtpDirection::Receive,
            RtpSessionKind::Sender => RtpDirection::Send,
            RtpSessionKind::Talk => RtpDirection::DuplexTalk,
        };
        let (transport, tcp_role) = match session.tcp_mode {
            Some(RtpTcpMode::Active) => (RtpTransport::Tcp, Some(TcpRole::Active)),
            Some(RtpTcpMode::Passive) => (RtpTransport::Tcp, Some(TcpRole::Passive)),
            None => (RtpTransport::Udp, None),
        };
        let framing = if transport == RtpTransport::Udp {
            RtpFraming::Datagram
        } else {
            stored
                .as_ref()
                .map(|s| s.framing)
                .unwrap_or(RtpFraming::Rfc4571)
        };
        let container = stored
            .as_ref()
            .map(|s| s.container)
            .unwrap_or(MediaContainer::Ps);
        let profile = stored
            .as_ref()
            .map(|s| s.profile)
            .unwrap_or(GbMediaCompatibilityProfile::GbCommon);
        let payload_bindings = stored
            .as_ref()
            .map(|s| s.payload_bindings.clone())
            .unwrap_or_default();
        let source_binding_policy = stored
            .as_ref()
            .map(|s| s.source_binding_policy)
            .unwrap_or_default();
        let resource_ref = stored
            .as_ref()
            .map(|s| s.resource_ref.clone())
            .unwrap_or_else(|| {
                self.resource_ref_from_context(ctx, &session.session_id, session.generation)
            });

        let default_ip = self.orchestrator.default_bind_addr().ip();
        let local_port = session.local_port.unwrap_or(0);
        let local = SocketAddr::new(default_ip, local_port);
        let remote = session
            .remote_endpoint
            .as_ref()
            .and_then(|s| s.parse::<SocketAddr>().ok());
        let endpoints = RtpEndpoints {
            local,
            remote,
            rtcp_local: None,
            rtcp_remote: None,
        };

        Ok(RtpSessionDescriptor {
            session_id: session.session_id.clone(),
            generation: RtpSessionGeneration(session.generation),
            state: Self::map_old_session_state(session.state),
            direction,
            transport,
            tcp_role,
            framing,
            container,
            profile,
            endpoints,
            ssrc: session.ssrc,
            payload_bindings,
            source_binding_policy,
            resource_ref,
        })
    }

    fn apply_request_overrides(
        &self,
        descriptor: &mut RtpSessionDescriptor,
        params: &RtpSessionParams,
    ) {
        descriptor.container = params.container;
        descriptor.profile = params.profile;
        descriptor.source_binding_policy = params.source_binding_policy;
        descriptor.payload_bindings = params.payload_bindings.clone();
        if let Some(local) = params.local_endpoint_hint {
            // Keep the actually-bound port and apply only the requested local IP.
            descriptor.endpoints.local =
                SocketAddr::new(local.ip(), descriptor.endpoints.local.port());
        }
        if let Some(remote) = params.remote_endpoint {
            descriptor.endpoints.remote = Some(remote);
        }
    }
}

#[async_trait]
impl RtpApi for RtpMediaProvider {
    async fn open_rtp_receiver(
        &self,
        ctx: &MediaRequestContext,
        request: RtpReceiverRequest,
    ) -> Result<RtpSession> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        self.orchestrator.open_rtp_receiver(request).await
    }

    async fn connect_rtp_receiver(
        &self,
        ctx: &MediaRequestContext,
        request: RtpConnectRequest,
    ) -> Result<RtpSession> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        self.orchestrator.connect_rtp_receiver(request).await
    }

    async fn open_rtp_sender(
        &self,
        ctx: &MediaRequestContext,
        request: RtpSenderRequest,
    ) -> Result<RtpSession> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        // Create the driver-side sender session first.
        let session = self.orchestrator.open_rtp_sender(request.clone()).await?;
        let session_key = session.session_id.0.clone();

        // Determine the engine stream we need to subscribe to.
        let stream_key = Self::stream_key_for_media_key(&request.media_key);

        let driver = self.driver()?;
        let cancel = self.module_cancel.child_token();
        self.active_senders
            .lock()
            .insert(session_key.clone(), cancel.clone());

        let engine = self.engine.clone();
        let orchestrator = self.orchestrator.clone();
        let cleanup = EgressCleanup::new(self.active_senders.clone(), session_key.clone());
        let runtime_api = self.engine.runtime_api.clone();
        runtime_api.spawn(Box::pin(async move {
            run_egress_session(
                engine,
                driver,
                vec![session_key],
                stream_key,
                cancel,
                Some(orchestrator),
                Some(cleanup),
            )
            .await;
        }));

        Ok(session)
    }

    async fn stop_rtp_session(&self, ctx: &MediaRequestContext, id: &RtpSessionId) -> Result<()> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        if let Some(cancel) = self.active_senders.lock().remove(&id.0) {
            cancel.cancel();
        }
        self.orchestrator.stop_rtp_session(id).await
    }

    async fn list_rtp_sessions(
        &self,
        _ctx: &MediaRequestContext,
        query: RtpQuery,
    ) -> Result<Page<RtpSession>> {
        self.orchestrator.list_rtp_sessions(query)
    }

    async fn update_rtp_session(
        &self,
        ctx: &MediaRequestContext,
        request: UpdateRtpRequest,
    ) -> Result<RtpSession> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        self.orchestrator.update_rtp_session(request).await
    }

    async fn get_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        id: &RtpSessionId,
    ) -> Result<RtpSession> {
        self.orchestrator.get_rtp_session(id)
    }
}

#[async_trait]
impl RtpSessionApi for RtpMediaProvider {
    async fn open_receiver(
        &self,
        ctx: &MediaRequestContext,
        request: OpenRtpReceiver,
    ) -> Result<RtpSessionDescriptor> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        self.check_profile_and_limits(&request.params)?;
        let old_req = self.build_old_receiver_request(request.clone())?;
        let session = self.open_rtp_receiver(ctx, old_req).await?;
        let mut descriptor = self.build_descriptor(ctx, &session, None)?;
        self.apply_request_overrides(&mut descriptor, &request.params);
        self.rtp_descriptors
            .lock()
            .insert(descriptor.session_id.clone(), descriptor.clone());
        Ok(descriptor)
    }

    async fn open_sender(
        &self,
        ctx: &MediaRequestContext,
        request: OpenRtpSender,
    ) -> Result<RtpSessionDescriptor> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        self.check_profile_and_limits(&request.params)?;
        let mode = match request.params.tcp_role {
            Some(TcpRole::Passive) => RtpSenderMode::Passive,
            _ => RtpSenderMode::Active,
        };
        let old_req = self.build_old_sender_request(request.clone(), mode)?;
        let session = self.open_rtp_sender(ctx, old_req).await?;
        let mut descriptor = self.build_descriptor(ctx, &session, None)?;
        self.apply_request_overrides(&mut descriptor, &request.params);
        self.rtp_descriptors
            .lock()
            .insert(descriptor.session_id.clone(), descriptor.clone());
        Ok(descriptor)
    }

    async fn open_talk(
        &self,
        ctx: &MediaRequestContext,
        request: OpenRtpTalk,
    ) -> Result<RtpSessionDescriptor> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        self.check_profile_and_limits(&request.params)?;
        let sender_req = self.build_old_sender_request(
            OpenRtpSender {
                params: request.params.clone(),
            },
            RtpSenderMode::Talk,
        )?;
        let session = self.open_rtp_sender(ctx, sender_req).await?;
        let mut descriptor = self.build_descriptor(ctx, &session, None)?;
        self.apply_request_overrides(&mut descriptor, &request.params);
        if let Some(binding) = request.talkback_binding {
            descriptor.payload_bindings.push(binding);
        }
        self.rtp_descriptors
            .lock()
            .insert(descriptor.session_id.clone(), descriptor.clone());
        Ok(descriptor)
    }

    async fn update_session(
        &self,
        ctx: &MediaRequestContext,
        request: UpdateRtpSession,
    ) -> Result<RtpSessionDescriptor> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;

        let session = self
            .orchestrator
            .get_rtp_session(&request.session_ref.session_id)
            .map_err(|e| self.enrich_error(e, ctx, &request.session_ref))?;
        if session.generation != request.session_ref.expected_generation.0 {
            return Err(
                MediaError::conflict("generation mismatch").with_resource_ref(
                    self.resource_ref_from_context(
                        ctx,
                        &request.session_ref.session_id,
                        request.session_ref.expected_generation.0,
                    ),
                ),
            );
        }

        let payload_type = request
            .payload_bindings
            .as_ref()
            .and_then(|b| b.first().map(|b| b.payload_type));
        let old_req = UpdateRtpRequest {
            session_id: request.session_ref.session_id.clone(),
            expected_generation: request.session_ref.expected_generation.0,
            ssrc: None,
            payload_type,
            pause_check: request.pause_check,
        };
        let mut updated = self.update_rtp_session(ctx, old_req).await?;
        if let Some(remote) = request.remote_endpoint {
            updated = self
                .orchestrator
                .set_session_remote_endpoint(&request.session_ref.session_id, remote)
                .map_err(|e| self.enrich_error(e, ctx, &request.session_ref))?;
        }

        let mut descs = self.rtp_descriptors.lock();
        let stored = descs.get(&request.session_ref.session_id).cloned();
        let mut descriptor = self.build_descriptor(ctx, &updated, stored)?;
        if let Some(bindings) = request.payload_bindings {
            descriptor.payload_bindings = bindings;
        }
        if let Some(policy) = request.source_binding_policy {
            descriptor.source_binding_policy = policy;
        }
        descs.insert(descriptor.session_id.clone(), descriptor.clone());
        Ok(descriptor)
    }

    async fn get_session(
        &self,
        ctx: &MediaRequestContext,
        session_ref: RtpSessionRef,
    ) -> Result<RtpSessionDescriptor> {
        let session = self
            .orchestrator
            .get_rtp_session(&session_ref.session_id)
            .map_err(|e| self.enrich_error(e, ctx, &session_ref))?;
        if session.generation != session_ref.expected_generation.0 {
            return Err(
                MediaError::conflict("generation mismatch").with_resource_ref(
                    self.resource_ref_from_context(
                        ctx,
                        &session_ref.session_id,
                        session_ref.expected_generation.0,
                    ),
                ),
            );
        }
        let stored = self
            .rtp_descriptors
            .lock()
            .get(&session_ref.session_id)
            .cloned();
        self.build_descriptor(ctx, &session, stored)
    }

    async fn stop_session(
        &self,
        ctx: &MediaRequestContext,
        request: StopRtpSession,
    ) -> Result<EffectOutcome> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;

        match self
            .orchestrator
            .get_rtp_session(&request.session_ref.session_id)
        {
            Ok(session) => {
                if session.generation != request.session_ref.expected_generation.0 {
                    return Err(
                        MediaError::conflict("generation mismatch").with_resource_ref(
                            self.resource_ref_from_context(
                                ctx,
                                &request.session_ref.session_id,
                                request.session_ref.expected_generation.0,
                            ),
                        ),
                    );
                }
            }
            Err(_) => return Ok(EffectOutcome::NotApplied),
        }

        match self
            .stop_rtp_session(ctx, &request.session_ref.session_id)
            .await
        {
            Ok(()) => {
                self.rtp_descriptors
                    .lock()
                    .remove(&request.session_ref.session_id);
                Ok(EffectOutcome::Applied)
            }
            Err(e) => Err(self.enrich_error(e, ctx, &request.session_ref)),
        }
    }

    async fn list_sessions(
        &self,
        ctx: &MediaRequestContext,
        mut query: RtpSessionQuery,
    ) -> Result<Page<RtpSessionDescriptor>> {
        query.clamp_page_size();

        let old_kind = query.direction.map(|d| match d {
            RtpDirection::Receive => RtpSessionKind::Receiver,
            RtpDirection::Send => RtpSessionKind::Sender,
            RtpDirection::DuplexTalk | _ => RtpSessionKind::Talk,
        });
        let old_state = query.state.map(|s| match s {
            RtpSessionState::Allocating => OldRtpSessionState::Created,
            RtpSessionState::Ready => OldRtpSessionState::Listening,
            RtpSessionState::Active => OldRtpSessionState::Connected,
            RtpSessionState::Draining => OldRtpSessionState::Stopping,
            RtpSessionState::Stopped => OldRtpSessionState::Stopped,
            RtpSessionState::Failed | _ => OldRtpSessionState::Failed,
        });
        let old_query = RtpQuery {
            kind: old_kind,
            state: old_state,
            session_id: query.session_id.clone(),
            media_key: query.media_key.clone(),
            page: query.page,
            page_size: query.page_size,
        };

        let page = self.orchestrator.list_rtp_sessions(old_query)?;
        let descs = self.rtp_descriptors.lock();
        let items: Vec<RtpSessionDescriptor> = page
            .items
            .into_iter()
            .map(|session| {
                let stored = descs.get(&session.session_id).cloned();
                self.build_descriptor(ctx, &session, stored)
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Page {
            items,
            page: page.page,
            page_size: page.page_size,
            total: page.total,
            next_cursor: page.next_cursor,
        })
    }
}
