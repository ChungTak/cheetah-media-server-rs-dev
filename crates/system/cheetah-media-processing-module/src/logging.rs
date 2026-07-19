//! Structured, sanitized logging helpers for media processing jobs.

use cheetah_media_api::processing::ProcessingJob;
use tracing::info;

use crate::spec_labels::{job_kind_label, job_media_codec};

/// Emit a structured lifecycle log for a processing job.
///
/// Includes job id, kind, owner, generation, profile, sanitized source/target
/// keys, media/codec summary, dimensions, counters, pending/drops, refcount,
/// terminal state and error.  Does not log raw payloads, credentials, font data,
/// or full StreamKeys.
pub(crate) fn log_job_lifecycle(job: &ProcessingJob, stage: &'static str, latency_ms: Option<i64>) {
    let (media, codec) = job_media_codec(&job.spec);
    let dimensions = crate::spec_labels::job_dimensions(&job.spec);
    let source = job
        .input_keys
        .first()
        .map(|k| k.to_string())
        .unwrap_or_default();
    let target = job
        .output_keys
        .first()
        .map(|k| k.to_string())
        .unwrap_or_default();
    let owner = job.owner.as_deref().unwrap_or("none");
    let error = job.last_error.as_deref().unwrap_or("");

    info!(
        job_id = %job.job_id,
        kind = job_kind_label(&job.spec),
        owner = %owner,
        generation = job.generation,
        profile = %job.profile,
        media,
        codec,
        dimensions = %dimensions,
        source = %source,
        target = %target,
        stage,
        latency_ms,
        state = ?job.state,
        frames_in = job.frames_in,
        frames_out = job.frames_out,
        bytes_in = job.bytes_in,
        bytes_out = job.bytes_out,
        drops = job.drops,
        pending = job.pending,
        flushes = job.flushes,
        resets = job.resets,
        ref_count = job.ref_count,
        restart_count = job.restart_count,
        error = %error,
        "processing job {stage}"
    );
}
