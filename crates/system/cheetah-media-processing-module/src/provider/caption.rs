//! Caption extraction job provider and worker.
//!
//! Implements `MediaProcessingApi` for `CaptionExtract` jobs: subscribe to a
//! source H.264/H.265 stream, parse CEA-608/708 closed captions with the Sans-I/O
//! `CeaParser`, and publish derived `WebVttCue` frames on a target stream.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_codec::{
    cea::{CeaParser, CeaParserConfig},
    frame::{FrameFlags, FrameFormat, FrameOrigin},
    subtitle::WebVttCue,
    time::Timebase,
    track::{CodecId, MediaKind, TrackId, TrackInfo, TrackReadiness},
    video::{AccessUnitAssembler, AccessUnitTiming},
    AVFrame,
};
use cheetah_media_api::{
    error::{MediaError, Result as MediaResult},
    ids::{MediaKey, ProcessingJobId, StreamKeyBridge},
    model::{AdmissionAction, AdmissionRequest, Decision, Page},
    port::MediaProcessingApi,
    processing::{
        CreateProcessingJob, ProcessingJob, ProcessingJobQuery, ProcessingJobSpec,
        ProcessingJobState, ProcessingPreflightReport, UpdateProcessingJob,
    },
    MediaCapability, MediaCapabilitySet, MediaRequestContext,
};
use cheetah_sdk::{
    BackpressurePolicy, BootstrapPolicy, CancellationToken, DispatchResult, EngineContext,
    MediaFilter, PublisherOptions, PublisherSink, SdkError, StreamKey, SubscriberOptions,
    SubscriberSource,
};
use futures::FutureExt;
use tracing::{info, warn};

use crate::config::MediaProcessingModuleConfig;

/// In-memory handle for a running or stopped caption job.
struct JobEntry {
    job: ProcessingJob,
    cancel: CancellationToken,
    handle: Option<Box<dyn cheetah_sdk::JoinHandle>>,
}

/// `MediaProcessingApi` provider that currently supports `CaptionExtract` jobs.
pub struct MediaProcessingProvider {
    ctx: EngineContext,
    config: MediaProcessingModuleConfig,
    jobs: Arc<Mutex<HashMap<ProcessingJobId, JobEntry>>>,
    id_counter: AtomicU64,
}

impl MediaProcessingProvider {
    pub fn new(ctx: EngineContext, config: MediaProcessingModuleConfig) -> Self {
        Self {
            ctx,
            config,
            jobs: Arc::new(Mutex::new(HashMap::new())),
            id_counter: AtomicU64::new(0),
        }
    }

    fn media_key_to_stream_key(key: &MediaKey) -> StreamKey {
        let (namespace, path) = StreamKeyBridge::to_namespace_path(key);
        StreamKey::new(namespace, path)
    }

    async fn authorize(
        &self,
        ctx: &MediaRequestContext,
        action: AdmissionAction,
        resource: &MediaKey,
    ) -> MediaResult<()> {
        if let Some(admission) = self.ctx.media_services.admission() {
            let decision = admission
                .authorize(
                    ctx,
                    AdmissionRequest {
                        action,
                        principal: ctx.principal.clone(),
                        resource: resource.clone(),
                        protocol: "caption-extract".to_string(),
                        source_address: None,
                        params: HashMap::new(),
                    },
                )
                .await?;
            if let Decision::Deny { code, reason } = decision {
                return Err(MediaError::new(code, reason));
            }
        }
        Ok(())
    }

    fn now_us() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as i64
    }

    fn new_job_id(&self) -> ProcessingJobId {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let n = self.id_counter.fetch_add(1, Ordering::Relaxed);
        ProcessingJobId(format!("cap-{ts}-{n}"))
    }

    pub fn default_capabilities() -> MediaCapabilitySet {
        let mut set = MediaCapabilitySet::empty();
        set.add(MediaCapability::VideoProcessing, 1);
        set.set_reason(
            MediaCapability::VideoProcessing,
            "caption extraction (H.264/H.265 -> WebVTT)",
        );
        set
    }

    async fn source_has_video(&self, source: &MediaKey) -> MediaResult<bool> {
        let key = Self::media_key_to_stream_key(source);
        let snapshot = self
            .ctx
            .stream_manager_api
            .get_stream(&key)
            .await
            .map_err(|e| MediaError::internal(format!("stream lookup failed: {e}")))?;
        match snapshot {
            Some(s) => Ok(s.tracks.iter().any(|t| {
                t.media_kind == MediaKind::Video && matches!(t.codec, CodecId::H264 | CodecId::H265)
            })),
            None => Ok(false),
        }
    }

    fn build_job(&self, request: &CreateProcessingJob, state: ProcessingJobState) -> ProcessingJob {
        let now = Self::now_us();
        ProcessingJob {
            job_id: self.new_job_id(),
            spec: request.spec.clone(),
            state,
            generation: 1,
            profile: self.config.profile.clone(),
            created_at: now,
            updated_at: now,
            started_at: Some(now),
            finished_at: None,
            input_keys: match &request.spec {
                ProcessingJobSpec::CaptionExtract { source, .. } => vec![source.clone()],
                _ => Vec::new(),
            },
            output_keys: match &request.spec {
                ProcessingJobSpec::CaptionExtract { target, .. } => vec![target.clone()],
                _ => Vec::new(),
            },
            ref_count: 1,
            restart_count: 0,
            frames_in: 0,
            frames_out: 0,
            bytes_in: 0,
            bytes_out: 0,
            drops: 0,
            pending: 0,
            flushes: 0,
            resets: 0,
            last_error: None,
        }
    }

    /// Cancel every running job and wait for the worker tasks to complete.
    pub async fn cancel_all(&self) {
        let handles = {
            let mut jobs = self.jobs.lock().unwrap();
            for entry in jobs.values_mut() {
                entry.cancel.cancel();
                entry.job.state = ProcessingJobState::Stopped;
                entry.job.updated_at = Self::now_us();
            }
            jobs.values_mut()
                .filter_map(|e| e.handle.take())
                .collect::<Vec<_>>()
        };
        let _ = futures::future::join_all(handles.into_iter().map(|h| h.wait())).await;
    }
}

#[async_trait]
impl MediaProcessingApi for MediaProcessingProvider {
    async fn preflight(
        &self,
        _ctx: &MediaRequestContext,
    ) -> MediaResult<ProcessingPreflightReport> {
        let available = true;
        let mut diagnostics = HashMap::new();
        if !cfg!(feature = "media-processing-caption") {
            diagnostics.insert(
                "caption".to_string(),
                "media-processing-caption feature not compiled".to_string(),
            );
        }
        Ok(ProcessingPreflightReport {
            profile: self.config.profile.clone(),
            available,
            operations: vec!["caption_extract".to_string()],
            diagnostics,
        })
    }

    async fn create_job(
        &self,
        ctx: &MediaRequestContext,
        request: CreateProcessingJob,
    ) -> MediaResult<ProcessingJob> {
        let ProcessingJobSpec::CaptionExtract {
            source,
            target,
            caption: _,
        } = &request.spec
        else {
            return Err(MediaError::unsupported(
                "only CaptionExtract processing jobs are supported",
            ));
        };

        let source_key = Self::media_key_to_stream_key(source);
        let target_key = Self::media_key_to_stream_key(target);

        // Admission check must happen before any stream resource is allocated.
        self.authorize(ctx, AdmissionAction::Play, source).await?;
        self.authorize(ctx, AdmissionAction::Publish, target)
            .await?;

        if !self.source_has_video(source).await? {
            return Err(MediaError::invalid_argument(format!(
                "source stream {source_key} has no H.264/H.265 video track"
            )));
        }

        let sub_options = SubscriberOptions {
            queue_capacity: 150,
            backpressure: BackpressurePolicy::DropDroppableFirst,
            bootstrap_policy: BootstrapPolicy::default(),
            media_filter: MediaFilter {
                enable_video: true,
                enable_audio: false,
            },
        };
        let subscriber = self
            .ctx
            .subscriber_api
            .subscribe(source_key, sub_options)
            .await
            .map_err(|e| MediaError::internal(format!("subscribe failed: {e}")))?;

        let pub_options = PublisherOptions {
            announce_tracks: true,
            protocol: "caption-extract".to_string(),
            remote_endpoint: None,
        };
        let (lease, publisher) = self
            .ctx
            .publisher_api
            .acquire_publisher(target_key, pub_options)
            .await
            .map_err(|e| MediaError::internal(format!("acquire publisher failed: {e}")))?;

        let mut worker = CaptionExtractWorker::new(CeaParserConfig::default());
        if let Err(e) = publisher.update_tracks(vec![worker.output_track()]) {
            let _ = self.ctx.publisher_api.release_publisher(&lease).await;
            return Err(MediaError::internal(format!("update tracks failed: {e}")));
        }

        let cancel = CancellationToken::new();
        let cancel_child = cancel.child_token();
        let runtime = self.ctx.runtime_api.clone();

        let job = self.build_job(&request, ProcessingJobState::Running);
        let job_id = job.job_id.clone();

        let jobs = self.jobs.clone();
        let publisher_api = self.ctx.publisher_api.clone();
        let handle = runtime.spawn(Box::pin(async move {
            let result = worker.run(subscriber, publisher, cancel_child).await;
            let finished_at = Self::now_us();
            if let Ok(mut guard) = jobs.lock() {
                if let Some(entry) = guard.get_mut(&job_id) {
                    entry.job.finished_at = Some(finished_at);
                    if let Err(ref e) = result {
                        entry.job.last_error = Some(e.to_string());
                    }
                    if entry.job.state == ProcessingJobState::Running {
                        entry.job.state = ProcessingJobState::Stopped;
                        entry.job.updated_at = finished_at;
                    }
                }
            }
            if let Err(e) = result {
                warn!(job_id = %job_id, "caption extract worker failed: {e}");
            }
            let _ = publisher_api.release_publisher(&lease).await;
        }));

        self.jobs.lock().unwrap().insert(
            job.job_id.clone(),
            JobEntry {
                job: job.clone(),
                cancel,
                handle: Some(handle),
            },
        );

        info!(job_id = %job.job_id, "caption extract job started");
        Ok(job)
    }

    async fn get_job(
        &self,
        _ctx: &MediaRequestContext,
        id: &ProcessingJobId,
    ) -> MediaResult<ProcessingJob> {
        let jobs = self.jobs.lock().unwrap();
        jobs.get(id)
            .map(|e| e.job.clone())
            .ok_or_else(|| MediaError::not_found(format!("job {id} not found")))
    }

    async fn list_jobs(
        &self,
        _ctx: &MediaRequestContext,
        mut query: ProcessingJobQuery,
    ) -> MediaResult<Page<ProcessingJob>> {
        query.clamp_page_size();
        let jobs = self.jobs.lock().unwrap();
        let mut items: Vec<ProcessingJob> = jobs
            .values()
            .map(|e| e.job.clone())
            .filter(|j| {
                query.state.is_none_or(|s| j.state == s)
                    && query.vhost.as_ref().is_none_or(|v| {
                        j.input_keys
                            .first()
                            .is_some_and(|k| k.vhost.0.as_str() == v.as_str())
                    })
                    && query.app.as_ref().is_none_or(|a| {
                        j.input_keys
                            .first()
                            .is_some_and(|k| k.app.0.as_str() == a.as_str())
                    })
                    && query.stream.as_ref().is_none_or(|s| {
                        j.input_keys
                            .first()
                            .is_some_and(|k| k.stream.0.as_str() == s.as_str())
                    })
            })
            .collect();
        let total = items.len() as u64;
        let page = query.page.max(1);
        let page_size = query.page_size as usize;
        let start = ((page - 1) as usize).saturating_mul(page_size);
        items = if start > items.len() {
            Vec::new()
        } else {
            items.into_iter().skip(start).take(page_size).collect()
        };
        Ok(Page {
            items,
            page,
            page_size: page_size as u64,
            total,
            next_cursor: None,
        })
    }

    async fn update_job(
        &self,
        _ctx: &MediaRequestContext,
        _request: UpdateProcessingJob,
    ) -> MediaResult<ProcessingJob> {
        Err(MediaError::unsupported(
            "update is not supported for caption extract jobs",
        ))
    }

    async fn stop_job(
        &self,
        _ctx: &MediaRequestContext,
        id: &ProcessingJobId,
    ) -> MediaResult<ProcessingJob> {
        let mut jobs = self.jobs.lock().unwrap();
        let entry = jobs
            .get_mut(id)
            .ok_or_else(|| MediaError::not_found(format!("job {id} not found")))?;
        entry.cancel.cancel();
        entry.job.state = ProcessingJobState::Stopped;
        entry.job.updated_at = Self::now_us();
        Ok(entry.job.clone())
    }

    async fn delete_job(
        &self,
        _ctx: &MediaRequestContext,
        id: &ProcessingJobId,
    ) -> MediaResult<()> {
        let mut jobs = self.jobs.lock().unwrap();
        let entry = jobs
            .get_mut(id)
            .ok_or_else(|| MediaError::not_found(format!("job {id} not found")))?;
        entry.cancel.cancel();
        jobs.remove(id);
        Ok(())
    }
}

/// Worker that drives `CeaParser` from a video subscriber and publishes WebVTT cues.
pub struct CaptionExtractWorker {
    parser: CeaParser,
}

impl CaptionExtractWorker {
    pub fn new(config: CeaParserConfig) -> Self {
        Self {
            parser: CeaParser::new(config),
        }
    }

    pub fn output_track(&self) -> TrackInfo {
        let mut track = TrackInfo::new(TrackId(0), MediaKind::Subtitle, CodecId::WebVtt, 1000);
        track.readiness = TrackReadiness::Ready;
        track
    }

    /// Convert one video `AVFrame` into WebVTT cues.
    ///
    /// Supports both Annex-B and length-prefixed H.264/H.265 payloads.
    pub fn process_frame(&mut self, frame: &AVFrame) -> Vec<WebVttCue> {
        if !matches!(frame.codec, CodecId::H264 | CodecId::H265) {
            return Vec::new();
        }

        let mut assembler = AccessUnitAssembler::default();
        if is_annexb(&frame.payload) {
            assembler.push_annexb(&frame.payload);
        } else {
            match assembler.push_length_prefixed_checked(&frame.payload) {
                Ok(()) => {}
                Err(_) => assembler.push_annexb(&frame.payload),
            }
        }

        let mut unit = assembler.take_access_unit();
        unit.timing = Some(AccessUnitTiming {
            pts: frame.pts,
            dts: frame.dts,
            duration: frame.duration,
            timebase: frame.timebase,
        });

        let mut cues = Vec::new();
        if frame.flags.contains(FrameFlags::DISCONTINUITY) {
            let pts_ms = (frame.pts_us.max(0) / 1000) as u64;
            cues.extend(self.parser.reset(Some(pts_ms)));
        }
        cues.extend(self.parser.push_access_unit(frame.codec, &unit));
        cues
    }

    /// Run the worker loop until the subscriber ends or cancellation is requested.
    pub async fn run(
        &mut self,
        mut subscriber: Box<dyn SubscriberSource>,
        publisher: Box<dyn PublisherSink>,
        cancel: CancellationToken,
    ) -> Result<(), SdkError> {
        publisher.update_tracks(vec![self.output_track()])?;

        let mut cancel_fut = cancel.cancelled().fuse();
        let mut recv_fut = subscriber.recv().fuse();

        let mut last_pts_ms: u64 = 0;

        loop {
            futures::select! {
                frame = recv_fut => {
                    match frame {
                        Ok(Some(frame)) => {
                            last_pts_ms = (frame.pts_us.max(0) / 1000) as u64;
                            for cue in self.process_frame(&frame) {
                                self.push_cue(&*publisher, cue)?;
                            }
                            drop(recv_fut);
                            drop(cancel_fut);
                            recv_fut = subscriber.recv().fuse();
                            cancel_fut = cancel.cancelled().fuse();
                        }
                        Ok(None) => break,
                        Err(e) => {
                            let _ = publisher.close();
                            return Err(e);
                        }
                    }
                }
                _ = cancel_fut => break,
            }
        }

        for cue in self.parser.reset(Some(last_pts_ms)) {
            self.push_cue(&*publisher, cue)?;
        }

        publisher.close()
    }

    fn push_cue(&self, publisher: &dyn PublisherSink, cue: WebVttCue) -> Result<(), SdkError> {
        let payload = serde_json::to_vec(&cue)
            .map_err(|e| SdkError::Internal(format!("failed to serialize WebVTT cue: {e}")))?;
        let duration = (cue.end_ms - cue.start_ms) as i64;
        let mut frame = AVFrame::new(
            TrackId(0),
            MediaKind::Subtitle,
            CodecId::WebVtt,
            FrameFormat::WebVttPacket,
            cue.start_ms as i64,
            cue.start_ms as i64,
            Timebase::new(1, 1000),
            Bytes::from(payload),
        );
        frame.duration = duration;
        frame.duration_us = duration * 1000;
        frame.flags = FrameFlags::GENERATED;
        frame.origin = FrameOrigin::Generated;
        match publisher.push_frame(Arc::new(frame))? {
            DispatchResult::Accepted | DispatchResult::DroppedByPolicy => Ok(()),
            DispatchResult::RejectedClosed => Err(SdkError::Internal("publisher closed".into())),
        }
    }
}

fn is_annexb(payload: &[u8]) -> bool {
    payload.starts_with(&[0, 0, 0, 1]) || payload.starts_with(&[0, 0, 1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use bytes::Bytes;
    use cheetah_codec::{AVFrame, CodecId, FrameFormat, MediaKind, Timebase, TrackId};
    use cheetah_sdk::{DispatchResult, PublisherSink, SubscriberId, SubscriberSource};
    use std::sync::Arc;

    fn make_h264_sei_nalu(user_data_payload: &[u8]) -> Bytes {
        let mut out = Vec::new();
        out.push(0x06); // SEI NAL unit type
        out.push(0x04); // payload type 4 = user_data_registered_itu_t_t35
        let mut size = user_data_payload.len();
        while size >= 255 {
            out.push(0xFF);
            size -= 255;
        }
        out.push(size as u8);
        out.extend_from_slice(user_data_payload);
        Bytes::from(out)
    }

    fn pop_on_cc_payload(text: &str) -> Vec<u8> {
        let triplets: Vec<(bool, u8, u8)> = vec![
            (true, 0x14, 0x20), // RCL
            (true, 0x14, 0x70), // PAC
            (true, text.as_bytes()[0], text.as_bytes()[1]),
            (true, 0x14, 0x2F), // EOC
        ];
        let mut cc = Vec::new();
        cc.push(0xB5);
        cc.extend_from_slice(&0x0031u16.to_be_bytes());
        cc.extend_from_slice(b"GA94");
        cc.push(0x03);
        cc.push(0xC0 | (triplets.len() as u8 & 0x1F));
        cc.push(0xFF);
        for (valid, d1, d2) in triplets {
            cc.push(0xF8 | (u8::from(valid) << 2));
            cc.push(d1);
            cc.push(d2);
        }
        cc.push(0xFF);
        cc
    }

    fn make_video_frame(payload: Bytes, pts_us: i64) -> AVFrame {
        AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            pts_us,
            pts_us,
            Timebase::new(1, 90_000),
            payload,
        )
    }

    #[test]
    fn process_frame_extracts_pop_on_cue() {
        let mut worker = CaptionExtractWorker::new(CeaParserConfig::default());
        let sei = make_h264_sei_nalu(&pop_on_cc_payload("HI"));
        let mut payload = Bytes::from_static(&[0, 0, 0, 1]);
        payload = Bytes::from([payload.as_ref(), sei.as_ref()].concat());
        let frame = make_video_frame(payload, 0);

        let cues = worker.process_frame(&frame);
        assert!(
            cues.is_empty(),
            "first access unit only changes state, no cue yet"
        );

        let cues = worker.parser.reset(Some(1000));
        assert_eq!(cues.len(), 1);
        let cue = &cues[0];
        assert!(cue.payload.contains('H'));
        assert!(cue.payload.contains('I'));
        assert_eq!(cue.start_ms, 0);
        assert!(cue.end_ms >= 1);
    }

    struct MockSubscriber {
        frames: Vec<Option<Arc<AVFrame>>>,
        closed: bool,
    }

    #[async_trait]
    impl SubscriberSource for MockSubscriber {
        async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, SdkError> {
            Ok(self.frames.remove(0))
        }

        async fn close(&mut self) -> Result<(), SdkError> {
            self.closed = true;
            Ok(())
        }

        fn id(&self) -> SubscriberId {
            SubscriberId(0)
        }
    }

    struct MockPublisher {
        frames: Arc<std::sync::Mutex<Vec<Arc<AVFrame>>>>,
        tracks: Arc<std::sync::Mutex<Vec<TrackInfo>>>,
        closed: Arc<std::sync::Mutex<bool>>,
        result: DispatchResult,
    }

    impl MockPublisher {
        fn new() -> (
            Self,
            Arc<std::sync::Mutex<Vec<Arc<AVFrame>>>>,
            Arc<std::sync::Mutex<Vec<TrackInfo>>>,
            Arc<std::sync::Mutex<bool>>,
        ) {
            let frames = Arc::new(std::sync::Mutex::new(Vec::new()));
            let tracks = Arc::new(std::sync::Mutex::new(Vec::new()));
            let closed = Arc::new(std::sync::Mutex::new(false));
            let publisher = Self {
                frames: frames.clone(),
                tracks: tracks.clone(),
                closed: closed.clone(),
                result: DispatchResult::Accepted,
            };
            (publisher, frames, tracks, closed)
        }
    }

    impl PublisherSink for MockPublisher {
        fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
            *self.tracks.lock().unwrap() = tracks;
            Ok(())
        }

        fn push_frame(&self, frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError> {
            self.frames.lock().unwrap().push(frame);
            Ok(self.result)
        }

        fn close(&self) -> Result<(), SdkError> {
            *self.closed.lock().unwrap() = true;
            Ok(())
        }

        fn take_keyframe_requests(&self) -> u64 {
            0
        }
    }

    struct BlockingSubscriber;

    #[async_trait]
    impl SubscriberSource for BlockingSubscriber {
        async fn recv(&mut self) -> Result<Option<Arc<AVFrame>>, SdkError> {
            std::future::pending::<Result<Option<Arc<AVFrame>>, SdkError>>().await
        }

        async fn close(&mut self) -> Result<(), SdkError> {
            Ok(())
        }

        fn id(&self) -> SubscriberId {
            SubscriberId(0)
        }
    }

    #[tokio::test]
    async fn run_publishes_cue_on_close() {
        let sei = make_h264_sei_nalu(&pop_on_cc_payload("HI"));
        let mut payload = Bytes::from_static(&[0, 0, 0, 1]);
        payload = Bytes::from([payload.as_ref(), sei.as_ref()].concat());
        let frame = Arc::new(make_video_frame(payload, 0));

        let subscriber = Box::new(MockSubscriber {
            frames: vec![Some(frame), None],
            closed: false,
        });
        let (publisher, frames, tracks, closed) = MockPublisher::new();
        let publisher = Box::new(publisher);

        let mut worker = CaptionExtractWorker::new(CeaParserConfig::default());
        let cancel = CancellationToken::new();
        worker
            .run(subscriber, publisher, cancel)
            .await
            .expect("run should complete cleanly");

        assert_eq!(tracks.lock().unwrap().len(), 1);
        assert_eq!(tracks.lock().unwrap()[0].codec, CodecId::WebVtt);
        assert_eq!(frames.lock().unwrap().len(), 1);
        let frame = &frames.lock().unwrap()[0];
        assert_eq!(frame.media_kind, MediaKind::Subtitle);
        assert_eq!(frame.codec, CodecId::WebVtt);
        let cue: WebVttCue = serde_json::from_slice(&frame.payload).expect("valid json cue");
        assert!(cue.payload.contains('H'));
        assert!(*closed.lock().unwrap(), "publisher close should be called");
    }

    #[tokio::test]
    async fn cancellation_stops_worker_and_releases_publisher() {
        let subscriber = Box::new(BlockingSubscriber);
        let (publisher, frames, _tracks, closed) = MockPublisher::new();
        let publisher = Box::new(publisher);

        let mut worker = CaptionExtractWorker::new(CeaParserConfig::default());
        let cancel = CancellationToken::new();
        let child = cancel.child_token();

        let handle = tokio::spawn(async move { worker.run(subscriber, publisher, child).await });

        // Give the worker a chance to enter the receive loop.
        tokio::task::yield_now().await;
        cancel.cancel();

        let result = handle.await.expect("worker task should complete");
        assert!(result.is_ok());
        assert!(frames.lock().unwrap().is_empty());
        assert!(
            *closed.lock().unwrap(),
            "publisher close should be called on cancel"
        );
    }

    #[tokio::test]
    async fn push_cue_returns_error_when_publisher_rejects() {
        let worker = CaptionExtractWorker::new(CeaParserConfig::default());
        let (mut publisher, _frames, _tracks, _closed) = MockPublisher::new();
        publisher.result = DispatchResult::RejectedClosed;

        let cue = WebVttCue {
            id: None,
            start_ms: 0,
            end_ms: 1000,
            payload: "HI".to_string(),
            settings: None,
        };
        assert!(worker.push_cue(&publisher, cue).is_err());
    }
}
