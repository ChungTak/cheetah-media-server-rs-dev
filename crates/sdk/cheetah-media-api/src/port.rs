use async_trait::async_trait;

use crate::auth::{AuthCredentials, Principal};
use crate::command::*;
use crate::error::{MediaError, Result};
use crate::event::{MediaEvent, MediaEventSender, MediaEventSubscription};
use crate::ids::*;
use crate::model::*;

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
        }
    }
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

    async fn query_snapshots(
        &self,
        ctx: &MediaRequestContext,
        query: SnapshotQuery,
    ) -> Result<Page<SnapshotInfo>>;

    async fn delete_snapshot_directory(
        &self,
        ctx: &MediaRequestContext,
        request: DeleteSnapshotRequest,
    ) -> Result<()>;
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

    async fn create_ffmpeg_proxy(
        &self,
        ctx: &MediaRequestContext,
        request: FfmpegProxyRequest,
    ) -> Result<ProxyInfo>;

    async fn delete_ffmpeg_proxy(&self, _ctx: &MediaRequestContext, _id: &ProxyId) -> Result<()> {
        Err(MediaError::unsupported_capability("delete_ffmpeg_proxy"))
    }

    async fn get_ffmpeg_proxy(
        &self,
        _ctx: &MediaRequestContext,
        _id: &ProxyId,
    ) -> Result<ProxyInfo> {
        Err(MediaError::unsupported_capability("get_ffmpeg_proxy"))
    }

    async fn list_ffmpeg_proxies(
        &self,
        _ctx: &MediaRequestContext,
        _query: ProxyQuery,
    ) -> Result<Page<ProxyInfo>> {
        Err(MediaError::unsupported_capability("list_ffmpeg_proxies"))
    }
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
    MediaControlApi + PublishSubscribeApi + RecordApi + SnapshotApi + ProxyApi + RtpApi + Send + Sync
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
