use async_trait::async_trait;

use crate::auth::{AuthCredentials, Principal};
use crate::capacity::{CapacityLimits, CapacityPermit, CapacityRequest, CapacitySnapshot};
use crate::command::*;
use crate::credential::CredentialLease;
use crate::error::{MediaError, Result};
use crate::event::{MediaEvent, MediaEventSender, MediaEventSubscription};
use crate::fencing::ControlledResourceRef;
use crate::ids::*;
use crate::model::{AdmissionRequest, Decision, *};
use crate::outbound_policy::{ResolvedEndpoint, UrlPolicyVerdict};
use crate::processing::{
    CreateProcessingJob, ProcessingJob, ProcessingJobQuery, ProcessingPreflightReport,
    UpdateProcessingJob,
};
use crate::webhook::{
    CreateWebhookProfileRequest, UpdateWebhookProfileRequest, WebhookProfile, WebhookProfileId,
    WebhookTestReport,
};

/// Request context passed to media API operations.
///
/// 媒体 API 操作传入的请求上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRequestContext {
    pub request_id: RequestId,
    pub correlation_id: Option<String>,
    pub principal: Option<Principal>,
    pub source_adapter: String,
    pub trace_context: Option<String>,
    pub deadline: Option<i64>,
    /// Idempotency key supplied by the client for create operations.
    /// Only used by routes that start tasks, sessions, proxies, etc.
    pub idempotency_key: Option<String>,
    /// Cluster signaling context for control-plane mutations and reads.
    ///
    /// Local adapters do not populate this; they use `source_adapter` and the
    /// principal to identify ownership.
    pub mutation: Option<MediaMutationContext>,
}

impl Default for MediaRequestContext {
    fn default() -> Self {
        Self {
            request_id: RequestId("".to_string()),
            correlation_id: None,
            principal: None,
            source_adapter: "unknown".to_string(),
            trace_context: None,
            deadline: None,
            idempotency_key: None,
            mutation: None,
        }
    }
}

/// Cluster control-plane context carried inside a [`MediaRequestContext`].
///
/// Required for all cluster-side mutations and reads: it identifies the tenant,
/// signaling source node, target node/instance, operation, and contract version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaMutationContext {
    pub tenant_id: TenantId,
    pub message_id: MessageId,
    pub source_signaling_node_id: MediaNodeId,
    pub owner_epoch: OwnerEpoch,
    pub target_media_node_id: MediaNodeId,
    pub target_media_node_instance_epoch: MediaNodeInstanceEpoch,
    pub operation_id: OperationId,
    pub operation_step_id: OperationStepId,
    pub media_session_id: Option<MediaSessionId>,
    pub media_binding_id: Option<MediaBindingId>,
    pub contract_version: String,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

/// Framework-neutral authentication and authorization API for control-plane
/// requests.
///
/// 控制面请求的框架无关认证与授权 API。
pub trait ControlAuthApi: Send + Sync {
    /// Authenticate the request credentials and return a principal with scopes.
    ///
    /// Returning `Ok` with an anonymous principal is allowed; callers must still
    /// enforce scope checks before performing high-risk operations.
    fn authenticate(&self, credentials: &AuthCredentials) -> Result<Principal>;
}

/// Core media control and query operations.
///
/// 核心媒体控制与查询操作。
#[async_trait]
pub trait MediaControlApi: Send + Sync {
    async fn get_media_list(
        &self,
        ctx: &MediaRequestContext,
        query: MediaQuery,
    ) -> Result<Page<StreamInfo>>;

    async fn get_media(&self, ctx: &MediaRequestContext, key: &MediaKey) -> Result<StreamInfo>;

    async fn is_media_online(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
    ) -> Result<OnlineState>;

    async fn list_sessions(
        &self,
        ctx: &MediaRequestContext,
        query: SessionQuery,
    ) -> Result<Page<SessionInfo>>;

    async fn kick_session(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
        reason: CloseReason,
    ) -> Result<()>;

    async fn kick_stream(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
        reason: CloseReason,
    ) -> Result<CloseReport>;

    async fn request_keyframe(&self, ctx: &MediaRequestContext, key: &MediaKey) -> Result<()>;
}

/// Publish and subscribe operations.
///
/// 发布与订阅操作。
#[async_trait]
pub trait PublishSubscribeApi: Send + Sync {
    async fn acquire_publisher(
        &self,
        ctx: &MediaRequestContext,
        request: PublishRequest,
    ) -> Result<PublisherHandle>;

    async fn open_subscriber(
        &self,
        ctx: &MediaRequestContext,
        request: SubscribeRequest,
    ) -> Result<SubscriberHandle>;

    async fn close_handle(
        &self,
        ctx: &MediaRequestContext,
        id: &SessionId,
        reason: CloseReason,
    ) -> Result<()>;
}

/// Record operations.
///
/// 录制操作。
#[async_trait]
pub trait RecordApi: Send + Sync {
    async fn start_record(
        &self,
        ctx: &MediaRequestContext,
        request: StartRecordRequest,
    ) -> Result<RecordTask>;

    async fn stop_record(
        &self,
        ctx: &MediaRequestContext,
        request: StopRecordRequest,
    ) -> Result<RecordTask>;

    async fn query_record_tasks(
        &self,
        ctx: &MediaRequestContext,
        query: RecordTaskQuery,
    ) -> Result<Page<RecordTask>>;

    async fn query_record_files(
        &self,
        ctx: &MediaRequestContext,
        query: RecordFileQuery,
    ) -> Result<Page<RecordFile>>;

    async fn delete_record_file(
        &self,
        ctx: &MediaRequestContext,
        request: DeleteRecordRequest,
    ) -> Result<()>;

    async fn control_record_playback(
        &self,
        ctx: &MediaRequestContext,
        file_id: &RecordFileId,
        command: RecordPlaybackCommand,
    ) -> Result<()>;
}

/// Snapshot operations.
///
/// 快照操作。
#[async_trait]
pub trait SnapshotApi: Send + Sync {
    async fn take_snapshot(
        &self,
        ctx: &MediaRequestContext,
        request: SnapshotRequest,
    ) -> Result<SnapshotHandle>;

    /// Fetch a snapshot from an external URL, applying URL policy and content
    /// validation before committing it to the configured FileStore namespace.
    async fn fetch_snapshot(
        &self,
        _ctx: &MediaRequestContext,
        _request: FetchSnapshotRequest,
    ) -> Result<SnapshotHandle> {
        Err(MediaError::unsupported(
            "snapshot fetch is not supported by this provider",
        ))
    }

    async fn query_snapshots(
        &self,
        ctx: &MediaRequestContext,
        query: SnapshotQuery,
    ) -> Result<Page<SnapshotInfo>>;

    /// Delete snapshots matching the request and return a per-handle batch result.
    ///
    /// 删除匹配请求的快照并返回逐项批量结果。
    async fn delete_snapshots(
        &self,
        ctx: &MediaRequestContext,
        request: DeleteSnapshotRequest,
    ) -> Result<crate::media_file_store::DeleteBatchResult>;

    /// Deprecated alias for `delete_snapshots`. Delegates to the new method and
    /// returns an error if any deletion failed.
    ///
    /// `delete_snapshots` 的旧别名，委托给新方法并在有失败项时返回错误。
    async fn delete_snapshot_directory(
        &self,
        ctx: &MediaRequestContext,
        request: DeleteSnapshotRequest,
    ) -> Result<()> {
        let result = self.delete_snapshots(ctx, request).await?;
        if result.failed > 0 {
            return Err(crate::error::MediaError::new(
                crate::error::MediaErrorCode::Internal,
                format!("{} snapshot deletion(s) failed", result.failed),
            ));
        }
        Ok(())
    }
}

/// Playback operations.
///
/// 回放操作。
#[async_trait]
pub trait PlaybackApi: Send + Sync {
    async fn open_playback(
        &self,
        ctx: &MediaRequestContext,
        request: OpenPlaybackRequest,
    ) -> Result<PlaybackSession>;

    async fn get_playback(
        &self,
        ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
    ) -> Result<PlaybackSession>;

    async fn list_playbacks(
        &self,
        ctx: &MediaRequestContext,
        query: PlaybackQuery,
    ) -> Result<Page<PlaybackSession>>;

    async fn control_playback(
        &self,
        ctx: &MediaRequestContext,
        id: &PlaybackSessionId,
        command: PlaybackControl,
    ) -> Result<PlaybackSession>;

    async fn stop_playback(&self, ctx: &MediaRequestContext, id: &PlaybackSessionId) -> Result<()>;
}

/// Proxy operations.
///
/// 代理操作。
#[async_trait]
pub trait ProxyApi: Send + Sync {
    async fn create_pull_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: PullProxyRequest,
    ) -> Result<ProxyInfo>;

    async fn delete_pull_proxy(&self, ctx: &MediaRequestContext, id: &ProxyId) -> Result<()>;

    async fn list_pull_proxies(
        &self,
        ctx: &MediaRequestContext,
        query: ProxyQuery,
    ) -> Result<Page<ProxyInfo>>;

    async fn get_pull_proxy(&self, _ctx: &MediaRequestContext, _id: &ProxyId) -> Result<ProxyInfo> {
        Err(MediaError::unsupported_capability("get_pull_proxy"))
    }

    async fn list_push_proxies(
        &self,
        _ctx: &MediaRequestContext,
        _query: ProxyQuery,
    ) -> Result<Page<ProxyInfo>> {
        Err(MediaError::unsupported_capability("list_push_proxies"))
    }

    async fn get_push_proxy(&self, _ctx: &MediaRequestContext, _id: &ProxyId) -> Result<ProxyInfo> {
        Err(MediaError::unsupported_capability("get_push_proxy"))
    }

    async fn create_push_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: PushProxyRequest,
    ) -> Result<ProxyInfo>;

    async fn delete_push_proxy(&self, ctx: &MediaRequestContext, id: &ProxyId) -> Result<()>;
}

/// Media processing job operations.
///
/// 媒体处理任务操作。
#[async_trait]
pub trait MediaProcessingApi: Send + Sync {
    async fn preflight(&self, ctx: &MediaRequestContext) -> Result<ProcessingPreflightReport>;

    async fn create_job(
        &self,
        ctx: &MediaRequestContext,
        request: CreateProcessingJob,
    ) -> Result<ProcessingJob>;

    async fn get_job(
        &self,
        ctx: &MediaRequestContext,
        id: &ProcessingJobId,
    ) -> Result<ProcessingJob>;

    async fn list_jobs(
        &self,
        ctx: &MediaRequestContext,
        query: ProcessingJobQuery,
    ) -> Result<Page<ProcessingJob>>;

    async fn update_job(
        &self,
        ctx: &MediaRequestContext,
        request: UpdateProcessingJob,
    ) -> Result<ProcessingJob>;

    async fn stop_job(
        &self,
        ctx: &MediaRequestContext,
        id: &ProcessingJobId,
    ) -> Result<ProcessingJob>;

    async fn delete_job(&self, ctx: &MediaRequestContext, id: &ProcessingJobId) -> Result<()>;
}

/// RTP operations.
///
/// RTP 操作。
#[async_trait]
pub trait RtpApi: Send + Sync {
    async fn open_rtp_receiver(
        &self,
        ctx: &MediaRequestContext,
        request: RtpReceiverRequest,
    ) -> Result<RtpSession>;

    async fn connect_rtp_receiver(
        &self,
        ctx: &MediaRequestContext,
        request: RtpConnectRequest,
    ) -> Result<RtpSession>;

    async fn open_rtp_sender(
        &self,
        ctx: &MediaRequestContext,
        request: RtpSenderRequest,
    ) -> Result<RtpSession>;

    async fn stop_rtp_session(&self, ctx: &MediaRequestContext, id: &RtpSessionId) -> Result<()>;

    async fn list_rtp_sessions(
        &self,
        ctx: &MediaRequestContext,
        query: RtpQuery,
    ) -> Result<Page<RtpSession>>;

    async fn update_rtp_session(
        &self,
        ctx: &MediaRequestContext,
        request: UpdateRtpRequest,
    ) -> Result<RtpSession>;

    async fn get_rtp_session(
        &self,
        ctx: &MediaRequestContext,
        id: &RtpSessionId,
    ) -> Result<RtpSession>;
}

/// Combined facade over all media capabilities.
///
/// 所有媒体能力的组合 facade。
///
/// Provider implementations may split the trait into sub-traits and combine
/// them behind this facade. Unimplemented methods return `Unsupported`.
#[async_trait]
pub trait MediaFacade:
    MediaControlApi
    + PublishSubscribeApi
    + RecordApi
    + SnapshotApi
    + PlaybackApi
    + ProxyApi
    + RtpApi
    + Send
    + Sync
{
    /// Return the capability set currently supported by the facade.
    ///
    /// 返回 facade 当前支持的能力集。
    fn capabilities(&self) -> crate::capability::MediaCapabilitySet;

    /// Subscribe to internal media events.
    ///
    /// Returns a subscription handle; dropping it cancels the subscription.
    /// 订阅内部媒体事件，返回可取消的订阅句柄。
    fn subscribe_events(
        &self,
        sender: Box<dyn MediaEventSender>,
    ) -> Result<Box<dyn MediaEventSubscription>>;
}

/// Resolve playable output URLs for a media resource.
///
/// Implementations must use configured public host/port/TLS settings and must
/// not trust unauthenticated request `Host` headers.
///
/// 为媒体资源解析可播放输出 URL。
///
/// 实现必须使用配置的 public host/端口/TLS，不得信任未认证请求的 `Host` 头。
#[async_trait]
pub trait MediaUrlResolverApi: Send + Sync {
    /// Resolve URLs for the given media key and requested schemas.
    ///
    /// When `schemas` is empty, return all schemas the resolver currently
    /// supports.
    ///
    /// 为给定媒体键和请求的 schema 解析 URL；`schemas` 为空时返回当前支持的全部 schema。
    async fn resolve_urls(
        &self,
        ctx: &MediaRequestContext,
        key: &MediaKey,
        schemas: &[MediaSchema],
    ) -> Result<Vec<MediaUrl>>;
}

/// Registry of active public output endpoints.
///
/// Protocol modules register their public listener endpoints after `start`
/// succeeds and unregister them in `stop` (or when the module is dropped).
/// URL resolvers consume the snapshot to build accurate, runtime-driven URLs.
///
/// 活跃公网输出端点注册表。
///
/// 协议 module 在 `start` 成功后注册其公网监听端点，在 `stop`（或 module 被 drop）时注销。
/// URL resolver 消费 snapshot 来构建基于运行时的准确 URL。
#[async_trait]
pub trait MediaOutputRegistryApi: Send + Sync {
    /// Register a new endpoint and return a unique registration id.
    async fn register_endpoint(
        &self,
        endpoint: crate::output::MediaOutputEndpoint,
    ) -> Result<String>;

    /// Unregister the endpoint with the given id. Returns `NotFound` when the
    /// id does not match a live registration.
    async fn unregister_endpoint(&self, registration_id: &str) -> Result<()>;

    /// Return a snapshot of all currently registered endpoints.
    async fn snapshot(&self) -> Result<Vec<crate::output::MediaOutputEndpoint>>;
}

/// Synchronous webhook decision hooks.
///
/// 同步 webhook 决策钩子。
#[async_trait]
pub trait WebhookApi: Send + Sync {
    /// Ask configured webhook targets whether an action should be allowed.
    ///
    /// The event is translated to a ZLM-compatible hook name and payload,
    /// sent with a short deadline, and the response is parsed into an
    /// `Allow`/`Deny` decision. Targets that time out or fail apply their
    /// configured failure policy.
    ///
    /// 向配置的 webhook 目标询问是否允许某个动作。事件会被翻译成兼容 ZLM 的 hook
    /// 名称与负载，短 deadline 发送，响应被解析成 `Allow`/`Deny` 决策；超时或失败
    /// 时应用对应失败策略。
    async fn request_decision(&self, event: MediaEvent) -> Result<Decision>;
}

/// Synchronous admission decision before a side-effecting media operation.
///
/// 副作用媒体操作前的同步准入决策。
#[async_trait]
pub trait MediaAdmissionApi: Send + Sync {
    /// Ask configured admission targets whether the requested action should be
    /// allowed. A missing provider, timeout, or failure returns a `Deny`
    /// decision with an appropriate stable `MediaErrorCode`.
    ///
    /// 向配置的准入目标询问是否允许请求动作。provider 缺失、超时或失败时返回
    /// 带有稳定 `MediaErrorCode` 的 `Deny`。
    async fn authorize(
        &self,
        ctx: &MediaRequestContext,
        request: AdmissionRequest,
    ) -> Result<Decision>;
}

/// Administrative management of webhook profiles.
///
/// 外部投递配置的管理入口。
#[async_trait]
pub trait WebhookAdminApi: Send + Sync {
    /// Create a new profile. Returns the stored profile with its generation.
    async fn create_profile(
        &self,
        ctx: &MediaRequestContext,
        request: CreateWebhookProfileRequest,
    ) -> Result<WebhookProfile>;

    /// Return the profile with the given id, including its current secret.
    async fn get_profile(
        &self,
        ctx: &MediaRequestContext,
        id: &WebhookProfileId,
    ) -> Result<WebhookProfile>;

    /// List all stored profiles.
    async fn list_profiles(&self, ctx: &MediaRequestContext) -> Result<Vec<WebhookProfile>>;

    /// Update an existing profile using expected-generation concurrency control.
    async fn update_profile(
        &self,
        ctx: &MediaRequestContext,
        request: UpdateWebhookProfileRequest,
    ) -> Result<WebhookProfile>;

    /// Delete a profile by id.
    async fn delete_profile(&self, ctx: &MediaRequestContext, id: &WebhookProfileId) -> Result<()>;

    /// Send a synthetic `WebhookTest` envelope to the profile target and return
    /// a security summary with DNS, connect, HTTP, signature and latency.
    async fn test_profile(
        &self,
        ctx: &MediaRequestContext,
        id: &WebhookProfileId,
    ) -> Result<WebhookTestReport>;
}

/// Runtime-neutral credential exchange used by proxy/snapshot modules to obtain
/// short-lived secrets without embedding provider details.
///
/// Proxy/Snapshot module 通过此接口获取短期凭据，无需嵌入 provider 细节。
#[async_trait]
pub trait CredentialExchangeApi: Send + Sync {
    /// Exchange a `CredentialHandle` for a short-lived, purpose-bound lease.
    ///
    /// The returned `CredentialLease` is tied to the tenant, `resource_ref`,
    /// `operation_step_id` and `purpose`; it must not be cached beyond its TTL,
    /// reused across resources, or persisted.
    async fn exchange(
        &self,
        ctx: &MediaRequestContext,
        handle: &CredentialHandle,
        purpose: &str,
        resource_ref: &ControlledResourceRef,
    ) -> Result<CredentialLease>;
}

/// Runtime-neutral capacity and load-gate API.
///
/// `acquire` returns a `Box<dyn CapacityPermit>` that releases its reservation
/// when dropped. Implementations must not over-commit resources.
///
/// 运行时无关的容量与负载门控 API。
#[async_trait]
pub trait MediaCapacityApi: Send + Sync {
    /// Acquire capacity for a new resource operation.
    async fn acquire(&self, request: CapacityRequest) -> Result<Box<dyn CapacityPermit>>;

    /// Return a point-in-time snapshot of usage and remaining capacity.
    async fn snapshot(&self) -> Result<CapacitySnapshot>;

    /// Update the hard limits for each resource dimension.
    async fn update_limits(&self, limits: CapacityLimits) -> Result<()>;

    /// Open or close the node gate that controls whether new resources may be
    /// created on this node instance.
    async fn set_node_gate(&self, open: bool) -> Result<()>;
}

/// Runtime-neutral outbound URL policy used by snapshot fetch and proxy pull.
///
/// 运行时无关的出站 URL 策略，供快照抓取与代理拉流使用。
#[async_trait]
pub trait OutboundUrlPolicyApi: Send + Sync {
    /// Validate a URL statically against configured scheme/length rules.
    fn check_static(&self, url: &str) -> Result<UrlPolicyVerdict>;

    /// Resolve, sanitize and validate an outbound URL, returning a pinned
    /// endpoint if allowed.
    async fn evaluate(&self, url: &str) -> Result<ResolvedEndpoint>;

    /// Re-evaluate a redirect target using the same policy and remaining budget.
    async fn validate_redirect(
        &self,
        previous: &ResolvedEndpoint,
        location: &str,
        redirects_remaining: u32,
    ) -> Result<ResolvedEndpoint>;
}
