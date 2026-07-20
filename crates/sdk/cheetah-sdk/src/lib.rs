//! `cheetah-sdk` defines the module contract and runtime-neutral APIs used by
//! feature modules and the engine.
//!
//! Core concepts:
//! - `Module` / `ModuleFactory`: module lifecycle and registration.
//! - `EngineContext`: injected capability set (runtime, streams, tasks, config, services).
//! - `StreamKey`, `PublisherApi`, `SubscriberApi`, `StreamManagerApi`: stream addressing and publishing/subscribing.
//! - `EventBus`: publish/subscribe system events across modules.
//! - `Config*` traits: schema, validation, application, and rollback of runtime config.
//!
//! `cheetah-sdk` 定义了特性模块和引擎使用的模块契约与运行时无关 API。
//!
//! 核心概念：
//! - `Module` / `ModuleFactory`：模块生命周期与注册。
//! - `EngineContext`：注入的能力集（运行时、流、任务、配置、服务）。
//! - `StreamKey`、`PublisherApi`、`SubscriberApi`、`StreamManagerApi`：流寻址与发布/订阅。
//! - `EventBus`：跨模块发布/订阅系统事件。
//! - `Config*` trait：运行时配置的 schema、校验、应用与回滚。

pub mod config;
pub mod deadline;
pub mod error;
pub mod event;
pub mod idempotency;
pub mod ids;
pub mod media_data_plane;
pub mod media_provider;
pub mod media_session;
pub mod module;
pub mod output;
pub mod service;
pub mod stream;
pub mod task;

#[cfg(feature = "macros")]
pub use cheetah_sdk_macros::ConfigSchema;

pub use cheetah_media_api::image::{
    ImageArtifact, ImageEncodeApi, ImageEncodeRequest, ImageFormat, ImageProcessApi,
    ImageProcessRequest,
};
pub use cheetah_media_api::media_file_store::{
    sanitize_filename, DeleteBatchResult, FileDownload, FileRange, FileStoreEntry, FileStoreQuery,
    MediaFileStoreApi,
};
pub use cheetah_media_api::processing::{
    AbrVariant, AudioCodec, AudioMix, AudioMixInput, AudioTarget, CaptionConfig,
    CreateProcessingJob, ImageInput, ImageOperation, MosaicCell, MosaicLayout, Overlay,
    OverlayKind, OverlayPosition, OverlaySize, ProcessingJob, ProcessingJobQuery,
    ProcessingJobSpec, ProcessingJobState, ProcessingPolicy, ProcessingPreflightReport,
    ProcessingPreset, ProcessingTarget, TrackSelection, UpdateProcessingJob, VideoCodec,
    VideoMosaicInput, VideoTarget,
};
pub use cheetah_media_api::{MediaProcessingApi, ProcessingJobId};
pub use cheetah_runtime_api::{
    AsyncTcpListener, AsyncTcpStream, AsyncTimer, AsyncUdpSocket, CancellationToken,
    ConnectTcpFuture, ConnectTlsFuture, JoinHandle, OneShotReceiver, OneShotRecvError,
    OneShotSendError, OneShotSender, ResolveHostFuture, Runtime, RuntimeApi, SpawnError,
    TaskJoinError, UdpRecvMeta,
};
pub use config::{
    ConfigAdminApi, ConfigApplyApi, ConfigApplyOutcome, ConfigApplyResult, ConfigEffect,
    ConfigProvider, ConfigRollbackToken, ConfigSchemaRegistry, ConfigValidator, ConfigValueChange,
    ModuleConfigChange, ModuleSchemaRegistration, RegisteredSchema,
};
pub use deadline::{cancellation_child, Deadline};
pub use error::SdkError;
pub use event::{
    ConfigEvent, EventBus, EventSubscriber, ModuleEvent, ModuleEventKind, ProtocolEvent,
    StreamEvent, StreamEventKind, SystemEvent, SystemLifecycleEvent, TaskEvent, TaskEventKind,
};
pub use idempotency::{
    canonical_hash, IdempotencyError, IdempotencyFingerprint, IdempotencyKey, IdempotencyOutcome,
    InMemoryIdempotencyRepository,
};
pub use ids::{
    ModuleId, PublisherId, RoomId, SessionId, StreamId, StreamKey, SubscriberId, TaskId,
};
pub use media_data_plane::{
    default_media_data_plane, MediaDataPlaneApi, MediaFramePublisher, MediaFrameSubscriber,
};
pub use media_session::{default_session_directory, MediaSessionDirectoryApi, SessionCloseHandle};
pub use module::{
    EngineContext, HttpHeader, HttpMethod, HttpRequest, HttpResponse, HttpRouteDescriptor,
    HttpRouteMount, MediaServices, Module, ModuleCapability, ModuleFactory, ModuleHttpService,
    ModuleInfo, ModuleInitContext, ModuleManifest, ModuleState, ProviderRegistration,
};
pub use output::{InMemoryMediaOutputRegistry, MediaOutputEndpoint, OutputRegistryRegistration};
pub use service::{
    ClusterApi, ClusterNode, DatabaseApi, HealthApi, MetricLabel, MetricRecord, MetricValue,
    MetricsApi, ModuleConfigApplyReport, ModuleManagerApi, ProxyManager, ProxyRoute,
    RoomParticipant, RoomServiceApi, RoomSnapshot, ServiceDescriptor, ServiceRegistry,
};
pub use stream::{
    BackpressurePolicy, BootstrapMode, BootstrapPolicy, CoreAdaptersApi, DispatchResult,
    MediaFilter, PublishLease, PublisherApi, PublisherOptions, PublisherSink, StreamManagerApi,
    StreamSnapshot, SubscriberApi, SubscriberOptions, SubscriberSource,
};
pub use task::{
    TaskKind, TaskOutcome, TaskSnapshot, TaskState, TaskSystemApi, TaskTerminalOutcome,
};

/// Re-export of the media-domain API crate for use by feature modules.
///
/// 为 feature module 提供的媒体领域 API crate 再导出。
pub use cheetah_media_api as media_api;
