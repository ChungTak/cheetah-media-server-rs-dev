use std::sync::Arc;

use async_trait::async_trait;
use cheetah_codec::{AVFrame, TrackInfo};
use cheetah_sdk::{
    CoreAdaptersApi, DispatchResult, PublishLease, PublisherApi, PublisherOptions, PublisherSink,
    SdkError, StreamKey,
};
use dashmap::DashMap;
use parking_lot::Mutex;

/// Internal adapter that holds a publisher lease and a sink for a stream.
///
/// 内部适配器，持有流的发布者租约与 sink。
struct AdapterPublisher {
    lease: PublishLease,
    sink: Mutex<Box<dyn PublisherSink>>,
}

/// Runtime-neutral core adapter that multiplexes per-stream publishers.
///
/// 运行时无关的核心适配器，按流复用发布者。
pub struct LocalCoreAdapters {
    publisher_api: Arc<dyn PublisherApi>,
    publishers: DashMap<StreamKey, Arc<AdapterPublisher>>,
}

impl LocalCoreAdapters {
    /// Create a core adapter with the given publisher API.
    ///
    /// 用指定发布者 API 创建核心适配器。
    pub fn new(publisher_api: Arc<dyn PublisherApi>) -> Self {
        Self {
            publisher_api,
            publishers: DashMap::new(),
        }
    }

    /// Get or create a publisher for `stream_key`, deduplicating races.
    ///
    /// 获取或创建 `stream_key` 的发布者，并对竞争去重。
    async fn ensure_publisher(
        &self,
        stream_key: StreamKey,
    ) -> Result<Arc<AdapterPublisher>, SdkError> {
        if let Some(existing) = self.publishers.get(&stream_key) {
            return Ok(existing.value().clone());
        }

        let (lease, sink) = self
            .publisher_api
            .acquire_publisher(stream_key.clone(), PublisherOptions::default())
            .await?;
        let publisher = Arc::new(AdapterPublisher {
            lease,
            sink: Mutex::new(sink),
        });

        let winner = match self.publishers.entry(stream_key) {
            dashmap::mapref::entry::Entry::Occupied(entry) => Some(entry.get().clone()),
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(publisher.clone());
                None
            }
        };

        if let Some(existing) = winner {
            self.release_orphan_publisher(publisher.as_ref()).await?;
            return Ok(existing);
        }

        Ok(publisher)
    }

    /// Close and release a publisher that lost the race to be cached.
    ///
    /// 关闭并释放未在缓存中胜出的发布者。
    async fn release_orphan_publisher(&self, publisher: &AdapterPublisher) -> Result<(), SdkError> {
        let close_res = publisher.sink.lock().close();
        let release_res = self.publisher_api.release_publisher(&publisher.lease).await;

        close_res?;

        match release_res {
            Ok(()) => Ok(()),
            Err(SdkError::NotFound(_) | SdkError::Conflict(_)) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Remove and release the publisher for `stream_key` if present.
    ///
    /// 若存在，则移除并释放 `stream_key` 的发布者。
    async fn close_publisher(&self, stream_key: &StreamKey) -> Result<(), SdkError> {
        let Some((_, publisher)) = self.publishers.remove(stream_key) else {
            return Ok(());
        };

        self.release_orphan_publisher(publisher.as_ref()).await
    }
}

/// `CoreAdaptersApi` implementation with publisher acquisition and retry.
///
/// `CoreAdaptersApi` 实现，包含发布者获取与重试。
#[async_trait]
impl CoreAdaptersApi for LocalCoreAdapters {
    async fn publish_frame(
        &self,
        stream_key: StreamKey,
        frame: Arc<AVFrame>,
    ) -> Result<DispatchResult, SdkError> {
        let publisher = self.ensure_publisher(stream_key.clone()).await?;
        let first_try = publisher.sink.lock().push_frame(frame.clone());
        match first_try {
            Ok(DispatchResult::RejectedClosed) => {
                self.close_publisher(&stream_key).await?;
                let retry = self.ensure_publisher(stream_key).await?;
                let result = retry.sink.lock().push_frame(frame);
                result
            }
            Ok(result) => Ok(result),
            Err(SdkError::Unavailable(_) | SdkError::Conflict(_)) => {
                self.close_publisher(&stream_key).await?;
                let retry = self.ensure_publisher(stream_key).await?;
                let result = retry.sink.lock().push_frame(frame);
                result
            }
            Err(err) => Err(err),
        }
    }

    async fn update_tracks(
        &self,
        stream_key: StreamKey,
        tracks: Vec<TrackInfo>,
    ) -> Result<(), SdkError> {
        let publisher = self.ensure_publisher(stream_key.clone()).await?;
        let update_res = {
            let sink = publisher.sink.lock();
            sink.update_tracks(tracks.clone())
        };
        if let Err(err) = update_res {
            self.close_publisher(&stream_key).await?;
            if matches!(err, SdkError::Unavailable(_) | SdkError::Conflict(_)) {
                let retry = self.ensure_publisher(stream_key).await?;
                retry.sink.lock().update_tracks(tracks)?;
                return Ok(());
            }
            return Err(err);
        }
        Ok(())
    }

    async fn close_stream(&self, stream_key: &StreamKey) -> Result<(), SdkError> {
        self.close_publisher(stream_key).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use async_trait::async_trait;
    use bytes::Bytes;
    use cheetah_codec::{CodecId, FrameFormat, MediaKind, Timebase, TrackId};
    use parking_lot::Mutex as ParkingMutex;

    use super::*;

    #[derive(Clone, Copy)]
    enum SinkBehavior {
        Accept,
        PushUnavailableThenAccept,
    }

    struct MockSink {
        close_count: Arc<AtomicU64>,
        behavior: SinkBehavior,
        first_push: AtomicBool,
    }

    impl PublisherSink for MockSink {
        fn update_tracks(&self, _tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
            Ok(())
        }

        fn push_frame(&self, _frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError> {
            if matches!(self.behavior, SinkBehavior::PushUnavailableThenAccept)
                && self.first_push.swap(false, Ordering::AcqRel)
            {
                return Err(SdkError::Unavailable("stale sink".to_string()));
            }
            Ok(DispatchResult::Accepted)
        }

        fn close(&self) -> Result<(), SdkError> {
            self.close_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn take_keyframe_requests(&self) -> u64 {
            0
        }
    }

    struct MockPublisherApi {
        next_lease_id: AtomicU64,
        acquire_count: AtomicU64,
        release_count: AtomicU64,
        close_count: Arc<AtomicU64>,
        behaviors: ParkingMutex<VecDeque<SinkBehavior>>,
    }

    impl MockPublisherApi {
        fn new(behaviors: Vec<SinkBehavior>) -> Self {
            Self {
                next_lease_id: AtomicU64::new(0),
                acquire_count: AtomicU64::new(0),
                release_count: AtomicU64::new(0),
                close_count: Arc::new(AtomicU64::new(0)),
                behaviors: ParkingMutex::new(behaviors.into()),
            }
        }
    }

    #[async_trait]
    impl PublisherApi for MockPublisherApi {
        async fn acquire_publisher(
            &self,
            stream_key: StreamKey,
            _options: PublisherOptions,
        ) -> Result<(PublishLease, Box<dyn PublisherSink>), SdkError> {
            let lease_id = self.next_lease_id.fetch_add(1, Ordering::Relaxed) + 1;
            self.acquire_count.fetch_add(1, Ordering::Relaxed);
            let behavior = self
                .behaviors
                .lock()
                .pop_front()
                .unwrap_or(SinkBehavior::Accept);
            Ok((
                PublishLease {
                    stream_id: cheetah_sdk::StreamId(1),
                    stream_key,
                    lease_id,
                },
                Box::new(MockSink {
                    close_count: self.close_count.clone(),
                    behavior,
                    first_push: AtomicBool::new(true),
                }),
            ))
        }

        async fn release_publisher(&self, _lease: &PublishLease) -> Result<(), SdkError> {
            self.release_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    fn frame() -> Arc<AVFrame> {
        Arc::new(AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 1000),
            Bytes::from_static(b"x"),
        ))
    }

    #[tokio::test(flavor = "current_thread")]
    async fn publish_frame_retries_on_unavailable() {
        let api = Arc::new(MockPublisherApi::new(vec![
            SinkBehavior::PushUnavailableThenAccept,
            SinkBehavior::Accept,
        ]));
        let adapters = LocalCoreAdapters::new(api.clone());
        let result = adapters
            .publish_frame(StreamKey::new("live", "a"), frame())
            .await
            .expect("publish frame");
        assert_eq!(result, DispatchResult::Accepted);
        assert_eq!(api.acquire_count.load(Ordering::Relaxed), 2);
        assert_eq!(api.release_count.load(Ordering::Relaxed), 1);
        assert_eq!(api.close_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_first_publish_does_not_leak_orphan_publisher() {
        let api = Arc::new(MockPublisherApi::new(vec![
            SinkBehavior::Accept,
            SinkBehavior::Accept,
            SinkBehavior::Accept,
        ]));
        let adapters = Arc::new(LocalCoreAdapters::new(api.clone()));
        let key = StreamKey::new("live", "race");

        let a1 = adapters.clone();
        let k1 = key.clone();
        let t1 = tokio::spawn(async move { a1.publish_frame(k1, frame()).await });
        let a2 = adapters.clone();
        let k2 = key.clone();
        let t2 = tokio::spawn(async move { a2.publish_frame(k2, frame()).await });

        let r1 = t1.await.expect("join 1").expect("publish 1");
        let r2 = t2.await.expect("join 2").expect("publish 2");
        assert_eq!(r1, DispatchResult::Accepted);
        assert_eq!(r2, DispatchResult::Accepted);

        adapters.close_stream(&key).await.expect("close stream");

        let acquires = api.acquire_count.load(Ordering::Relaxed);
        let releases = api.release_count.load(Ordering::Relaxed);
        let closes = api.close_count.load(Ordering::Relaxed);
        assert_eq!(
            releases, acquires,
            "every acquired publisher must be released"
        );
        assert_eq!(closes, acquires, "every acquired publisher must be closed");
    }
}
