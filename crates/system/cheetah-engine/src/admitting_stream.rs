//! Admission-aware wrappers around `PublisherApi` / `SubscriberApi`.
//!
//! Protocol modules acquire leases through `EngineContext::{publisher_api,subscriber_api}`.
//! These wrappers ensure `MediaAdmissionApi::authorize` runs before any lease is
//! created, so RTMP/RTSP/WebRTC (and any future protocol) cannot bypass webhook
//! admission. Control-plane paths that already admit via `EngineMediaFacade`
//! continue to use the raw stream manager through the data plane.
//!
//! 带准入检查的 `PublisherApi` / `SubscriberApi` 包装。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::error::MediaError;
use cheetah_media_api::ids::StreamKeyBridge;
use cheetah_media_api::model::{AdmissionAction, AdmissionRequest, Decision};
use cheetah_media_api::port::MediaRequestContext;
use cheetah_sdk::stream::{
    PublishLease, PublisherApi, PublisherOptions, PublisherSink, SubscriberApi, SubscriberOptions,
    SubscriberSource,
};
use cheetah_sdk::{MediaServices, SdkError, StreamKey};

/// Wrap a `PublisherApi` so every acquire goes through media admission.
///
/// 包装 `PublisherApi`，使每次获取租约都经过媒体准入。
pub struct AdmittingPublisherApi {
    inner: Arc<dyn PublisherApi>,
    media_services: MediaServices,
}

impl AdmittingPublisherApi {
    pub fn new(inner: Arc<dyn PublisherApi>, media_services: MediaServices) -> Self {
        Self {
            inner,
            media_services,
        }
    }
}

/// Wrap a `SubscriberApi` so every subscribe goes through media admission.
///
/// 包装 `SubscriberApi`，使每次订阅都经过媒体准入。
pub struct AdmittingSubscriberApi {
    inner: Arc<dyn SubscriberApi>,
    media_services: MediaServices,
}

impl AdmittingSubscriberApi {
    pub fn new(inner: Arc<dyn SubscriberApi>, media_services: MediaServices) -> Self {
        Self {
            inner,
            media_services,
        }
    }
}

async fn authorize(
    media_services: &MediaServices,
    action: AdmissionAction,
    stream_key: &StreamKey,
    protocol: String,
    source_address: Option<String>,
) -> Result<(), SdkError> {
    let Some(provider) = media_services.admission() else {
        return Ok(());
    };
    let resource = StreamKeyBridge::from_namespace_path(&stream_key.namespace, &stream_key.path)
        .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
    let ctx = MediaRequestContext::default();
    let request = AdmissionRequest {
        action,
        principal: ctx.principal.clone(),
        resource,
        protocol,
        source_address,
        params: HashMap::new(),
    };
    match provider.authorize(&ctx, request).await {
        Ok(Decision::Allow) => Ok(()),
        Ok(Decision::Deny { code, reason }) => Err(map_deny(code, reason)),
        Err(e) => Err(map_media_error(e)),
    }
}

fn map_deny(code: cheetah_media_api::error::MediaErrorCode, reason: String) -> SdkError {
    match code {
        cheetah_media_api::error::MediaErrorCode::PermissionDenied => {
            SdkError::InvalidArgument(format!("admission denied: {reason}"))
        }
        cheetah_media_api::error::MediaErrorCode::Unavailable => SdkError::Unavailable(reason),
        cheetah_media_api::error::MediaErrorCode::NotFound => SdkError::NotFound(reason),
        cheetah_media_api::error::MediaErrorCode::Conflict
        | cheetah_media_api::error::MediaErrorCode::AlreadyExists => SdkError::Conflict(reason),
        cheetah_media_api::error::MediaErrorCode::InvalidArgument => {
            SdkError::InvalidArgument(reason)
        }
        _ => SdkError::Internal(format!("admission denied ({code:?}): {reason}")),
    }
}

fn map_media_error(err: MediaError) -> SdkError {
    let message = err.message.to_string();
    match err.code {
        cheetah_media_api::error::MediaErrorCode::NotFound => SdkError::NotFound(message),
        cheetah_media_api::error::MediaErrorCode::AlreadyExists => SdkError::AlreadyExists(message),
        cheetah_media_api::error::MediaErrorCode::InvalidArgument
        | cheetah_media_api::error::MediaErrorCode::PermissionDenied
        | cheetah_media_api::error::MediaErrorCode::Unauthenticated => {
            SdkError::InvalidArgument(message)
        }
        cheetah_media_api::error::MediaErrorCode::Conflict => SdkError::Conflict(message),
        cheetah_media_api::error::MediaErrorCode::Unavailable
        | cheetah_media_api::error::MediaErrorCode::Busy
        | cheetah_media_api::error::MediaErrorCode::Timeout => SdkError::Unavailable(message),
        _ => SdkError::Internal(message),
    }
}

#[async_trait]
impl PublisherApi for AdmittingPublisherApi {
    async fn acquire_publisher(
        &self,
        stream_key: StreamKey,
        options: PublisherOptions,
    ) -> Result<(PublishLease, Box<dyn PublisherSink>), SdkError> {
        authorize(
            &self.media_services,
            AdmissionAction::Publish,
            &stream_key,
            options.protocol.clone(),
            options.remote_endpoint.clone(),
        )
        .await?;
        self.inner.acquire_publisher(stream_key, options).await
    }

    async fn release_publisher(&self, lease: &PublishLease) -> Result<(), SdkError> {
        self.inner.release_publisher(lease).await
    }
}

#[async_trait]
impl SubscriberApi for AdmittingSubscriberApi {
    async fn subscribe(
        &self,
        stream_key: StreamKey,
        options: SubscriberOptions,
    ) -> Result<Box<dyn SubscriberSource>, SdkError> {
        authorize(
            &self.media_services,
            AdmissionAction::Play,
            &stream_key,
            String::new(),
            None,
        )
        .await?;
        self.inner.subscribe(stream_key, options).await
    }
}
