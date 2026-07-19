//! Caption extraction and transcoding job provider, plus the caption worker.
//!
//! Implements `MediaProcessingApi` for `CaptionExtract` and, when compiled with
//! `media-processing-cpu`, `Transcode` jobs.

use std::collections::{HashMap, HashSet};
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
#[cfg(feature = "media-processing-cpu")]
use cheetah_media_api::processing::{
    AbrVariant, AudioMix, AudioMixInput, MosaicLayout, TrackSelection, VideoMosaicInput,
};
use cheetah_media_api::processing::{Overlay, OverlayKind};
use cheetah_media_api::{
    auth::MediaScope,
    error::{MediaError, MediaErrorCode, Result as MediaResult},
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
    canonical_hash, BackpressurePolicy, BootstrapPolicy, CancellationToken, Deadline,
    DispatchResult, EngineContext, IdempotencyError, IdempotencyKey, MediaFilter, PublisherOptions,
    PublisherSink, SdkError, StreamKey, SubscriberOptions, SubscriberSource,
};
use futures::FutureExt;
use tracing::{info, warn};

use crate::config::MediaProcessingModuleConfig;

#[cfg(feature = "media-processing-cpu")]
use crate::provider::transcode::spawn_transcode_worker;

#[cfg(feature = "media-processing-cpu")]
use crate::provider::abr::spawn_abr_ladder_worker;

#[cfg(feature = "media-processing-cpu")]
use crate::provider::mix::spawn_audio_mix_worker;

#[cfg(feature = "media-processing-cpu")]
use crate::provider::mosaic::spawn_video_mosaic_worker;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Reserved namespace for internally-derived streams. External API callers may not
/// publish or create processing jobs targeting this namespace.
const RESERVED_DERIVED_NAMESPACE: &str = "__cheetah_derived";

/// In-memory handle for a running or stopped processing job.
struct JobEntry {
    job: Arc<Mutex<ProcessingJob>>,
    cancel: CancellationToken,
    handle: Option<Box<dyn cheetah_sdk::JoinHandle>>,
}

/// `MediaProcessingApi` provider for caption extraction and stream transcoding jobs.
pub struct MediaProcessingProvider {
    ctx: EngineContext,
    config: MediaProcessingModuleConfig,
    jobs: Arc<Mutex<HashMap<ProcessingJobId, JobEntry>>>,
    id_counter: AtomicU64,
    gauge_keys: Arc<Mutex<HashSet<String>>>,
    last_metric_counts: Arc<Mutex<HashMap<ProcessingJobId, MetricSnapshot>>>,
}

#[derive(Clone, Copy, Default)]
struct MetricSnapshot {
    frames_in: u64,
    frames_out: u64,
    bytes_in: u64,
    bytes_out: u64,
    drops: u64,
    restarts: u64,
}

impl MediaProcessingProvider {
    pub fn new(ctx: EngineContext, config: MediaProcessingModuleConfig) -> Self {
        Self {
            ctx,
            config,
            jobs: Arc::new(Mutex::new(HashMap::new())),
            id_counter: AtomicU64::new(0),
            gauge_keys: Arc::new(Mutex::new(HashSet::new())),
            last_metric_counts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn job_kind_label(spec: &ProcessingJobSpec) -> &'static str {
        match spec {
            ProcessingJobSpec::CaptionExtract { .. } => "caption",
            #[cfg(feature = "media-processing-cpu")]
            ProcessingJobSpec::Transcode { .. } => "transcode",
            #[cfg(feature = "media-processing-cpu")]
            ProcessingJobSpec::AbrLadder { .. } => "abr",
            #[cfg(feature = "media-processing-cpu")]
            ProcessingJobSpec::AudioMix { .. } => "mix",
            #[cfg(feature = "media-processing-cpu")]
            ProcessingJobSpec::VideoMosaic { .. } => "mosaic",
            #[cfg(not(feature = "media-processing-cpu"))]
            _ => "unknown",
        }
    }

    fn job_primary_media_and_codec(spec: &ProcessingJobSpec) -> (&'static str, String) {
        match spec {
            ProcessingJobSpec::Transcode { video, audio, .. } => match (video, audio) {
                (Some(_), Some(_)) => ("mixed", "mixed".to_string()),
                (Some(v), None) => ("video", format!("{0:?}", v.codec).to_lowercase()),
                (None, Some(a)) => ("audio", format!("{0:?}", a.codec).to_lowercase()),
                (None, None) => ("none", "none".to_string()),
            },
            ProcessingJobSpec::AbrLadder { variants, .. } => {
                if variants.is_empty() {
                    ("video", "none".to_string())
                } else if variants.iter().any(|v| v.audio.is_some()) {
                    ("mixed", "mixed".to_string())
                } else {
                    let mut codecs: HashSet<String> = HashSet::new();
                    for v in variants {
                        codecs.insert(format!("{0:?}", v.video.codec).to_lowercase());
                    }
                    (
                        "video",
                        if codecs.len() == 1 {
                            codecs.into_iter().next().unwrap()
                        } else {
                            "mixed".to_string()
                        },
                    )
                }
            }
            ProcessingJobSpec::AudioMix { output, .. } => {
                ("audio", format!("{0:?}", output.codec).to_lowercase())
            }
            ProcessingJobSpec::VideoMosaic { layout, .. } => (
                "video",
                format!(
                    "{0:?}",
                    layout
                        .video_codec
                        .unwrap_or(cheetah_media_api::processing::VideoCodec::H264)
                )
                .to_lowercase(),
            ),
            ProcessingJobSpec::CaptionExtract { .. } => ("video", "unknown".to_string()),
        }
    }

    /// Publish processing gauges/counters and zero out stale gauge keys.
    ///
    /// Counters (`*_total`) are incremented by the delta since the last publish
    /// so they stay monotonic. Gauges are overwritten and stale keys are zeroed.
    pub fn publish_job_metrics(&self) {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let active_ids: HashSet<ProcessingJobId> = jobs.keys().cloned().collect();
        let mut gauge_values: HashMap<String, u64> = HashMap::new();
        let mut counter_deltas: HashMap<String, u64> = HashMap::new();
        let mut current_counts: HashMap<ProcessingJobId, MetricSnapshot> = HashMap::new();
        let mut shared_refs: u64 = 0;
        let mut reserved_publishers: u64 = 0;
        let mut reserved_subscribers: u64 = 0;

        for entry in jobs.values() {
            let guard = entry.job.lock().unwrap_or_else(|e| e.into_inner());
            let job_id = guard.job_id.clone();
            let kind = Self::job_kind_label(&guard.spec);
            let (media, codec) = Self::job_primary_media_and_codec(&guard.spec);
            let state = format!("{0:?}", guard.state).to_lowercase();

            let job_key = format!(
                "media_processing_jobs{{kind={kind},state={state},profile={}}}",
                guard.profile
            );
            *gauge_values.entry(job_key).or_insert(0) += 1;

            let prev = self
                .last_metric_counts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&job_id)
                .copied()
                .unwrap_or_default();
            let frames_in_delta = guard.frames_in.saturating_sub(prev.frames_in);
            let frames_out_delta = guard.frames_out.saturating_sub(prev.frames_out);
            let bytes_in_delta = guard.bytes_in.saturating_sub(prev.bytes_in);
            let bytes_out_delta = guard.bytes_out.saturating_sub(prev.bytes_out);
            let drops_delta = guard.drops.saturating_sub(prev.drops);
            let restarts_delta = (guard.restart_count as u64).saturating_sub(prev.restarts);

            *counter_deltas
                .entry(format!(
                    "media_processing_frames_total{{direction=ingress,media={media},codec={codec}}}"
                ))
                .or_insert(0) += frames_in_delta;
            *counter_deltas
                .entry(format!(
                    "media_processing_frames_total{{direction=egress,media={media},codec={codec}}}"
                ))
                .or_insert(0) += frames_out_delta;
            *counter_deltas
                .entry(format!(
                    "media_processing_bytes_total{{direction=ingress,media={media},codec={codec}}}"
                ))
                .or_insert(0) += bytes_in_delta;
            *counter_deltas
                .entry(format!(
                    "media_processing_bytes_total{{direction=egress,media={media},codec={codec}}}"
                ))
                .or_insert(0) += bytes_out_delta;
            *counter_deltas
                .entry(format!(
                    "media_processing_drops_total{{reason=policy,media={media}}}"
                ))
                .or_insert(0) += drops_delta;
            *counter_deltas
                .entry("media_processing_restarts_total{reason=failure}".to_string())
                .or_insert(0) += restarts_delta;

            *gauge_values
                .entry("media_processing_pending_total{stage=frame}".to_string())
                .or_insert(0) += guard.pending;
            *gauge_values
                .entry("media_processing_queue_depth{stage=frame}".to_string())
                .or_insert(0) += guard.pending;

            shared_refs += guard.ref_count;
            reserved_publishers += guard.output_keys.len() as u64;
            reserved_subscribers += guard.input_keys.len() as u64;

            current_counts.insert(
                job_id,
                MetricSnapshot {
                    frames_in: guard.frames_in,
                    frames_out: guard.frames_out,
                    bytes_in: guard.bytes_in,
                    bytes_out: guard.bytes_out,
                    drops: guard.drops,
                    restarts: guard.restart_count as u64,
                },
            );

            let created = guard.created_at;
            let mut record_latency = |stage: &'static str, at: i64| {
                let lat = (at.saturating_sub(created)).max(0) as u64;
                let key = format!(
                    "media_processing_latency_ms{{stage={stage},kind={kind},profile={}}}",
                    guard.profile
                );
                gauge_values
                    .entry(key)
                    .and_modify(|v| *v = (*v).max(lat))
                    .or_insert(lat);
            };
            if let Some(started) = guard.started_at {
                record_latency("startup", started);
            }
            if let Some(first) = guard.first_output_at {
                record_latency("first_output", first);
            }
            if let Some(finished) = guard.finished_at {
                record_latency("drain", finished);
            }
        }

        gauge_values.insert("media_processing_shared_refs".to_string(), shared_refs);
        gauge_values.insert(
            "media_processing_resource_reserved{kind=publisher}".to_string(),
            reserved_publishers,
        );
        gauge_values.insert(
            "media_processing_resource_reserved{kind=subscriber}".to_string(),
            reserved_subscribers,
        );

        for (key, delta) in counter_deltas {
            if delta > 0 {
                self.ctx.metrics_api.inc(&key, delta);
            }
        }

        let mut emitted = self.gauge_keys.lock().unwrap_or_else(|e| e.into_inner());
        let new_keys: HashSet<String> = gauge_values.keys().cloned().collect();
        for (key, value) in gauge_values {
            self.ctx.metrics_api.set(&key, value);
            emitted.insert(key);
        }
        for stale in emitted.difference(&new_keys) {
            self.ctx.metrics_api.set(stale, 0);
        }
        *emitted = new_keys;

        let mut last_counts = self
            .last_metric_counts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        last_counts.retain(|id, _| active_ids.contains(id));
        for (id, snap) in current_counts {
            last_counts.insert(id, snap);
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
                        protocol: "media-processing".to_string(),
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

    fn owner_from_ctx(ctx: &MediaRequestContext) -> Option<String> {
        ctx.principal.as_ref().map(|p| p.identity.clone())
    }

    /// Returns true if the request principal is the job owner, has server admin
    /// scope, or holds a `MediaControl` resource grant for one of the job's keys.
    fn job_accessible(&self, job: &ProcessingJob, ctx: &MediaRequestContext) -> bool {
        match (&job.owner, &ctx.principal) {
            (None, None) => true,
            (Some(owner), Some(principal)) if owner == &principal.identity => true,
            (_, Some(principal)) if principal.has_scope(&MediaScope::ServerAdmin) => true,
            (_, Some(principal)) => {
                let scope = &MediaScope::MediaControl;
                job.input_keys
                    .iter()
                    .chain(&job.output_keys)
                    .any(|key| principal.authorizes(scope, Some(key)))
            }
            _ => false,
        }
    }

    /// Reject external job requests that target the reserved derived-stream
    /// namespace. Internal/auto tasks are expected to use the provider directly.
    fn validate_no_reserved_targets(spec: &ProcessingJobSpec) -> MediaResult<()> {
        let targets: Vec<&MediaKey> = match spec {
            ProcessingJobSpec::CaptionExtract { target, .. }
            | ProcessingJobSpec::Transcode { target, .. }
            | ProcessingJobSpec::AudioMix { target, .. }
            | ProcessingJobSpec::VideoMosaic { target, .. } => vec![target],
            ProcessingJobSpec::AbrLadder { variants, .. } => {
                variants.iter().map(|v| &v.target).collect()
            }
        };
        for target in targets {
            let (namespace, _) = StreamKeyBridge::to_namespace_path(target);
            if namespace == RESERVED_DERIVED_NAMESPACE {
                return Err(MediaError::invalid_argument(format!(
                    "processing job target uses reserved namespace '{RESERVED_DERIVED_NAMESPACE}'"
                )));
            }
        }
        Ok(())
    }

    /// Authorize the principal to play all source streams and publish all target
    /// streams before any job slot, subscriber, publisher, or worker is allocated.
    async fn authorize_create(
        &self,
        ctx: &MediaRequestContext,
        spec: &ProcessingJobSpec,
    ) -> MediaResult<()> {
        let (sources, targets) = match spec {
            ProcessingJobSpec::CaptionExtract { source, target, .. }
            | ProcessingJobSpec::Transcode { source, target, .. } => {
                (vec![source.clone()], vec![target.clone()])
            }
            ProcessingJobSpec::AbrLadder { source, variants } => {
                let targets = variants.iter().map(|v| v.target.clone()).collect();
                (vec![source.clone()], targets)
            }
            ProcessingJobSpec::AudioMix { inputs, target, .. } => {
                let sources = inputs.iter().map(|i| i.source.clone()).collect();
                (sources, vec![target.clone()])
            }
            ProcessingJobSpec::VideoMosaic { inputs, target, .. } => {
                let sources = inputs.iter().map(|i| i.source.clone()).collect();
                (sources, vec![target.clone()])
            }
        };
        for source in sources {
            self.authorize(ctx, AdmissionAction::Play, &source).await?;
        }
        for target in targets {
            self.authorize(ctx, AdmissionAction::Publish, &target)
                .await?;
        }
        Ok(())
    }

    /// Validate that a processing job spec respects configured upper bounds.
    fn validate_spec(&self, spec: &ProcessingJobSpec) -> MediaResult<()> {
        let cfg = &self.config;
        match spec {
            ProcessingJobSpec::CaptionExtract { caption, .. } => {
                if caption.source_streams.len() > cfg.max_processing_inputs as usize {
                    return Err(MediaError::invalid_argument(
                        "caption source_streams exceed max_processing_inputs".to_string(),
                    ));
                }
                if caption.languages.len() > cfg.max_processing_inputs as usize {
                    return Err(MediaError::invalid_argument(
                        "caption languages exceed max_processing_inputs".to_string(),
                    ));
                }
            }
            ProcessingJobSpec::Transcode {
                video, overlays, ..
            } => {
                if let Some(video) = video {
                    self.validate_video_target(video)?;
                }
                self.validate_overlays(overlays)?;
            }
            ProcessingJobSpec::AbrLadder { variants, .. } => {
                for variant in variants {
                    self.validate_video_target(&variant.video)?;
                }
            }
            ProcessingJobSpec::AudioMix { inputs, .. } => {
                if inputs.len() > cfg.max_processing_inputs as usize {
                    return Err(MediaError::invalid_argument(format!(
                        "audio mix inputs exceed max_processing_inputs ({})",
                        cfg.max_processing_inputs
                    )));
                }
            }
            ProcessingJobSpec::VideoMosaic {
                inputs,
                layout,
                overlays,
                ..
            } => {
                if inputs.len() > cfg.max_processing_inputs as usize {
                    return Err(MediaError::invalid_argument(format!(
                        "video mosaic inputs exceed max_processing_inputs ({})",
                        cfg.max_processing_inputs
                    )));
                }
                self.validate_mosaic_layout(layout)?;
                self.validate_overlays(overlays)?;
            }
        }
        Ok(())
    }

    fn validate_video_target(
        &self,
        video: &cheetah_media_api::processing::VideoTarget,
    ) -> MediaResult<()> {
        let cfg = &self.config;
        if let (Some(width), Some(height)) = (video.width, video.height) {
            if width > cfg.max_image_width || height > cfg.max_image_height {
                return Err(MediaError::invalid_argument(format!(
                    "video target {width}x{height} exceeds configured limit {}x{}",
                    cfg.max_image_width, cfg.max_image_height
                )));
            }
            if let (Some(num), Some(den)) = (video.frame_rate_num, video.frame_rate_den) {
                if den == 0 {
                    return Err(MediaError::invalid_argument(
                        "video target frame_rate_den must be non-zero".to_string(),
                    ));
                }
                let pixel_rate = (width as u128) * (height as u128) * (num as u128) / (den as u128);
                if pixel_rate > cfg.max_video_pixel_rate as u128 {
                    return Err(MediaError::invalid_argument(format!(
                        "video target pixel rate {pixel_rate} exceeds configured limit {}",
                        cfg.max_video_pixel_rate
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_mosaic_layout(
        &self,
        layout: &cheetah_media_api::processing::MosaicLayout,
    ) -> MediaResult<()> {
        let cfg = &self.config;
        let width = (layout.columns as u128) * (layout.cell_width as u128);
        let height = (layout.rows as u128) * (layout.cell_height as u128);
        if width > cfg.max_image_width as u128 || height > cfg.max_image_height as u128 {
            return Err(MediaError::invalid_argument(format!(
                "mosaic output {width}x{height} exceeds configured limit {}x{}",
                cfg.max_image_width, cfg.max_image_height
            )));
        }
        if let (Some(num), Some(den)) = (layout.frame_rate_num, layout.frame_rate_den) {
            if den == 0 {
                return Err(MediaError::invalid_argument(
                    "mosaic frame_rate_den must be non-zero".to_string(),
                ));
            }
            let pixel_rate = width * height * (num as u128) / (den as u128);
            if pixel_rate > cfg.max_video_pixel_rate as u128 {
                return Err(MediaError::invalid_argument(format!(
                    "mosaic pixel rate {pixel_rate} exceeds configured limit {}",
                    cfg.max_video_pixel_rate
                )));
            }
        }
        Ok(())
    }

    fn validate_overlays(&self, overlays: &[Overlay]) -> MediaResult<()> {
        let cfg = &self.config;
        if overlays.len() > cfg.max_processing_overlays as usize {
            return Err(MediaError::invalid_argument(format!(
                "overlays exceed max_processing_overlays ({})",
                cfg.max_processing_overlays
            )));
        }
        for overlay in overlays {
            if let OverlayKind::Text { text, .. } = &overlay.kind {
                if text.chars().count() > cfg.max_overlay_text_length as usize {
                    return Err(MediaError::invalid_argument(format!(
                        "overlay text length exceeds max_overlay_text_length ({})",
                        cfg.max_overlay_text_length
                    )));
                }
            }
        }
        Ok(())
    }

    fn new_job_id(&self) -> ProcessingJobId {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let n = self.id_counter.fetch_add(1, Ordering::Relaxed);
        ProcessingJobId(format!("job-{ts}-{n}"))
    }

    fn reserve_job_slot(
        &self,
        request: &CreateProcessingJob,
        owner: Option<String>,
    ) -> MediaResult<(
        ProcessingJobId,
        Arc<Mutex<ProcessingJob>>,
        CancellationToken,
    )> {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let running = jobs
            .values()
            .filter(|e| {
                e.job.lock().unwrap_or_else(|e| e.into_inner()).state == ProcessingJobState::Running
            })
            .count();
        if running >= self.config.max_concurrent_jobs as usize {
            return Err(MediaError::unavailable(format!(
                "max concurrent processing jobs ({}) reached",
                self.config.max_concurrent_jobs
            )));
        }

        let job = Arc::new(Mutex::new(self.build_job(
            request,
            ProcessingJobState::Running,
            owner,
        )));
        let cancel = CancellationToken::new();
        let job_id = job.lock().unwrap_or_else(|e| e.into_inner()).job_id.clone();
        jobs.insert(
            job_id.clone(),
            JobEntry {
                job: job.clone(),
                cancel: cancel.clone(),
                handle: None,
            },
        );
        Ok((job_id, job, cancel))
    }

    fn fail_reserved_job(
        &self,
        job_id: &ProcessingJobId,
        job: Arc<Mutex<ProcessingJob>>,
        error: &MediaError,
    ) {
        let mut guard = job.lock().unwrap_or_else(|e| e.into_inner());
        let now = now_ms();
        guard.state = ProcessingJobState::Failed;
        guard.last_error = Some(error.to_string());
        guard.finished_at = Some(now);
        guard.updated_at = now;
        drop(guard);
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(job_id);
    }

    pub fn default_capabilities() -> MediaCapabilitySet {
        let mut set = MediaCapabilitySet::empty();
        set.add(MediaCapability::VideoProcessing, 1);
        set.set_reason(
            MediaCapability::VideoProcessing,
            "caption extraction / video transcode / abr ladder",
        );
        #[cfg(feature = "media-processing-cpu")]
        {
            set.add(MediaCapability::AudioProcessing, 1);
            set.set_reason(
                MediaCapability::AudioProcessing,
                "audio transcode / audio mix",
            );
        }
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

    fn build_job(
        &self,
        request: &CreateProcessingJob,
        state: ProcessingJobState,
        owner: Option<String>,
    ) -> ProcessingJob {
        let now = now_ms();
        ProcessingJob {
            job_id: self.new_job_id(),
            spec: request.spec.clone(),
            state,
            generation: 1,
            profile: self.config.profile.clone(),
            created_at: now,
            updated_at: now,
            started_at: None,
            first_output_at: None,
            finished_at: None,
            input_keys: match &request.spec {
                ProcessingJobSpec::CaptionExtract { source, .. }
                | ProcessingJobSpec::Transcode { source, .. }
                | ProcessingJobSpec::AbrLadder { source, .. } => vec![source.clone()],
                ProcessingJobSpec::AudioMix { inputs, .. } => {
                    inputs.iter().map(|i| i.source.clone()).collect()
                }
                ProcessingJobSpec::VideoMosaic { inputs, .. } => {
                    inputs.iter().map(|i| i.source.clone()).collect()
                }
            },
            output_keys: match &request.spec {
                ProcessingJobSpec::CaptionExtract { target, .. }
                | ProcessingJobSpec::Transcode { target, .. } => vec![target.clone()],
                ProcessingJobSpec::AbrLadder { variants, .. } => {
                    variants.iter().map(|v| v.target.clone()).collect()
                }
                ProcessingJobSpec::AudioMix { target, .. } => vec![target.clone()],
                ProcessingJobSpec::VideoMosaic { target, .. } => vec![target.clone()],
            },
            owner,
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
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            for entry in jobs.values_mut() {
                entry.cancel.cancel();
                let mut guard = entry.job.lock().unwrap_or_else(|e| e.into_inner());
                guard.state = ProcessingJobState::Stopped;
                guard.updated_at = now_ms();
            }
            jobs.values_mut()
                .filter_map(|e| e.handle.take())
                .collect::<Vec<_>>()
        };
        let _ = futures::future::join_all(handles.into_iter().map(|h| h.wait())).await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_caption_job(
        &self,
        _ctx: &MediaRequestContext,
        request: CreateProcessingJob,
        source: &MediaKey,
        target: &MediaKey,
        job_id: &ProcessingJobId,
        job: Arc<Mutex<ProcessingJob>>,
        cancel: CancellationToken,
    ) -> MediaResult<ProcessingJob> {
        let _ = request;
        let source_key = Self::media_key_to_stream_key(source);
        let target_key = Self::media_key_to_stream_key(target);

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

        let cancel_child = cancel.child_token();
        let runtime = self.ctx.runtime_api.clone();

        let job_snapshot = job.lock().unwrap_or_else(|e| e.into_inner()).clone();

        let publisher_api = self.ctx.publisher_api.clone();
        let spawned_job_id = job_id.clone();
        let handle = runtime.spawn(Box::pin(async move {
            let result = worker
                .run(subscriber, publisher, cancel_child, Some(job.clone()))
                .await;
            if let Err(e) = result {
                warn!(job_id = %spawned_job_id, "caption extract worker failed: {e}");
            }
            let _ = publisher_api.release_publisher(&lease).await;
        }));

        if let Some(entry) = self
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(job_id)
        {
            entry.cancel = cancel;
            entry.handle = Some(handle);
        }

        info!(job_id = %job_id, "caption extract job started");
        Ok(job_snapshot)
    }

    #[cfg(feature = "media-processing-cpu")]
    #[allow(clippy::too_many_arguments)]
    async fn create_transcode_job(
        &self,
        _ctx: &MediaRequestContext,
        request: CreateProcessingJob,
        source: &MediaKey,
        target: &MediaKey,
        track_selection: TrackSelection,
        video: &Option<cheetah_media_api::processing::VideoTarget>,
        audio: &Option<cheetah_media_api::processing::AudioTarget>,
        job_id: &ProcessingJobId,
        job: Arc<Mutex<ProcessingJob>>,
        cancel: CancellationToken,
    ) -> MediaResult<ProcessingJob> {
        let _ = request;
        let source_key = Self::media_key_to_stream_key(source);
        let target_key = Self::media_key_to_stream_key(target);

        let pub_options = PublisherOptions {
            announce_tracks: true,
            protocol: "transcode".to_string(),
            remote_endpoint: None,
        };
        let (lease, publisher) = self
            .ctx
            .publisher_api
            .acquire_publisher(target_key.clone(), pub_options)
            .await
            .map_err(|e| MediaError::internal(format!("acquire publisher failed: {e}")))?;

        let cancel_child = cancel.child_token();
        let runtime = self.ctx.runtime_api.clone();

        let job_snapshot = job.lock().unwrap_or_else(|e| e.into_inner()).clone();

        let config = self.config.clone();
        let engine = self.ctx.clone();
        let spawned_job_id = job_id.clone();
        let video = video.clone();
        let audio = audio.clone();
        let handle = runtime.spawn(Box::pin(async move {
            let result = spawn_transcode_worker(
                engine,
                config,
                source_key,
                target_key,
                track_selection,
                video,
                audio,
                lease,
                publisher,
                cancel_child,
                Some(job.clone()),
            )
            .await;
            if let Err(e) = result {
                warn!(job_id = %spawned_job_id, "transcode worker failed: {e}");
            }
        }));

        if let Some(entry) = self
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(job_id)
        {
            entry.cancel = cancel;
            entry.handle = Some(handle);
        }

        info!(job_id = %job_id, "transcode job started");
        Ok(job_snapshot)
    }

    #[cfg(feature = "media-processing-cpu")]
    #[allow(clippy::too_many_arguments)]
    async fn create_abr_ladder_job(
        &self,
        _ctx: &MediaRequestContext,
        request: CreateProcessingJob,
        source: &MediaKey,
        variants: &[AbrVariant],
        job_id: &ProcessingJobId,
        job: Arc<Mutex<ProcessingJob>>,
        cancel: CancellationToken,
    ) -> MediaResult<ProcessingJob> {
        let _ = request;
        if variants.is_empty() || variants.len() > 4 {
            return Err(MediaError::invalid_argument(
                "ABR ladder requires 1-4 variants",
            ));
        }

        let source_key = Self::media_key_to_stream_key(source);

        // Detect duplicate target keys.
        let mut seen_targets = std::collections::HashSet::new();
        for variant in variants {
            if !seen_targets.insert(variant.target.clone()) {
                return Err(MediaError::invalid_argument(format!(
                    "duplicate ABR ladder target: {}",
                    variant.target
                )));
            }
        }

        // Acquire all publishers before starting; roll back on any failure.
        let pub_options = PublisherOptions {
            announce_tracks: true,
            protocol: "abr-ladder".to_string(),
            remote_endpoint: None,
        };
        let mut publishers = Vec::with_capacity(variants.len());
        for variant in variants {
            let target_key = Self::media_key_to_stream_key(&variant.target);
            match self
                .ctx
                .publisher_api
                .acquire_publisher(target_key, pub_options.clone())
                .await
            {
                Ok(pair) => publishers.push(pair),
                Err(e) => {
                    for (lease, _publisher) in publishers {
                        let _ = self.ctx.publisher_api.release_publisher(&lease).await;
                    }
                    return Err(MediaError::internal(format!(
                        "acquire publisher for {} failed: {e}",
                        variant.target
                    )));
                }
            }
        }

        let cancel_child = cancel.child_token();
        let runtime = self.ctx.runtime_api.clone();

        let job_snapshot = job.lock().unwrap_or_else(|e| e.into_inner()).clone();

        let config = self.config.clone();
        let engine = self.ctx.clone();
        let spawned_job_id = job_id.clone();
        let variants = variants.to_vec();
        let handle = runtime.spawn(Box::pin(async move {
            let result = spawn_abr_ladder_worker(
                engine,
                config,
                source_key,
                variants,
                publishers,
                cancel_child,
                Some(job.clone()),
            )
            .await;
            if let Err(e) = result {
                warn!(job_id = %spawned_job_id, "abr ladder worker failed: {e}");
            }
        }));

        if let Some(entry) = self
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(job_id)
        {
            entry.cancel = cancel;
            entry.handle = Some(handle);
        }

        info!(job_id = %job_id, "abr ladder job started");
        Ok(job_snapshot)
    }

    #[cfg(feature = "media-processing-cpu")]
    #[allow(clippy::too_many_arguments)]
    async fn create_audio_mix_job(
        &self,
        _ctx: &MediaRequestContext,
        request: CreateProcessingJob,
        inputs: &[AudioMixInput],
        mix: &AudioMix,
        job_id: &ProcessingJobId,
        job: Arc<Mutex<ProcessingJob>>,
        cancel: CancellationToken,
    ) -> MediaResult<ProcessingJob> {
        let _ = request;
        if inputs.len() < 2 || inputs.len() > 16 {
            return Err(MediaError::invalid_argument(
                "audio mix requires 2-16 sources",
            ));
        }

        let target_key = Self::media_key_to_stream_key(&mix.target);
        let pub_options = PublisherOptions {
            announce_tracks: true,
            protocol: "audio-mix".to_string(),
            remote_endpoint: None,
        };
        let (lease, publisher) = self
            .ctx
            .publisher_api
            .acquire_publisher(target_key, pub_options)
            .await
            .map_err(|e| MediaError::internal(format!("acquire publisher failed: {e}")))?;

        let cancel_child = cancel.child_token();
        let runtime = self.ctx.runtime_api.clone();

        let job_snapshot = job.lock().unwrap_or_else(|e| e.into_inner()).clone();

        let config = self.config.clone();
        let engine = self.ctx.clone();
        let spawned_job_id = job_id.clone();
        let inputs = inputs.to_vec();
        let mix = mix.clone();
        let handle = runtime.spawn(Box::pin(async move {
            let result = spawn_audio_mix_worker(
                engine,
                config,
                inputs,
                mix,
                lease,
                publisher,
                cancel_child,
                Some(job.clone()),
            )
            .await;
            if let Err(e) = result {
                warn!(job_id = %spawned_job_id, "audio mix worker failed: {e}");
            }
        }));

        if let Some(entry) = self
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(job_id)
        {
            entry.cancel = cancel;
            entry.handle = Some(handle);
        }

        info!(job_id = %job_id, "audio mix job started");
        Ok(job_snapshot)
    }

    #[cfg(feature = "media-processing-cpu")]
    #[allow(clippy::too_many_arguments)]
    async fn create_video_mosaic_job(
        &self,
        _ctx: &MediaRequestContext,
        request: CreateProcessingJob,
        inputs: &[VideoMosaicInput],
        layout: &MosaicLayout,
        target: &MediaKey,
        audio_mix: &Option<AudioMix>,
        overlays: &[Overlay],
        job_id: &ProcessingJobId,
        job: Arc<Mutex<ProcessingJob>>,
        cancel: CancellationToken,
    ) -> MediaResult<ProcessingJob> {
        let _ = request;
        if inputs.len() < 2 || inputs.len() > 9 {
            return Err(MediaError::invalid_argument(
                "video mosaic requires 2-9 sources",
            ));
        }
        if audio_mix.is_some() {
            return Err(MediaError::unsupported(
                "video mosaic audio mix is not supported in this release",
            ));
        }
        if !overlays.is_empty() {
            return Err(MediaError::unsupported(
                "video mosaic overlays are not supported in this release",
            ));
        }

        let target_key = Self::media_key_to_stream_key(target);
        let pub_options = PublisherOptions {
            announce_tracks: true,
            protocol: "video-mosaic".to_string(),
            remote_endpoint: None,
        };
        let (lease, publisher) = self
            .ctx
            .publisher_api
            .acquire_publisher(target_key, pub_options)
            .await
            .map_err(|e| MediaError::internal(format!("acquire publisher failed: {e}")))?;

        let cancel_child = cancel.child_token();
        let runtime = self.ctx.runtime_api.clone();

        let job_snapshot = job.lock().unwrap_or_else(|e| e.into_inner()).clone();

        let config = self.config.clone();
        let engine = self.ctx.clone();
        let spawned_job_id = job_id.clone();
        let inputs = inputs.to_vec();
        let layout = layout.clone();
        let handle = runtime.spawn(Box::pin(async move {
            let result = spawn_video_mosaic_worker(
                engine,
                config,
                inputs,
                layout,
                lease,
                publisher,
                cancel_child,
                Some(job.clone()),
            )
            .await;
            if let Err(e) = result {
                warn!(job_id = %spawned_job_id, "video mosaic worker failed: {e}");
            }
        }));

        if let Some(entry) = self
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(job_id)
        {
            entry.cancel = cancel;
            entry.handle = Some(handle);
        }

        info!(job_id = %job_id, "video mosaic job started");
        Ok(job_snapshot)
    }

    fn map_idempotency_error(e: IdempotencyError) -> MediaError {
        match e {
            IdempotencyError::Conflict { .. } => {
                MediaError::new(MediaErrorCode::Conflict, e.to_string())
            }
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

    async fn create_job_impl(
        &self,
        ctx: &MediaRequestContext,
        request: CreateProcessingJob,
    ) -> MediaResult<(ProcessingJob, Option<String>)> {
        let deadline = Deadline::from_context(ctx);
        if deadline.is_expired() {
            return Err(MediaError::new(
                MediaErrorCode::Timeout,
                "request deadline exceeded".to_string(),
            ));
        }
        Self::validate_no_reserved_targets(&request.spec)?;
        self.validate_spec(&request.spec)?;
        self.authorize_create(ctx, &request.spec).await?;
        if deadline.is_expired() {
            return Err(MediaError::new(
                MediaErrorCode::Timeout,
                "request deadline exceeded".to_string(),
            ));
        }
        let owner = Self::owner_from_ctx(ctx);
        let (job_id, job, cancel) = self.reserve_job_slot(&request, owner)?;

        let spec = request.spec.clone();
        let result = match &spec {
            ProcessingJobSpec::CaptionExtract { source, target, .. } => {
                self.create_caption_job(
                    ctx,
                    request,
                    source,
                    target,
                    &job_id,
                    job.clone(),
                    cancel.clone(),
                )
                .await
            }
            #[cfg(feature = "media-processing-cpu")]
            ProcessingJobSpec::Transcode {
                source,
                target,
                track_selection,
                video,
                audio,
                overlays,
            } => {
                if !overlays.is_empty() {
                    Err(MediaError::unsupported(
                        "transcode overlays are not supported in this release",
                    ))
                } else {
                    self.create_transcode_job(
                        ctx,
                        request,
                        source,
                        target,
                        *track_selection,
                        video,
                        audio,
                        &job_id,
                        job.clone(),
                        cancel.clone(),
                    )
                    .await
                }
            }
            #[cfg(feature = "media-processing-cpu")]
            ProcessingJobSpec::AbrLadder { source, variants } => {
                self.create_abr_ladder_job(
                    ctx,
                    request,
                    source,
                    variants,
                    &job_id,
                    job.clone(),
                    cancel.clone(),
                )
                .await
            }
            #[cfg(feature = "media-processing-cpu")]
            ProcessingJobSpec::AudioMix {
                inputs,
                target,
                output,
            } => {
                let mix = AudioMix {
                    target: target.clone(),
                    output: output.clone(),
                };
                self.create_audio_mix_job(
                    ctx,
                    request,
                    inputs,
                    &mix,
                    &job_id,
                    job.clone(),
                    cancel.clone(),
                )
                .await
            }
            #[cfg(feature = "media-processing-cpu")]
            ProcessingJobSpec::VideoMosaic {
                inputs,
                target,
                layout,
                audio_mix,
                overlays,
            } => {
                self.create_video_mosaic_job(
                    ctx,
                    request,
                    inputs,
                    layout,
                    target,
                    audio_mix,
                    overlays,
                    &job_id,
                    job.clone(),
                    cancel.clone(),
                )
                .await
            }
            #[cfg(not(feature = "media-processing-cpu"))]
            _ => Err(MediaError::unsupported(
                "processing job type is not compiled in this build",
            )),
        };
        if let Err(ref e) = result {
            self.fail_reserved_job(&job_id, job, e);
        }
        result.map(|job_snapshot| (job_snapshot, Some(job_id.to_string())))
    }
}

#[async_trait]
impl MediaProcessingApi for MediaProcessingProvider {
    async fn preflight(
        &self,
        _ctx: &MediaRequestContext,
    ) -> MediaResult<ProcessingPreflightReport> {
        let mut diagnostics = HashMap::new();
        let mut operations = Vec::new();

        if cfg!(feature = "media-processing-caption") {
            operations.push("caption_extract".to_string());
        } else {
            diagnostics.insert(
                "caption_extract".to_string(),
                "media-processing-caption feature not compiled".to_string(),
            );
        }

        #[cfg(feature = "media-processing-cpu")]
        {
            match crate::provider::avcodec_registry::build_registry(&self.config) {
                Ok(_) => {
                    operations.push("transcode".to_string());
                    operations.push("abr_ladder".to_string());
                    operations.push("audio_mix".to_string());
                    operations.push("video_mosaic".to_string());
                }
                Err(e) => {
                    let reason = format!("avcodec registry unavailable for profile: {e}");
                    for op in ["transcode", "abr_ladder", "audio_mix", "video_mosaic"] {
                        diagnostics.insert(op.to_string(), reason.clone());
                    }
                }
            }
        }

        let profile = self.config.profile.clone();
        let available = !operations.is_empty();

        for op in &operations {
            let key = format!("media_processing_preflight{{profile={profile},operation={op}}}");
            self.ctx.metrics_api.set(&key, 1);
        }
        for op in diagnostics.keys() {
            let key = format!("media_processing_preflight{{profile={profile},operation={op}}}");
            self.ctx.metrics_api.set(&key, 0);
        }

        Ok(ProcessingPreflightReport {
            profile,
            available,
            operations,
            diagnostics,
        })
    }

    async fn create_job(
        &self,
        ctx: &MediaRequestContext,
        request: CreateProcessingJob,
    ) -> MediaResult<ProcessingJob> {
        let idem_key = request
            .idempotency_key
            .as_ref()
            .or(ctx.idempotency_key.as_ref())
            .cloned();
        // Idempotency keys are scoped to a principal; unauthenticated callers
        // must not share a key namespace.
        if let (Some(key), Some(principal)) = (idem_key, ctx.principal.as_ref()) {
            let idem = self.ctx.media_services.idempotency();
            let idem_key =
                IdempotencyKey::new(principal.identity.clone(), "processing.create_job", key);
            let fingerprint = serde_json::to_vec(&(&request.spec, &request.deadline_ms))
                .map(canonical_hash)
                .map_err(|e| {
                    MediaError::internal(format!("idempotency fingerprint failed: {e}"))
                })?;
            // Idempotency retention is independent of the request deadline so retries
            // after a timeout can still be deduplicated.
            let ttl = 3_600_000;
            idem.execute(idem_key, fingerprint, ttl, || async move {
                self.create_job_impl(ctx, request.clone())
                    .await
                    .map_err(|e| {
                        let encoded = serde_json::to_string(&e)
                            .unwrap_or_else(|_| format!("internal: {}", e));
                        if e.retryable
                            || matches!(
                                e.code,
                                MediaErrorCode::Busy
                                    | MediaErrorCode::Timeout
                                    | MediaErrorCode::Unavailable
                            )
                        {
                            IdempotencyError::Retryable(encoded)
                        } else {
                            IdempotencyError::OperationFailed(encoded)
                        }
                    })
            })
            .await
            .map_err(Self::map_idempotency_error)
        } else {
            self.create_job_impl(ctx, request)
                .await
                .map(|(job, _resource_id)| job)
        }
    }

    async fn get_job(
        &self,
        ctx: &MediaRequestContext,
        id: &ProcessingJobId,
    ) -> MediaResult<ProcessingJob> {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let job = jobs
            .get(id)
            .map(|e| e.job.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .ok_or_else(|| MediaError::not_found(format!("job {id} not found")))?;
        if !self.job_accessible(&job, ctx) {
            return Err(MediaError::new(
                MediaErrorCode::PermissionDenied,
                format!("job {id} not accessible"),
            ));
        }
        Ok(job)
    }

    async fn list_jobs(
        &self,
        ctx: &MediaRequestContext,
        mut query: ProcessingJobQuery,
    ) -> MediaResult<Page<ProcessingJob>> {
        query.clamp_page_size();
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let mut items: Vec<ProcessingJob> = jobs
            .values()
            .map(|e| e.job.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .filter(|j| self.job_accessible(j, ctx))
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
            "update is not supported for processing jobs",
        ))
    }

    async fn stop_job(
        &self,
        ctx: &MediaRequestContext,
        id: &ProcessingJobId,
    ) -> MediaResult<ProcessingJob> {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let entry = jobs
            .get_mut(id)
            .ok_or_else(|| MediaError::not_found(format!("job {id} not found")))?;
        if !self.job_accessible(&entry.job.lock().unwrap_or_else(|e| e.into_inner()), ctx) {
            return Err(MediaError::new(
                MediaErrorCode::PermissionDenied,
                format!("job {id} not accessible"),
            ));
        }
        entry.cancel.cancel();
        let mut guard = entry.job.lock().unwrap_or_else(|e| e.into_inner());
        guard.state = ProcessingJobState::Stopped;
        guard.updated_at = now_ms();
        Ok(guard.clone())
    }

    async fn delete_job(&self, ctx: &MediaRequestContext, id: &ProcessingJobId) -> MediaResult<()> {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let entry = jobs
            .get_mut(id)
            .ok_or_else(|| MediaError::not_found(format!("job {id} not found")))?;
        if !self.job_accessible(&entry.job.lock().unwrap_or_else(|e| e.into_inner()), ctx) {
            return Err(MediaError::new(
                MediaErrorCode::PermissionDenied,
                format!("job {id} not accessible"),
            ));
        }
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
        subscriber: Box<dyn SubscriberSource>,
        publisher: Box<dyn PublisherSink>,
        cancel: CancellationToken,
        progress: Option<Arc<Mutex<ProcessingJob>>>,
    ) -> Result<(), SdkError> {
        let result = self
            .run_loop(subscriber, publisher, cancel, &progress)
            .await;
        Self::finish_progress(
            &progress,
            result.as_ref().err().map(|e| format!("{e}")).as_deref(),
        );
        result
    }

    async fn run_loop(
        &mut self,
        mut subscriber: Box<dyn SubscriberSource>,
        publisher: Box<dyn PublisherSink>,
        cancel: CancellationToken,
        progress: &Option<Arc<Mutex<ProcessingJob>>>,
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
                            Self::update_progress(progress, |job| {
                                job.frames_in += 1;
                                job.bytes_in += frame.payload.len() as u64;
                            });
                            for cue in self.process_frame(&frame) {
                                let payload = serde_json::to_vec(&cue)
                                    .map_err(|e| SdkError::Internal(format!("failed to serialize WebVTT cue: {e}")))?;
                                Self::update_progress(progress, |job| {
                                    job.frames_out += 1;
                                    job.bytes_out += payload.len() as u64;
                                });
                                self.push_cue_payload(&*publisher, cue, Bytes::from(payload))?;
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
            let payload = serde_json::to_vec(&cue)
                .map_err(|e| SdkError::Internal(format!("failed to serialize WebVTT cue: {e}")))?;
            Self::update_progress(progress, |job| {
                job.frames_out += 1;
                job.bytes_out += payload.len() as u64;
            });
            self.push_cue_payload(&*publisher, cue, Bytes::from(payload))?;
        }

        publisher.close()
    }

    fn update_progress<F>(progress: &Option<Arc<Mutex<ProcessingJob>>>, f: F)
    where
        F: FnOnce(&mut ProcessingJob),
    {
        if let Some(p) = progress {
            let mut guard = p.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut guard);
            let now = now_ms();
            guard.updated_at = now;
            if guard.started_at.is_none() {
                guard.started_at = Some(now);
            }
            if guard.frames_out > 0 && guard.first_output_at.is_none() {
                guard.first_output_at = Some(now);
            }
        }
    }

    fn finish_progress(progress: &Option<Arc<Mutex<ProcessingJob>>>, last_error: Option<&str>) {
        if let Some(p) = progress {
            let mut guard = p.lock().unwrap_or_else(|e| e.into_inner());
            let finished_at = now_ms();
            guard.finished_at = Some(finished_at);
            if guard.state == ProcessingJobState::Running {
                if let Some(err) = last_error {
                    guard.last_error = Some(err.to_string());
                    guard.state = ProcessingJobState::Failed;
                } else {
                    guard.state = ProcessingJobState::Stopped;
                }
                guard.updated_at = finished_at;
            }
        }
    }

    #[allow(dead_code)]
    fn push_cue(&self, publisher: &dyn PublisherSink, cue: WebVttCue) -> Result<(), SdkError> {
        let payload = serde_json::to_vec(&cue)
            .map_err(|e| SdkError::Internal(format!("failed to serialize WebVTT cue: {e}")))?;
        self.push_cue_payload(publisher, cue, Bytes::from(payload))
    }

    fn push_cue_payload(
        &self,
        publisher: &dyn PublisherSink,
        cue: WebVttCue,
        payload: Bytes,
    ) -> Result<(), SdkError> {
        let duration = (cue.end_ms - cue.start_ms) as i64;
        let mut frame = AVFrame::new(
            TrackId(0),
            MediaKind::Subtitle,
            CodecId::WebVtt,
            FrameFormat::WebVttPacket,
            cue.start_ms as i64,
            cue.start_ms as i64,
            Timebase::new(1, 1000),
            payload,
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
            .run(subscriber, publisher, cancel, None)
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

        let handle =
            tokio::spawn(async move { worker.run(subscriber, publisher, child, None).await });

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

    #[tokio::test]
    async fn push_cue_accepts_dropped_by_policy() {
        let worker = CaptionExtractWorker::new(CeaParserConfig::default());
        let (mut publisher, _frames, _tracks, _closed) = MockPublisher::new();
        publisher.result = DispatchResult::DroppedByPolicy;

        let cue = WebVttCue {
            id: None,
            start_ms: 0,
            end_ms: 1000,
            payload: "HI".to_string(),
            settings: None,
        };
        assert!(worker.push_cue(&publisher, cue).is_ok());
    }

    #[test]
    #[cfg(feature = "media-processing-cpu")]
    fn job_primary_media_and_codec_labels() {
        use cheetah_media_api::processing::{
            AudioCodec, AudioTarget, CaptionConfig, MosaicCell, MosaicLayout, VideoCodec,
            VideoTarget,
        };

        let source = MediaKey::new("__internal", "app", "src", None).unwrap();
        let target = MediaKey::new("__internal", "app", "dst", None).unwrap();

        let transcode_video = ProcessingJobSpec::Transcode {
            source: source.clone(),
            target: target.clone(),
            track_selection: TrackSelection::All,
            audio: None,
            video: Some(VideoTarget {
                codec: VideoCodec::H265,
                width: None,
                height: None,
                frame_rate_num: None,
                frame_rate_den: None,
                bit_rate: None,
                gop_size: None,
                profile: None,
            }),
            overlays: vec![],
        };
        assert_eq!(
            MediaProcessingProvider::job_primary_media_and_codec(&transcode_video),
            ("video", "h265".to_string())
        );

        let transcode_audio = ProcessingJobSpec::Transcode {
            source: source.clone(),
            target: target.clone(),
            track_selection: TrackSelection::All,
            audio: Some(AudioTarget {
                codec: AudioCodec::Opus,
                sample_rate: None,
                channels: None,
                bit_rate: None,
            }),
            video: None,
            overlays: vec![],
        };
        assert_eq!(
            MediaProcessingProvider::job_primary_media_and_codec(&transcode_audio),
            ("audio", "opus".to_string())
        );

        let abr = ProcessingJobSpec::AbrLadder {
            source: source.clone(),
            variants: vec![AbrVariant {
                target: target.clone(),
                video: VideoTarget {
                    codec: VideoCodec::H264,
                    width: None,
                    height: None,
                    frame_rate_num: None,
                    frame_rate_den: None,
                    bit_rate: None,
                    gop_size: None,
                    profile: None,
                },
                audio: None,
            }],
        };
        assert_eq!(
            MediaProcessingProvider::job_primary_media_and_codec(&abr),
            ("video", "h264".to_string())
        );

        let mosaic = ProcessingJobSpec::VideoMosaic {
            inputs: vec![VideoMosaicInput {
                source: source.clone(),
                cell: MosaicCell {
                    column: 0,
                    row: 0,
                    z_order: 0,
                },
                audio_gain_db: None,
                fit: None,
                label: None,
            }],
            target: target.clone(),
            layout: MosaicLayout {
                columns: 1,
                rows: 1,
                cell_width: 64,
                cell_height: 64,
                background: None,
                frame_rate_num: None,
                frame_rate_den: None,
                bit_rate: None,
                gop_size: None,
                video_codec: Some(VideoCodec::H265),
                fit: None,
            },
            audio_mix: None,
            overlays: vec![],
        };
        assert_eq!(
            MediaProcessingProvider::job_primary_media_and_codec(&mosaic),
            ("video", "h265".to_string())
        );

        let mix = ProcessingJobSpec::AudioMix {
            inputs: vec![AudioMixInput {
                source: source.clone(),
                gain_db: None,
            }],
            target: target.clone(),
            output: AudioTarget {
                codec: AudioCodec::Aac,
                sample_rate: None,
                channels: None,
                bit_rate: None,
            },
        };
        assert_eq!(
            MediaProcessingProvider::job_primary_media_and_codec(&mix),
            ("audio", "aac".to_string())
        );

        let caption = ProcessingJobSpec::CaptionExtract {
            source,
            target,
            caption: CaptionConfig {
                source_streams: vec!["src".to_string()],
                languages: vec!["eng".to_string()],
            },
        };
        assert_eq!(
            MediaProcessingProvider::job_primary_media_and_codec(&caption),
            ("video", "unknown".to_string())
        );
    }
}
