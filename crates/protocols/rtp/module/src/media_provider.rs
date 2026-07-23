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
use cheetah_sdk::media_api::error::{EffectOutcome, MediaError, MediaErrorCode, Result};
use cheetah_sdk::media_api::fencing::{ControlledResourceRef, ResourceOrigin};
use cheetah_sdk::media_api::ids::{
    MediaBindingId, MediaKey, MediaNodeInstanceEpoch, MediaSessionId, OwnerEpoch,
    ResourceGeneration, RtpSessionId, StreamKeyBridge, TenantId,
};
use cheetah_sdk::media_api::model::{
    AdmissionAction, AdmissionRequest, Decision, Page, RtpSession, RtpSessionKind,
    RtpSessionState as OldRtpSessionState, RtpTcpMode,
};
use cheetah_sdk::media_api::port::{MediaRequestContext, RtpApi};
use cheetah_sdk::media_api::rtp_session::{
    GbMediaCompatibilityProfile, MediaContainer, OpenRtpReceiver, OpenRtpSender, OpenRtpTalk,
    RtpDirection, RtpEndpoints, RtpFraming, RtpPayloadBinding, RtpSessionApi, RtpSessionDescriptor,
    RtpSessionGeneration, RtpSessionParams, RtpSessionQuery, RtpSessionRef, RtpSessionState,
    RtpTransport, StopRtpSession, TcpRole, UpdateRtpSession,
};
use cheetah_sdk::{
    CancellationToken, Deadline, EngineContext, IdempotencyError, IdempotencyKey, StreamKey,
};
use parking_lot::Mutex;
use serde::Serialize;

use crate::config::RtpModuleConfig;
use crate::egress::{run_egress_session, ActiveEgressMap, EgressCleanup};
use crate::orchestrator::RtpSessionOrchestrator;
use crate::rollback::RollbackGuard;

/// Media-domain `RtpApi` and `RtpSessionApi` provider.
///
/// `RtpApi` / `RtpSessionApi` provider。
#[derive(Clone)]
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

    /// Ask the configured admission provider whether the operation is allowed.
    /// A missing provider is treated as allow-all so optional admission remains optional.
    async fn admit(
        &self,
        ctx: &MediaRequestContext,
        action: AdmissionAction,
        resource: &MediaKey,
        protocol: &str,
        source_address: Option<String>,
    ) -> Result<()> {
        let Some(provider) = self.engine.media_services.admission() else {
            return Ok(());
        };
        let request = AdmissionRequest {
            action,
            principal: ctx.principal.clone(),
            resource: resource.clone(),
            protocol: protocol.to_string(),
            source_address,
            params: HashMap::new(),
        };
        match provider.authorize(ctx, request).await {
            Ok(Decision::Allow) => Ok(()),
            Ok(Decision::Deny { code, reason }) => Err(MediaError::new(code, reason)),
            Err(e) => Err(e),
        }
    }

    /// Enforce per-module capability/profile and session limits before allocating a session.
    fn check_profile_and_limits(&self, params: &RtpSessionParams) -> Result<()> {
        if !self.config.enabled_profiles.contains(&params.profile) {
            return Err(MediaError::unsupported(format!(
                "profile {:?} is not enabled",
                params.profile
            )));
        }
        if self.orchestrator.session_count() >= self.config.max_sessions {
            return Err(MediaError::unavailable("rtp session limit reached")
                .with_outcome(EffectOutcome::NotApplied));
        }
        Ok(())
    }

    /// Normalize a codec name for case-insensitive comparison.
    fn normalize_codec_name(codec: &str) -> String {
        codec.trim().to_lowercase()
    }

    /// Enforce talk codec capability: PCMA/PCMU are preferred; AAC is only allowed when
    /// explicitly enabled in the RTP module configuration.
    fn check_talk_codec(&self, request: &OpenRtpTalk) -> Result<()> {
        if self.config.enabled_talk_codecs.is_empty() {
            return Err(MediaError::unsupported("no talk codecs are enabled"));
        }
        let enabled: std::collections::HashSet<String> = self
            .config
            .enabled_talk_codecs
            .iter()
            .map(|c| Self::normalize_codec_name(c))
            .collect();
        let check = |binding: &RtpPayloadBinding| {
            let codec = Self::normalize_codec_name(&binding.codec);
            if !enabled.contains(&codec) {
                return Err(MediaError::unsupported(format!(
                    "talk codec {} is not enabled",
                    binding.codec
                )));
            }
            Ok(())
        };
        for binding in &request.params.payload_bindings {
            check(binding)?;
        }
        if let Some(binding) = &request.talkback_binding {
            check(binding)?;
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
            source_binding_policy: params.source_binding_policy,
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
        // Voice talkback with raw G.711 belongs to the raw-audio packetizer, not the generic
        // ES packetizer, regardless of whether the container is explicitly ElementaryStream
        // or left as AutoDetect.
        let codec_hint = if matches!(
            params.container,
            MediaContainer::ElementaryStream | MediaContainer::AutoDetect
        ) && params.payload_bindings.first().is_some_and(|b| {
            matches!(
                b.codec.to_lowercase().as_str(),
                "pcma" | "pcmu" | "g711a" | "g711u"
            )
        }) {
            Some("raw_audio".to_string())
        } else {
            codec_hint
        };
        Ok(RtpSenderRequest {
            media_key: params.media_key,
            destination_endpoint,
            ssrc: params.ssrc,
            payload_type,
            codec_hint,
            mode,
            transport_options,
            source_binding_policy: params.source_binding_policy,
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
        self.open_rtp_sender_with_cancel(ctx, request, None)
            .await
            .map(|(s, _)| s)
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

impl RtpMediaProvider {
    fn idempotency_key(
        &self,
        ctx: &MediaRequestContext,
        operation: &str,
    ) -> Option<IdempotencyKey> {
        let key = ctx.idempotency_key.as_ref()?;
        let principal = ctx
            .principal
            .as_ref()
            .map(|p| p.identity.clone())
            .unwrap_or_default();
        Some(IdempotencyKey::new(principal, operation, key.clone()))
    }

    fn request_fingerprint<T: Serialize>(
        request: &T,
    ) -> Result<cheetah_sdk::IdempotencyFingerprint> {
        let bytes = serde_json::to_vec(request).map_err(|e| {
            MediaError::invalid_argument(format!(
                "failed to serialize idempotency fingerprint: {e}"
            ))
        })?;
        Ok(cheetah_sdk::canonical_hash(&bytes))
    }

    fn idempotency_to_media_error(err: IdempotencyError) -> MediaError {
        match err {
            IdempotencyError::Conflict { .. } => MediaError::conflict(err.to_string()),
            IdempotencyError::InProgress => MediaError::new(
                MediaErrorCode::Busy,
                "idempotency operation in progress".to_string(),
            ),
            IdempotencyError::OperationFailed(msg) | IdempotencyError::Retryable(msg) => {
                serde_json::from_str::<MediaError>(&msg)
                    .unwrap_or_else(|_| MediaError::new(MediaErrorCode::Internal, msg))
            }
        }
    }

    /// Run `f` through the shared idempotency repository when the caller supplied a key.
    /// No key means the operation is executed exactly once without caching.
    /// Deterministic errors are stored and replayed preserving their original `MediaError` code;
    /// retryable errors (Busy, Timeout, Unavailable, or explicitly marked retryable) are not cached.
    async fn idempotent_open<F, Fut>(
        &self,
        ctx: &MediaRequestContext,
        operation: &str,
        fingerprint: cheetah_sdk::IdempotencyFingerprint,
        f: F,
    ) -> Result<RtpSessionDescriptor>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<RtpSessionDescriptor>> + Send,
    {
        let Some(key) = self.idempotency_key(ctx, operation) else {
            return f().await;
        };
        self.engine
            .media_services
            .idempotency()
            .execute(key, fingerprint, 60_000, || async {
                f().await
                    .map_err(|e| {
                        let encoded = serde_json::to_string(&e)
                            .unwrap_or_else(|_| format!("internal: {}", e.message));
                        if e.retryable
                            || matches!(
                                e.code,
                                MediaErrorCode::Busy
                                    | MediaErrorCode::Timeout
                                    | MediaErrorCode::Unavailable
                                    | MediaErrorCode::PermissionDenied
                                    | MediaErrorCode::Unauthenticated
                                    | MediaErrorCode::RateLimited
                            )
                        {
                            IdempotencyError::Retryable(encoded)
                        } else {
                            IdempotencyError::OperationFailed(encoded)
                        }
                    })
                    .map(|d| {
                        let sid = d.session_id.0.clone();
                        (d, Some(sid))
                    })
            })
            .await
            .map_err(Self::idempotency_to_media_error)
    }
}

#[async_trait]
impl RtpSessionApi for RtpMediaProvider {
    async fn open_receiver(
        &self,
        ctx: &MediaRequestContext,
        request: OpenRtpReceiver,
    ) -> Result<RtpSessionDescriptor> {
        let fingerprint = Self::request_fingerprint(&request)?;
        self.idempotent_open(ctx, "open_rtp_receiver", fingerprint, || {
            let provider = self.clone();
            let ctx = ctx.clone();
            let request = request.clone();
            async move { provider.open_receiver_impl(&ctx, request).await }
        })
        .await
    }

    async fn open_sender(
        &self,
        ctx: &MediaRequestContext,
        request: OpenRtpSender,
    ) -> Result<RtpSessionDescriptor> {
        let fingerprint = Self::request_fingerprint(&request)?;
        self.idempotent_open(ctx, "open_rtp_sender", fingerprint, || {
            let provider = self.clone();
            let ctx = ctx.clone();
            let request = request.clone();
            async move { provider.open_sender_impl(&ctx, request).await }
        })
        .await
    }

    async fn open_talk(
        &self,
        ctx: &MediaRequestContext,
        request: OpenRtpTalk,
    ) -> Result<RtpSessionDescriptor> {
        let fingerprint = Self::request_fingerprint(&request)?;
        self.idempotent_open(ctx, "open_rtp_talk", fingerprint, || {
            let provider = self.clone();
            let ctx = ctx.clone();
            let request = request.clone();
            async move { provider.open_talk_impl(&ctx, request).await }
        })
        .await
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
            source_policy: request.source_binding_policy,
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

impl RtpMediaProvider {
    /// Extract the audio packet duration from the first payload binding, if any.
    fn packet_duration_ms_from_params(params: &RtpSessionParams) -> Option<u32> {
        params
            .payload_bindings
            .first()
            .and_then(|b| b.packet_duration_ms)
    }

    async fn open_receiver_impl(
        &self,
        ctx: &MediaRequestContext,
        request: OpenRtpReceiver,
    ) -> Result<RtpSessionDescriptor> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        self.admit(
            ctx,
            AdmissionAction::OpenRtpReceiver,
            &request.params.media_key,
            "rtp",
            request.params.remote_endpoint.map(|a| a.to_string()),
        )
        .await?;
        self.check_profile_and_limits(&request.params)?;
        let old_req = self.build_old_receiver_request(request.clone())?;
        let session = self.open_rtp_receiver(ctx, old_req).await?;
        let guard = RollbackGuard::new(
            self.orchestrator.clone(),
            self.engine.runtime_api.clone(),
            session.session_id.clone(),
        );
        let mut descriptor = self.build_descriptor(ctx, &session, None)?;
        self.apply_request_overrides(&mut descriptor, &request.params);
        self.rtp_descriptors
            .lock()
            .insert(descriptor.session_id.clone(), descriptor.clone());
        guard.commit();
        Ok(descriptor)
    }

    /// Create a driver-side sender/talk session and start the egress worker,
    /// returning both the session and the cancellation token that controls the
    /// background task. The caller is responsible for cancelling the token on
    /// rollback; if the open succeeds, the token is owned by `run_egress_session`.
    async fn open_rtp_sender_with_cancel(
        &self,
        ctx: &MediaRequestContext,
        request: RtpSenderRequest,
        packet_duration_ms: Option<u32>,
    ) -> Result<(RtpSession, CancellationToken)> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        // Create the driver-side sender session first.
        let session = self
            .orchestrator
            .open_rtp_sender_with_duration(request.clone(), packet_duration_ms)
            .await?;
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
        let cancel_for_task = cancel.clone();
        runtime_api.spawn(Box::pin(async move {
            run_egress_session(
                engine,
                driver,
                vec![session_key],
                stream_key,
                cancel_for_task,
                Some(orchestrator),
                Some(cleanup),
            )
            .await;
        }));

        Ok((session, cancel))
    }

    async fn open_sender_impl(
        &self,
        ctx: &MediaRequestContext,
        request: OpenRtpSender,
    ) -> Result<RtpSessionDescriptor> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        self.admit(
            ctx,
            AdmissionAction::OpenRtpSender,
            &request.params.media_key,
            "rtp",
            request.params.remote_endpoint.map(|a| a.to_string()),
        )
        .await?;
        self.check_profile_and_limits(&request.params)?;
        let mode = match request.params.tcp_role {
            Some(TcpRole::Passive) => RtpSenderMode::Passive,
            _ => RtpSenderMode::Active,
        };
        let old_req = self.build_old_sender_request(request.clone(), mode)?;
        let packet_duration_ms = Self::packet_duration_ms_from_params(&request.params);
        let (session, cancel) = self
            .open_rtp_sender_with_cancel(ctx, old_req, packet_duration_ms)
            .await?;
        let guard = RollbackGuard::new(
            self.orchestrator.clone(),
            self.engine.runtime_api.clone(),
            session.session_id.clone(),
        )
        .with_egress_cancel(self.active_senders.clone(), cancel);
        let mut descriptor = self.build_descriptor(ctx, &session, None)?;
        self.apply_request_overrides(&mut descriptor, &request.params);
        self.rtp_descriptors
            .lock()
            .insert(descriptor.session_id.clone(), descriptor.clone());
        guard.commit();
        Ok(descriptor)
    }
    async fn open_talk_impl(
        &self,
        ctx: &MediaRequestContext,
        request: OpenRtpTalk,
    ) -> Result<RtpSessionDescriptor> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        self.admit(
            ctx,
            AdmissionAction::OpenRtpSender,
            &request.params.media_key,
            "rtp",
            request.params.remote_endpoint.map(|a| a.to_string()),
        )
        .await?;
        self.check_profile_and_limits(&request.params)?;
        self.check_talk_codec(&request)?;
        let sender_req = self.build_old_sender_request(
            OpenRtpSender {
                params: request.params.clone(),
            },
            RtpSenderMode::Talk,
        )?;
        let packet_duration_ms =
            Self::packet_duration_ms_from_params(&request.params).or_else(|| {
                request
                    .talkback_binding
                    .as_ref()
                    .and_then(|b| b.packet_duration_ms)
            });
        let (session, cancel) = self
            .open_rtp_sender_with_cancel(ctx, sender_req, packet_duration_ms)
            .await?;
        let guard = RollbackGuard::new(
            self.orchestrator.clone(),
            self.engine.runtime_api.clone(),
            session.session_id.clone(),
        )
        .with_egress_cancel(self.active_senders.clone(), cancel);
        let mut descriptor = self.build_descriptor(ctx, &session, None)?;
        self.apply_request_overrides(&mut descriptor, &request.params);
        if let Some(binding) = request.talkback_binding {
            descriptor.payload_bindings.push(binding);
        }
        self.rtp_descriptors
            .lock()
            .insert(descriptor.session_id.clone(), descriptor.clone());
        guard.commit();
        Ok(descriptor)
    }
}
