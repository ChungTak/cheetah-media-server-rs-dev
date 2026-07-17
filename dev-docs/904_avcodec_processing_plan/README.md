# 904 · avcodec-rs 可选媒体处理能力开发计划

> 面向下一阶段执行实现的开发指导。本文档集固定依赖边界、公共契约、任务顺序、测试矩阵和发布证据；实现者不需要重新决定架构。

## 1. 文档地位

本目录基于当前 `main` 源码、历史 `dev-docs` 任务以及
[avcodec-rs SDK integration guide](https://raw.githubusercontent.com/TimothyWalker6922/avcodec-rs-develop/refs/heads/main/docs/sdk-integration-guide.md)
重新审计。903 中已经交付的快照、代理、能力和资源生命周期继续作为基线；本目录只处理因缺少音视频编解码或图片处理后端而未完成的能力，并替换现有 FFmpeg/image 实现。

完成标准是生产 provider、真实派生流和可复现互操作证据。trait、配置字段、fake encoder、静态能力声明或只保持 session 存活均不构成完成。

## 2. 不可变边界

- 所有处理能力默认不编译；Cargo 默认 feature 集不得出现 `avcodec`。
- Cheetah 只直接依赖顶层 `avcodec` crate，固定 version + git revision，`default-features = false`。
- 不直接依赖 avcodec-rs backend/core/FFI crate、FFmpeg/image 库或 FFmpeg 可执行任务。
- `profile-software` 可以在 avcodec-rs 内部选择其 FFmpeg backend；该实现细节不得泄漏到 Cheetah 类型、日志契约或调用点。
- `cheetah-codec` 继续只维护 `AVFrame + TrackInfo`、时间戳、Access Unit、参数集和兼容解析，不持有编解码 session。
- 编解码、图片处理、混流和宫格属于 `cheetah-media-processing-module` 的有界 Job/Work；协议 core 保持 Sans-I/O。
- 所有流式处理结果发布到独立 `StreamKey`，不得覆盖源流或绕过单发布者租约。
- 公共 API runtime-neutral、framework-neutral，不暴露 Tokio、avcodec-rs 或 FFmpeg 类型。
- 能力报告只声明 preflight 和真实数据面均通过的 operation。

## 3. 发布范围

| 能力 | 本期交付 |
| --- | --- |
| 音频 | G711A/U、AAC、Opus 互转；上游补齐后的 MP3 → Opus；重采样和声道适配 |
| 视频 | H.264/H.265/MJPEG 解码，H.264/H.265 编码，缩放、帧率/码率调整 |
| 图片 | JPEG/PNG 输入解码；crop、resize/fit、rotate、flip、pad、CSC、blend、文字 OSD；JPEG 输出 |
| 快照 | MJPEG/H.264/H.265 → JPEG |
| 扩展 | 显式 ABR 梯度、音频混音、固定视频宫格、图片/文字水印 |
| 字幕 | H.264/H.265 SEI 中 CEA-608/708 → WebVTT；HLS 字幕轨 |
| 协议闭环 | Snapshot、RTMP/HTTP-FLV、WebRTC、Pull Proxy、HLS |

PNG 编码保留公共兼容枚举但返回 `Unsupported`。硬件 profile、SVC bitstream、DRM、DVR、SCTE-35/CUE 和自由场景编排不属于本期发布范围。

## 4. 执行阶段

| 阶段 | 目标 | 退出条件 |
| --- | --- | --- |
| P0 | 上游、依赖、公共契约和旧能力清理 | MP3 上游合入并锁 revision；默认构建无 avcodec；FFmpeg/image 边界删除 |
| P1 | 单流基础处理 | 音频矩阵、视频转码、图片处理、JPEG 快照和水印通过真实数据测试 |
| P2 | 派生流和扩展能力 | Job、ABR、混音、宫格、CEA/WebVTT 及五条协议闭环通过 |
| P3 | 产品化与发布 | 安全、资源、观测、故障、互操作、长稳和发布证据全部通过 |

每个阶段按 Domain/SDK → provider/module → adapter → tests/docs 顺序执行。公共 trait 同一时刻只允许一个迁移序列修改。

## 5. 文档索引

1. [审计基线与差距登记](01_audited_baseline_and_gap_register.md)
2. [avcodec 依赖与上游契约](02_avcodec_dependency_and_upstream_contract.md)
3. [架构与公共契约](03_architecture_and_public_contracts.md)
4. [任务、运行时与资源模型](04_job_runtime_and_resource_model.md)
5. [图片、快照与水印](05_image_snapshot_and_overlay.md)
6. [音频转码](06_audio_transcode.md)
7. [视频转码与 ABR](07_video_transcode_and_abr.md)
8. [音频混音与视频宫格](08_audio_mix_and_video_mosaic.md)
9. [CEA 与 WebVTT](09_cea_and_webvtt.md)
10. [协议集成](10_protocol_integration.md)
11. [FFmpeg 删除与破坏性迁移](11_ffmpeg_removal_and_breaking_migration.md)
12. [安全、观测与运维](12_security_observability_and_operations.md)
13. [测试、CI 与发布门禁](13_test_ci_and_release_gates.md)
14. [执行路线与 Agent 交接](14_execution_roadmap_and_agent_handoff.md)
15. [发布证据模板](15_release_evidence_template.md)

## 6. 全局完成定义

- [ ] 默认构建和默认制品不包含 avcodec-rs 或媒体处理模块。
- [ ] 只有顶层 `avcodec` 是直接依赖，revision、feature、许可证和 SBOM 可追溯。
- [ ] 所有处理 session 单 worker 所有，异步热路径不执行阻塞编解码。
- [ ] 所有缓存、队列、输入数、像素率、并发任务和重试有上界。
- [ ] Snapshot、RTMP/HTTP-FLV、WebRTC、Pull Proxy、HLS 使用真实派生流完成闭环。
- [ ] admission deny、取消、源流中断、module restart 和 shutdown 不留下租约、流或任务。
- [ ] 能力、API、配置、日志、metrics、health 与当前编译 profile 和 preflight 结果一致。
- [ ] 24 小时混合负载长稳和发布矩阵在同一候选制品上通过。
