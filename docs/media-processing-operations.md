# Media Processing Operations Guide

This guide covers day-to-day operation of the optional `cheetah-media-processing-module`
backed by `avcodec-rs`. It assumes the module is enabled; without the relevant Cargo
features the endpoints return `Unavailable`.

The HTTP routes used below are provided by the native media HTTP module
(`cheetah-media-module`) under its `/api/v1` prefix; the media-processing module
itself only registers a `BackgroundJob` capability and does not expose HTTP routes.

## Capability and preflight

Query what the running instance can do before creating jobs:

```bash
curl -s http://localhost:8080/api/v1/processing/preflight | jq .
```

Response fields:

- `revision`: pinned `avcodec` git revision.
- `profile`: active profile (`native-free` or `software`).
- `operations`: list of operations with `available`, `kind`, `codec`, `reason`.

An operation with `available: false` and a `reason` such as
`NoCandidateBackendMatched` means the requested codec/operator is not compiled or
not supported by the active profile.

## Creating and stopping a job

Create a single-stream transcode:

```bash
curl -s -X POST http://localhost:8080/api/v1/processing/jobs \
  -H 'Content-Type: application/json' \
  -d '{
    "spec": {
      "kind": "transcode",
      "source": {"app": "live", "stream": "input"},
      "target": {"app": "live", "stream": "output"},
      "track_selection": "all",
      "video": {"codec": "h264"},
      "audio": {"codec": "aac", "sample_rate": 44100, "channels": 2}
    }
  }' | jq .
```

Use an explicit target `StreamKey`; do not publish to the reserved
`__cheetah_derived` namespace from outside the engine. Internal auto-derived jobs
use that namespace together with a normalized spec fingerprint so they can be
shared.

Stop a job (idempotent):

```bash
curl -s -X POST "http://localhost:8080/api/v1/processing/jobs/${JOB_ID}/stop"
```

Delete removes the record after the job is terminal:

```bash
curl -s -X DELETE "http://localhost:8080/api/v1/processing/jobs/${JOB_ID}"
```

## Locating `Unsupported` errors

A `create_job` or `update_job` call may fail with `Unsupported`. Common causes:

1. The feature is not compiled (e.g. `media-processing-video` not enabled).
2. The active profile does not include the backend (e.g. `native-free` excludes
   FFmpeg and therefore MP3 decode/encode).
3. The requested codec/operator is in scope but no backend reported support for
   the exact sample rate, channel count, pixel format, or memory domain.

Check `MediaProcessingApi::preflight` first. If preflight says an operation is
available but `create_job` still returns `Unsupported`, inspect the job
`last_error` for the backend selection trace:

```bash
curl -s "http://localhost:8080/api/v1/processing/jobs/${JOB_ID}" | jq '.last_error'
```

## Observing queues and drops

Prometheus metrics are published by `MediaProcessingProvider`:

- `media_processing_jobs{kind, state, profile}` — job count by state.
- `media_processing_frames_total{direction, media, codec}` — `ingress`/`egress` frames.
- `media_processing_bytes_total{direction, media, codec}` — `ingress`/`egress` bytes.
- `media_processing_drops_total{reason, media}` — dropped frames/packets.
- `media_processing_pending_total{stage}` — `input`/`output` pending count.
- `media_processing_queue_depth{stage}` — input/output queue high watermark.
- `media_processing_latency_ms{stage}` — `startup`, `first_output`, `drain`.
- `media_processing_preflight{profile, operation}` — available capability probes.
- `media_processing_restarts_total{reason}` — module/job restarts.
- `media_processing_resource_reserved{kind}` — reserved slots/semaphores.
- `media_processing_shared_refs` — shared derived-job reference count.

Labels never contain the job id, full `StreamKey`, or other high-cardinality
identifiers.

Structured job logs are emitted at creation, startup, first output, drain, and
terminal state. Search for `processing job` and filter by `job_id` or `kind`.
The logs include counters and a stable `error` string when the job fails.

## Checking the dynamic library / SBOM boundary

Cheetah only links the top-level `avcodec` crate directly. Verify the boundary
with:

```bash
cargo tree -p cheetah-media-processing-module --features media-processing-cpu \
  | grep -E 'avcodec|ffmpeg|libyuv|jpeg|zune|opencv|fdk'
```

Only `avcodec` itself should appear as a direct dependency; backend crates must
be transitive through `avcodec` and must not be referenced by Cheetah crates.
`dev-scripts/check_runtime_boundaries.sh` enforces that the module's production
manifest contains no direct `tokio` or `ffmpeg` dependency.

## Safe shutdown and leak checks

Before shutting down the engine:

1. Stop or delete all externally created jobs.
2. Drain protocol-derived jobs by stopping the source publishers/subscribers
   that triggered them.
3. Verify `ResourceLeakReport` is clean using the engine's internal diagnostic
   surface or by confirming the processing metrics have all returned to zero.
   A clean report has no `active_processing_job_ids`, `active_stream_keys`,
   `active_task_ids`, `running_module_ids`, or `active_session_ids` related to
   media processing.
4. Shut down the process with `SIGTERM` (or through your process manager).

`ModuleManagerApi::restart_module` for `media-processing` can be used to apply a
`ModuleRestartRequired` config change; it stops the old provider, releases
workers and leases, and starts a fresh instance.
