//! `cheetah-media-api` defines the media-domain ports, models, events, and errors
//! used by the Cheetah streaming server.
//!
//! This crate is intentionally runtime-neutral: it does not depend on Tokio,
//! Axum, any specific protocol module, or `cheetah-sdk`. It provides stable
//! typed contracts that adapters and providers can implement against.
//!
//! `cheetah-media-api` 定义了 Cheetah 流媒体服务器使用的媒体领域端口、模型、事件和错误。
//!
//! 本 crate 刻意保持运行时无关：不依赖 Tokio、Axum、任何具体协议 module 或 `cheetah-sdk`。
//! 它提供稳定的类型化契约，供 adapter 和 provider 实现。

pub mod audit;
pub mod auth;
pub mod capability;
pub mod command;
pub mod error;
pub mod event;
pub mod ids;
pub mod image;
pub mod media_file_store;
pub mod model;
pub mod output;
pub mod port;

pub use auth::{AuthCredentials, MediaScope, Principal};
pub use capability::{
    MediaCapability, MediaCapabilityDescriptor, MediaCapabilityReport, MediaCapabilitySet,
};
pub use error::{MediaError, MediaErrorCode};
pub use event::MediaEvent;
pub use ids::{AppName, MediaKey, MediaSchema, StreamName, VhostName};
pub use image::{ImageArtifact, ImageEncodeApi, ImageEncodeRequest, ImageFormat};
pub use media_file_store::{
    sanitize_filename, DeleteBatchResult, FileDownload, FileRange, FileStoreEntry, FileStoreQuery,
    MediaFileStoreApi,
};
pub use model::{AdmissionAction, AdmissionRequest, Decision};
pub use output::{EndpointState, MediaOutputEndpoint};
pub use port::{
    ControlAuthApi, MediaAdmissionApi, MediaControlApi, MediaFacade, MediaOutputRegistryApi,
    MediaRequestContext, MediaUrlResolverApi, ProxyApi, PublishSubscribeApi, RecordApi, RtpApi,
    SnapshotApi, WebhookApi,
};
