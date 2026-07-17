# 03 · 架构与公共契约

## 1. 固定调用链

```text
native HTTP / in-process Rust SDK / protocol module
  -> MediaProcessingApi / ImageProcessApi
  -> MediaServices provider slot
  -> cheetah-media-processing-module
  -> bounded blocking worker
  -> avcodec high-level session
  -> encoded AVFrame + TrackInfo
  -> dedicated derived StreamKey publisher
  -> RTMP / HTTP-FLV / WebRTC / HLS / proxy consumer
```

图片快照不创建派生流，使用 `ImageProcessApi` 返回有界 `ImageArtifact`。所有协议 core 对处理模块完全不可见。

## 2. Crate 与分层

新增 `crates/system/cheetah-media-processing-module`：

- 依赖 `cheetah-sdk`、`cheetah-media-api`、`cheetah-codec`、runtime-neutral API 和可选 `avcodec`。
- 负责 preflight、job registry、AVFrame adapter、worker、派生发布者、处理资源与恢复。
- 不直接依赖 Tokio；阻塞任务、取消、计时和完成通知通过 `RuntimeApi`。
- 不承担 RTMP/HLS/WebRTC 状态机，也不复制时间戳、NALU 和参数集逻辑。

`cheetah-codec` 只新增纯 Sans-I/O CEA parser、WebVTT 规范化和必要的 codec/frame 枚举；删除编解码执行 traits 和 session 逻辑。

## 3. MediaProcessingApi

新增独立 provider slot，不扩张所有 module 必须实现的总 facade：

```rust
#[async_trait]
pub trait MediaProcessingApi: Send + Sync {
    async fn preflight(&self, ctx: &MediaRequestContext)
        -> Result<ProcessingPreflightReport>;
    async fn create_job(&self, ctx: &MediaRequestContext, request: CreateProcessingJob)
        -> Result<ProcessingJob>;
    async fn get_job(&self, ctx: &MediaRequestContext, id: &ProcessingJobId)
        -> Result<ProcessingJob>;
    async fn list_jobs(&self, ctx: &MediaRequestContext, query: ProcessingJobQuery)
        -> Result<Page<ProcessingJob>>;
    async fn update_job(&self, ctx: &MediaRequestContext, request: UpdateProcessingJob)
        -> Result<ProcessingJob>;
    async fn stop_job(&self, ctx: &MediaRequestContext, id: &ProcessingJobId)
        -> Result<ProcessingJob>;
    async fn delete_job(&self, ctx: &MediaRequestContext, id: &ProcessingJobId)
        -> Result<()>;
}
```

缺 provider 返回 `Unavailable`；provider 存在但 profile/codec/op 不支持返回 `Unsupported`；配额不足返回 `ResourceExhausted`。

## 4. Job 类型

`ProcessingJobSpec` 使用 `serde(tag = "kind", rename_all = "snake_case")`：

- `Transcode { source, target, track_selection, audio, video, overlays }`
- `AbrLadder { source, variants }`
- `AudioMix { inputs, target, output }`
- `VideoMosaic { inputs, target, layout, audio_mix, overlays }`
- `CaptionExtract { source, target, caption }`

所有输出都显式包含目标 StreamKey。公共创建请求包含 `idempotency_key`、deadline 和 spec；更新请求包含 `expected_generation` 和完整 next spec，不做含糊的部分布尔 patch。

`ProcessingJob` 固定包含：

- `job_id`、`spec`、`state`、`generation`
- `created_at`、`updated_at`、`started_at`、`finished_at`
- `profile`、selection/preflight 摘要
- 输入/输出流状态、引用数、重启次数
- frames/packets/bytes、drop、pending、flush/reset、latency 计数
- `last_error`

状态固定为 `Pending`、`Starting`、`Running`、`Draining`、`Stopped`、`Failed`。

## 5. 处理策略

删除 `TranscodePolicy`，新增：

```rust
enum ProcessingPolicy {
    Passthrough,
    Auto { preset: ProcessingPreset },
    Transcode { target: ProcessingTarget },
}

enum TrackSelection {
    All,
    AudioOnly,
    VideoOnly,
}
```

- `Passthrough` 不允许隐式 CPU 工作。
- `Auto` 仅在处理 provider 可用时创建/复用内部派生流；不可用时使用协议已定义的诚实降级。
- `Transcode` 是强要求，不能满足时直接失败，不得退回 passthrough。
- 外部显式任务必须使用非保留目标；内部任务使用 namespace `__cheetah_derived` 和规范化 spec 指纹作为 path。

## 6. 图片 API

用 `ImageProcessApi` 破坏性替换 `ImageEncodeApi`：

```rust
#[async_trait]
pub trait ImageProcessApi: Send + Sync {
    async fn process(
        &self,
        ctx: &MediaRequestContext,
        request: ImageProcessRequest,
    ) -> Result<ImageArtifact>;
}
```

输入为受控 encoded bytes 或 `Arc<AVFrame> + TrackInfo`；operation 使用 Cheetah 自有 enum 表达 crop、resize/fit、rotate、flip、pad、CSC、blend 和 text。图片、logo、字体引用受权 `FileHandle`，不接受服务端任意路径。

输出 v1 只保证 JPEG。`ImageFormat::Png` 保留 wire/Rust 兼容值，但 preflight 不声明 encode，调用返回 `Unsupported`。

## 7. 能力与字幕类型

新增能力：

- `AudioProcessing`
- `VideoProcessing`
- `ImageProcessing`

operation 至少区分 transcode、abr、audio_mix、video_mosaic、caption_extract、image_process、jpeg_encode。operation 只有在编译 feature、startup preflight 和 provider registration 三者都成立时可见。

`cheetah-codec::CodecId` 增加 `WebVtt`。WebVTT 帧使用 `MediaKind::Subtitle`、显式 timebase/PTS/duration 和 UTF-8 payload；不得塞入 video metadata 或自定义 side-data。

## 8. Native HTTP

固定路由：

- `GET /api/v1/processing/preflight`
- `POST /api/v1/processing/jobs`
- `GET /api/v1/processing/jobs`
- `GET|PATCH|DELETE /api/v1/processing/jobs/{id}`
- `POST /api/v1/processing/jobs/{id}/stop`
- `POST /api/v1/images/process`

adapter 只做认证、大小限制、serde/上传转换和错误映射。Job 创建沿用 deadline、幂等、资源授权和分页规则；图片上传使用有界 multipart/raw body，Domain API 不依赖 HTTP 类型。
