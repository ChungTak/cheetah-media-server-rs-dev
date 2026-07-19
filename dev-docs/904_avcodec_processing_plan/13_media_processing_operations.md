# 13 · 媒体处理运维指南

## 1. 启动预检（Preflight）

`MediaProcessingProvider` 在模块启动时会探测已编译 profile 的实际能力：

- backend/profile 是否启用
- 视频 H.264/H.265/MJPEG decode/encode
- 音频 G.711/AAC/Opus/MP3 decode/encode
- image operator / JPEG encode
- 音频 resample + channel adapt + flush/reset
- memory domain 和 buffer path

查询结果：

```bash
GET /api/v1/media-processing/preflight
```

响应 `ProcessingPreflightReport` 包含：

- `profile`：当前 profile（`native-free` / `software`）
- `avcodec_revision`：avcodec-rs 的 git revision
- `features`：编译进的 Cargo feature 列表
- `operations`：可用的操作（`transcode`、`abr_ladder`、`audio_mix`、`video_mosaic`、`image_process`、`audio_resample`、`caption_extract`）
- `selection`：每个 operation 选中的 backend、memory domain、staging 摘要
- `diagnostics`：不可用的 operation 及原因

若某 operation 显示 `media-processing-cpu feature not compiled`，说明该能力未编译进当前二进制；若显示 `no H.264/H.265 encoder available` 等，说明 avcodec 注册表未选中对应 backend。

## 2. 创建与停止 Job

创建：

```bash
POST /api/v1/media-processing/jobs
{
  "spec": {
    "kind": "transcode",
    "source": { "vhost": "__defaultVhost__", "app": "live", "stream": "input" },
    "target": { "vhost": "__defaultVhost__", "app": "live", "stream": "output" },
    "track_selection": "all",
    "video": { "codec": "h264", "width": 640, "height": 360 }
  }
}
```

停止：

```bash
POST /api/v1/media-processing/jobs/{job_id}/stop
```

删除：

```bash
DELETE /api/v1/media-processing/jobs/{job_id}
```

`stop` 会发送 cancel token 到 subscriber、worker、publisher 以及共享任务引用，并释放发布者租约。`delete` 会移除终态 job 的记录。

## 3. 定位 Unsupported

创建 Job 返回 `Unsupported` 时：

1. 先查 `/preflight`，确认 `operations` 包含目标 operation。
2. 查看 `diagnostics` 中对应条目的 `reason`。
3. 检查 `profile` 是否为 `software`（需要 `--features avcodec-profile-software`）。
4. 检查 `features` 是否包含 `media-processing-cpu`（混音/宫格/ABR/转码需要）。
5. 确认输入 codec 在 `audio_decode` / `video_decode` selection 中列出。

## 4. 观察队列与丢帧

关键指标：

- `media_processing_jobs{kind,state,profile}`：按 kind 和 state 分布的 job 数
- `media_processing_frames_total{direction,media,codec}`：ingress/egress 帧数
- `media_processing_drops_total{reason,media}`：按原因统计的丢帧
- `media_processing_pending_total{stage=frame}`：待处理帧数
- `media_processing_queue_depth{stage=frame}`：队列深度
- `media_processing_latency_ms{stage}`：`startup` / `first_output` / `drain`
- `media_processing_preflight{profile,operation}`：预检是否通过
- `media_processing_shared_refs`：共享 job 的引用计数
- `media_processing_resource_reserved{kind}`：`publisher` / `subscriber` 预留数
- `media_processing_restarts_total{reason=failure}`：故障重启次数

队列持续升高或 `drops_total` 增加，说明 subscriber 慢或 worker 阻塞；`latency_ms{stage=startup}` 过大说明 admission/preflight/lease 等待超时或 codec 初始化慢。

## 5. 动态库 / SBOM

`preflight` 报告的 `avcodec_revision` 字段来自 `Cargo.toml` 中 avcodec 依赖的 `rev`，可作为 SBOM 的一部分。若怀疑动态库链接异常：

```bash
# 检查编译进的 avcodec revision
grep avcodec crates/system/cheetah-media-processing-module/Cargo.toml
# 检查启动日志中的 preflight 结果
cargo test -p cheetah-media-processing-module --features media-processing-cpu preflight
```

`native-free` profile 不链接 FFmpeg 等外部动态库；`software` profile 依赖 libx264/libopus 等，需确保运行环境存在对应共享库。

## 6. 安全下线

下线 media-processing 模块前：

1. 停止所有 processing job（模块 `stop` 会自动 `cancel_all`）。
2. 观察 `ResourceLeakReport`：
   - `active_processing_job_ids` 应为空
   - `live_blocking_worker_job_ids` 应为空
   - `derived_publisher_stream_keys` / `derived_subscriber_stream_keys` 应为空
   - `shared_task_references` 应为空
3. 确认 `active_stream_keys` 中不再包含 `__cheetah_derived` namespace 的派生流。
4. 执行模块 `stop` 或引擎 `stop`，等待 `ResourceLeakReport.is_clean()` 为 `true`。

热更新配置时：

- 纯上限增加可 live apply。
- `profile` 变更、上限降低到当前使用量以下、`max_encoded_frame_bytes` / `max_overlay_font_size` 降低会返回 `ModuleRestartRequired`。
- 模块不实现私有 restart 流程，由引擎统一重建。
