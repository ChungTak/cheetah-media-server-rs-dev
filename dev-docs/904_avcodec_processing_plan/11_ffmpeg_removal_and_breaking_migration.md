# 11 · FFmpeg 删除与破坏性迁移

## 1. 迁移原则

本期是有意的破坏性清理，不保留一版兼容解析或 deprecated shim。目标是让 Cheetah 生产代码只通过 avcodec-rs 获取 codec/image 能力，并彻底删除通用外部 FFmpeg 进程执行面。

`profile-software` 在 avcodec-rs 内部使用的 backend 属于上游封装实现，不恢复任何 Cheetah FFmpeg API。

## 2. 删除清单

### SDK/Domain

- `FfmpegApi`
- `FfmpegJobSpec`、`FfmpegJobHandle`、`FfmpegJobStatus`
- `FfmpegInput`、`FfmpegOutput`、`FfmpegResourceLimits`、`FfmpegJobState`
- `FfmpegProxyRequest`
- `ProxyKind::Ffmpeg`
- `AdmissionAction::CreateFfmpegProxy`
- `ProxyApi::{create,delete,get}_ffmpeg_proxy`
- 旧 `TranscodePolicy`

### Engine

- `LocalFfmpegService`
- `EngineContext.ffmpeg_api`
- executable/profile 配置和进程 supervision
- running FFmpeg jobs resource-leak 字段
- FFmpeg health/capability 检测

### Module/Adapter

- Proxy module 的 FFmpeg job orchestration
- Native `/proxies/ffmpeg` 路由
- ZLM add/del/list FFmpeg source 兼容实现
- FFmpeg operation/capability/URL 宣告
- 只为 FFmpeg executor 存在的测试 fixture 和配置

### Codec/Image

- `cheetah-codec::transcode` 中执行器 traits、G711 查表 codec、临时 resampler 和未使用 pipeline
- Snapshot module 的 `image` backend、重复 resize/encode fallback
- Workspace 对 `image` 的直接 dependency 及仅为它存在的 feature 配置

## 3. 新旧映射

| 旧契约 | 新契约 |
| --- | --- |
| `FfmpegApi::submit/get/list/wait/cancel/remove` | `MediaProcessingApi` Job 生命周期 |
| FFmpeg proxy | Pull Proxy + `ProcessingPolicy` |
| `TranscodePolicy` 布尔字段 | `ProcessingPolicy` + typed Job spec |
| `disable_audio/video` | `TrackSelection` |
| `ImageEncodeApi::encode` | `ImageProcessApi::process` |
| FFmpeg job leak/health | Processing Job/worker/derived stream leak/health |
| FFmpeg executable config | avcodec profile/features + processing limits |

旧 FFmpeg URL 不重定向到新 Job API，返回标准 404；旧 Rust 符号直接移除；旧 YAML 字段由 `deny_unknown_fields`/配置校验明确拒绝，不静默忽略。

## 4. 迁移顺序

1. 先加入新 public types/traits/provider slot，使 workspace 可编译。
2. 新增处理 module 和 production provider。
3. 迁移 Snapshot、Pull Proxy、WebRTC、RTMP/HTTP-FLV、HLS 调用点。
4. 更新 Native API、配置和能力报告。
5. 删除所有 FFmpeg API/实现/路由和 image backend。
6. 运行全 workspace `rg`/Cargo tree guard，确认没有残余生产依赖。
7. 更新 `SystemArchitecture.md`、`AGENTS.md`、README、配置示例和 API 文档。

迁移提交不得出现“旧 provider 和新 provider 同时维护同一任务”的双状态源。

## 5. 允许保留的 FFmpeg 文本

- 互操作客户端列表，例如 FFmpeg/ffprobe/OBS/VLC。
- 历史 `dev-docs` 和 release evidence。
- avcodec `profile-software` 的供应链说明。
- CI 脚本中作为外部测试工具的命令。

不允许保留生产 executor、公共类型、路由、配置、能力、进程启动或直接 library binding。

## 6. 验收

- [ ] `rg` 不再找到生产 `FfmpegApi`、`LocalFfmpegService`、`FfmpegProxyRequest` 调用。
- [ ] Native/ZLM 路由目录不宣告 FFmpeg source 操作。
- [ ] Cargo manifests 无直接 FFmpeg/image/backend/FFI dependency。
- [ ] 旧配置和 HTTP 请求有明确破坏性迁移测试。
- [ ] 新处理 Job 可以覆盖此前 FFmpeg proxy 中真正需要的受控 codec 转换，但不恢复任意命令执行。
