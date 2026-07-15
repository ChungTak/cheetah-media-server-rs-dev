use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
use cheetah_codec::{AVFrame, TrackInfo};
use cheetah_media_api::command::{PublishRequest, SubscribeRequest};
use cheetah_media_api::error::{MediaError, Result as MediaResult};
use cheetah_media_api::ids::MediaKey;
use cheetah_media_api::port::MediaRequestContext;
use cheetah_sdk::media_api::ids::StreamKeyBridge;
use cheetah_sdk::media_data_plane::{MediaDataPlaneApi, MediaFramePublisher, MediaFrameSubscriber};
use cheetah_sdk::stream::{PublisherApi, PublisherSink, SubscriberApi, SubscriberSource};
use cheetah_sdk::{PublisherOptions, SdkError, StreamKey, SubscriberId, SubscriberOptions};
use tokio::sync::Mutex;

pub struct EngineMediaDataPlane {
    publisher_api: Arc<dyn PublisherApi>,
    subscriber_api: Arc<dyn SubscriberApi>,
}

impl EngineMediaDataPlane {
    pub fn new(
        publisher_api: Arc<dyn PublisherApi>,
        subscriber_api: Arc<dyn SubscriberApi>,
    ) -> Self {
        Self {
            publisher_api,
            subscriber_api,
        }
    }

    fn media_key_to_stream_key(key: &MediaKey) -> StreamKey {
        let (namespace, path) = StreamKeyBridge::to_namespace_path(key);
        StreamKey::new(namespace, path)
    }

    fn map_sdk_error(err: SdkError) -> MediaError {
        match err {
            SdkError::NotFound(msg) => MediaError::not_found(msg),
            SdkError::AlreadyExists(msg) => MediaError::already_exists(msg),
            SdkError::InvalidArgument(msg) => MediaError::invalid_argument(msg),
            SdkError::Conflict(msg) => MediaError::conflict(msg),
            SdkError::Unavailable(msg) => MediaError::unavailable(msg),
            SdkError::Internal(msg) => MediaError::internal(msg),
        }
    }
}

#[async_trait]
impl MediaDataPlaneApi for EngineMediaDataPlane {
    async fn open_frame_publisher(
        &self,
        _ctx: &MediaRequestContext,
        request: PublishRequest,
    ) -> MediaResult<Box<dyn MediaFramePublisher>> {
        let stream_key = Self::media_key_to_stream_key(&request.media_key);
        let options = PublisherOptions {
            announce_tracks: true,
            protocol: request.protocol.clone(),
            remote_endpoint: request.remote_endpoint.clone(),
        };
        let (_, sink) = self
            .publisher_api
            .acquire_publisher(stream_key, options)
            .await
            .map_err(Self::map_sdk_error)?;
        Ok(Box::new(FramePublisher {
            sink,
            closed: AtomicBool::new(false),
        }))
    }

    async fn open_frame_subscriber(
        &self,
        _ctx: &MediaRequestContext,
        request: SubscribeRequest,
    ) -> MediaResult<Box<dyn MediaFrameSubscriber>> {
        let stream_key = Self::media_key_to_stream_key(&request.media_key);
        let source = self
            .subscriber_api
            .subscribe(stream_key, SubscriberOptions::default())
            .await
            .map_err(Self::map_sdk_error)?;
        let id = source.id();
        Ok(Box::new(FrameSubscriber {
            id,
            source: Mutex::new(source),
            closed: AtomicBool::new(false),
        }))
    }
}

struct FramePublisher {
    sink: Box<dyn PublisherSink>,
    closed: AtomicBool,
}

impl FramePublisher {
    fn map_sdk_error(err: SdkError) -> MediaError {
        EngineMediaDataPlane::map_sdk_error(err)
    }
}

#[async_trait]
impl MediaFramePublisher for FramePublisher {
    fn update_tracks(&self, tracks: Vec<TrackInfo>) -> MediaResult<()> {
        self.sink.update_tracks(tracks).map_err(Self::map_sdk_error)
    }

    fn push_frame(&self, frame: Arc<AVFrame>) -> MediaResult<()> {
        self.sink
            .push_frame(frame)
            .map(|_| ())
            .map_err(Self::map_sdk_error)
    }

    async fn close(&self) -> MediaResult<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.sink.close().map_err(Self::map_sdk_error)
    }

    fn take_keyframe_requests(&self) -> u64 {
        self.sink.take_keyframe_requests()
    }
}

struct FrameSubscriber {
    id: SubscriberId,
    source: Mutex<Box<dyn SubscriberSource>>,
    closed: AtomicBool,
}

impl FrameSubscriber {
    fn map_sdk_error(err: SdkError) -> MediaError {
        EngineMediaDataPlane::map_sdk_error(err)
    }
}

#[async_trait]
impl MediaFrameSubscriber for FrameSubscriber {
    async fn recv(&mut self) -> MediaResult<Option<Arc<AVFrame>>> {
        let mut guard = self.source.lock().await;
        guard.recv().await.map_err(Self::map_sdk_error)
    }

    async fn close(&mut self) -> MediaResult<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let mut guard = self.source.lock().await;
        guard.close().await.map_err(Self::map_sdk_error)
    }

    fn id(&self) -> SubscriberId {
        self.id
    }

    fn tracks(&self) -> Vec<TrackInfo> {
        if let Ok(guard) = self.source.try_lock() {
            guard.tracks()
        } else {
            Vec::new()
        }
    }
}
