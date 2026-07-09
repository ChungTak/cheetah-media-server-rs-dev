# 双时间线媒体架构设计

- 状态：进行中
- 范围：定义 source timeline、canonical timeline、egress timeline 的边界、数据流和协议职责。
- 完成标准：实现者能够据此修改 RTSP/RTMP/SRT/WebRTC 接入方式，不再混用 RTP timestamp、DTS、PTS、RTMP timestamp、RTP egress timestamp。

## 架构目标

当前问题的核心不是“是否统一时间戳”，而是统一的层次不够清晰。RTSP RTP timestamp 被直接当成 `AVFrame.dts` 后，带 B 帧的视频会触发大量 DTS 修复，修复结果把解码时间压缩为 `last + 1 tick`，导致播放不如旧版本顺畅。

新的架构固定三层时间模型：

1. Source timeline：保留源协议给出的原始时间语义，例如 RTP timestamp、RTMP tag timestamp、CTS、wrap、epoch、RTCP SR 映射。
2. Canonical timeline：引擎内部统一 `AVFrame + TrackInfo`，DTS/PTS 可解释、平滑、适合 RingBuffer、bootstrap、pacing、录制和转协议。
3. Egress timeline：由 `cheetah-codec` 从 canonical timeline 导出目标封装需要的时间，例如 RTMP timestamp/CTS、RTSP RTP timestamp、WebRTC RTP timestamp、SRT/FLV/HLS 封装时间。

## 具体任务

### A.1 明确三层时间模型

- [x] 在 `SystemArchitecture.md` 同步补充 source/canonical/egress timeline 定义。
- [x] 在 `cheetah-codec` 文档中明确 `AVFrame.pts/dts` 永远是 canonical timeline，不直接等同源 RTP timestamp 或 RTMP tag timestamp。
- [x] 明确 source timeline 只用于兼容、同协议保真、RTCP 映射、排障和 egress 导出辅助。
- [x] 明确 egress timeline 是目标协议视图，不允许倒逼 ingress 修改 canonical 语义。

### A.2 明确 ingress/canonical/egress 边界

- [x] RTSP ingress：RTP depacketize 与 AU 边界识别后，把 RTP timestamp 作为 source PTS 输入，不作为 canonical DTS。
- [x] RTMP ingress：RTMP tag timestamp 是源 DTS，CTS 是源 composition offset，可直接生成 canonical DTS/PTS，但仍保留原始 tag timestamp。
- [x] Engine：只存 canonical `AVFrame`，bootstrap 和 pacing 不读取协议私有状态。
- [x] RTMP/RTSP egress：只通过 `cheetah-codec` 导出目标时间戳和 codec config，不在 module 中私自修 timeline。

### A.3 明确兼容策略与未来协议约束

- [x] SRT ingress 按来源封装选择 source timeline 类型，不复制 RTSP/RTMP 私有修正逻辑。
- [x] WebRTC ingress 保留 RTP timestamp / sequence / marker / RTCP feedback，但 canonical DTS/PTS 仍由 codec adapter 生成。
- [x] 兼容脏数据时先记录 source timeline，再进入 canonical 修复；告警必须能说明 source 值、canonical 值和修复原因。
- [x] `NonMonotonicDtsRepaired` 不得作为 discontinuity 边界；只有真实 reset/restart/大跳变才切段。

## 最新进展

- 2026-04-29：完成任务 A.3。`cheetah-codec` future protocol ingress 契约补齐 WebRTC 归一化约束：`WebRtcRtpRtcp` 与 `SrtTransport` 一样必须走 `TimelineSource::TimestampNormalizer`，否则返回结构化错误；新增契约测试覆盖 WebRTC passthrough 拒绝和 normalized 接受。`cheetah-rtsp-module` publish 侧时间修复日志补齐 `source_pts/source_dts` 字段，确保脏数据排障能同时看到 source 与 canonical 值以及 alert 原因。并继续保持 `NonMonotonicDtsRepaired` 不触发 discontinuity 语义。
- 2026-04-29：完成任务 A.2。`cheetah-rtsp-module` publish 入站新增 `source_dts_for_rtsp_ingress()` 边界函数：视频轨不再将 RTP timestamp 作为 canonical DTS 输入，改为 `PTS-only` 进入 normalizer；音频轨继续保留 DTS 输入。新增回归测试覆盖“视频 raw dts 输入被忽略、音频 dts 输入被保留”。同时复核 RTMP ingress（DTS+CTS 输入 normalizer）、Engine bootstrap（仅 canonical `AVFrame`）和 RTMP/RTSP egress（统一 codec 导出）边界符合当前架构约束。
- 2026-04-29：完成任务 A.1。`SystemArchitecture.md` 在统一媒体语义章节补充三层时间模型与边界规则，明确 source timeline、canonical timeline、egress timeline 的职责；`cheetah-codec` crate 级文档新增 timeline contract，明确 `AVFrame.pts/dts` 仅表示 canonical timeline，协议原始 timestamp 只能作为 source metadata，egress timestamp 修复不得回写 canonical timeline。
- 2026-04-29：计划已创建，任务未开始。

## 完成后检查

- 检查 `SystemArchitecture.md` 与 `dev-docs/plans-4` 术语一致。
- 检查 `AGENTS.md` 的 `cheetah-codec` 规则未被破坏。
- 检查计划中没有要求 module 复制 timestamp normalizer、NALU 处理或参数集缓存。
