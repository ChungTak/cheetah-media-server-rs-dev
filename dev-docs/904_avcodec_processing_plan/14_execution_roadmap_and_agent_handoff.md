# 14 · 执行路线与 Agent 交接

## 1. 依赖图

```text
UP-01
  -> DEP-01 -> API-01 -> RUN-01 -> CAP-01
  -> IMG-01 -> SNAP-01
  -> AUD-01 -> AUD-02 -> AUD-03
  -> VID-01 -> ABR-01
  -> MIX-01/MIX-02
  -> SUB-01 -> HLS-SUB-01
  -> INT-RTMP / INT-WEBRTC / INT-PROXY / INT-HLS
  -> MIG-01 -> OBS-01 -> REL-01
```

`UP-01` 只阻塞 MP3 分支；其他 P0 scaffolding 可以先行。`API-01` 合入前不得并行修改多个 provider 的公共 trait 调用。

## 2. P0：上游与边界

| Task | 实施内容 | 完成证据 |
| --- | --- | --- |
| UP-01 | avcodec-rs MP3 model/software decoder/AudioTranscoder | 上游 merge SHA + tests |
| DEP-01 | pinned 顶层依赖、features、license/SBOM | C0–C4 Cargo tree/build |
| API-01 | Processing/Image API、typed policy、WebVtt 类型 | SDK contract tests |
| RUN-01 | RuntimeApi spawn_blocking + Tokio adapter | runtime tests、无 Tokio 泄漏 |
| CAP-01 | provider slot、preflight、capability honesty | registration/restart tests |

P0 期间先增加新契约，再迁移 provider；FFmpeg 删除留到新 provider 可替代后执行，避免 workspace 中间态不可编译。

## 3. P1：单流能力

| Task | 实施内容 | 完成证据 |
| --- | --- | --- |
| IMG-01 | 图片输入、九类算子、JPEG 输出 | image golden + negative matrix |
| SNAP-01 | Snapshot 改用 ImageProcessApi | 三 codec snapshot E2E |
| AUD-01 | G711/AAC/Opus adapter 与 session | audio matrix |
| AUD-02 | 重采样、声道、timestamp、flush | sample/timeline tests |
| AUD-03 | MP3 → Opus | upstream + WebRTC audio fixture |
| VID-01 | H.264/H.265/MJPEG → H.264/H.265 | NativeFree/Software matrix |
| OSD-01 | 图片和静态文字水印 | visual golden + generation update |

## 4. P2：任务与协议

| Task | 实施内容 | 完成证据 |
| --- | --- | --- |
| JOB-01 | registry、状态、配额、共享指纹、恢复 | lifecycle/fault tests |
| ABR-01 | 1–4 档派生流、原子发布 | HLS master/switch E2E |
| MIX-01 | 2–16 路音频混音 | RMS/sync/stale tests |
| MIX-02 | 2–9 路固定宫格 | layout/source-loss E2E |
| SUB-01 | SEI/CEA parser 与 WebVTT frame | unit/property/fuzz |
| HLS-SUB-01 | VTT muxer/playlist/master | HLS subtitle E2E |
| INT-RTMP | G711/Opus → AAC | RTMP/HTTP-FLV decode |
| INT-WEBRTC | AAC/G711/MP3 → Opus | Chrome WHEP stats/audio |
| INT-PROXY | Pull + ProcessingPolicy | RTSP → derived → consumer |

## 5. P3：迁移与产品化

| Task | 实施内容 | 完成证据 |
| --- | --- | --- |
| MIG-01 | 删除 FFmpeg/image/TranscodePolicy 旧边界 | rg/Cargo/API migration tests |
| SEC-01 | admission、授权、deadline、幂等、FileHandle | deny 无副作用 |
| OBS-01 | preflight、logs、metrics、health、leak report | observability tests |
| PERF-01 | benchmark、fault、24h soak | report + artifact |
| REL-01 | profile builds、license、SBOM、release evidence | C0–C6 全绿 |
| DOC-01 | 架构、配置、README、API/ops 文档同步 | link/config review |

## 6. 每项任务执行模板

1. 在 01 差距表确认 current state 和 task ID。
2. 先写失败测试，证明真实缺口，不用 fake success 替代。
3. 修改最小公共契约；破坏性变更同时迁移所有 workspace 调用方。
4. 按 SDK → provider/module → adapter → E2E 实施。
5. 检查 admission、配额、deadline、取消、重启和清理。
6. 运行 changed crate、反向依赖和对应 CI lane。
7. 更新本路线状态和 15 发布证据，记录命令、revision、profile、制品。

交接必须写明 task ID、公共接口变更、当前 pinned revision、feature/profile、未完成分支、测试结果、资源上限和安全回滚点。禁止使用“基本完成”“理论支持”“应该可用”。

## 7. 并行规则

- UP-01 可与 API/RUN scaffolding 并行，但主代理必须独立复核 integration guide。
- 图片、音频、视频可在 API/RUN 稳定后并行，不能同时修改同一公共 trait。
- MIX/ABR 在单流 audio/video matrix 通过后开始。
- CEA parser 可独立开发；HLS VTT 必须等待公共 WebVtt 类型稳定。
- 各协议集成等待 production Job provider，不允许各自临时接 avcodec。
- MIG-01 最后执行，但每个迁移任务应及时删除已替代的局部旧实现。

## 8. 最终 DoD

所有 task 有唯一 owner、提交和证据；C0–C6 全绿；五条 E2E、24 小时 soak、license/SBOM 和空 leak report 在同一候选制品上完成；能力报告与该制品实际 profile/preflight 一致。
