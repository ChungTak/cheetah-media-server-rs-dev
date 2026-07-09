# RTMP/RTSP 协议完善设计与开发计划总索引（对标 ABLMediaServer）

- **状态**: 未开始
- **目标**: 对标 ABLMediaServer 工程实践，补齐协议兼容性、管理能力和生产运维特性
- **方法**: 对比 `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer` 实现，逐项补齐缺口
- **完成标准**: 所有阶段任务通过 `cargo fmt` + `cargo clippy` + `cargo test`，端到端互操作验证通过

---

## 与 ABLMediaServer 对比后的主要缺口

| 能力 | ABLMediaServer 参考点 | 本地状态 | 计划处理 |
|------|----------------------|----------|----------|
| RTSP 厂商兼容（宇视/大华/海康） | 自动探测编码、trackID 变体、Content-Base | ❌ 未实现 | Phase 01 |
| RTCP SR/RR 收发 | 大华摄像头要求 RR 响应 | ❌ 未实现 | Phase 01 |
| RTSP OPTIONS 心跳 | 25s 周期 OPTIONS keep-alive | ❌ 部分（GET_PARAMETER） | Phase 01 |
| G711→AAC 实时转码 | `nG711ConvertAAC` | ❌ 未实现 | Phase 02 |
| 多格式录制（FMP4/MP4/TS/FLV） | 5 种格式 + 分片 + 保留策略 | ❌ 未实现 | Phase 03 |
| RTSP VOD 回放（Seek/Pause/Speed） | 多文件连续播放 + 倍速 | ❌ 未实现 | Phase 03 |
| HTTP Webhook 事件系统 | 16+ 事件类型 | ❌ 未实现 | Phase 04 |
| HTTP 管理 API | 30+ 端点 | ❌ 部分 | Phase 04 |
| 无人观看自动关流 | `maxTimeNoOneWatch` | ❌ 未实现 | Phase 05 |
| 强制关键帧给新订阅者 | `ForceSendingIFrame` | ✅ 已有（keyframe gate） | — |
| 按需拉流（on_stream_not_found） | Webhook 触发 | ❌ 未实现 | Phase 04 |
| 快照/截图 API | `getSnap` 解码 I 帧输出 JPEG | ❌ 未实现 | Phase 06 |
| 视频水印/文字叠加 | FFmpeg filter graph | ❌ 未实现 | Phase 06 |
| GB28181 完整集成 | PS 封装/解封装 + RTP 收发 | ❌ 未实现 | Phase 07 |

---

## 总体约束

1. 严格遵循 `core + driver + module` 三段式架构
2. 厂商兼容逻辑集中管理，不散落在协议热路径
3. 转码/截图等重计算功能作为独立可选模块
4. Webhook/API 通过 `cheetah-control` 统一暴露
5. 录制作为独立模块 `cheetah-record-module`
6. GB28181 作为独立协议 crate `cheetah-gb28181-*`

---

## 参考来源

| 来源 | 路径 |
|------|------|
| ABLMediaServer | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/` |
| 本项目架构文档 | `SystemArchitecture.md`、`AGENTS.md` |
| 前序计划 | `dev-docs/plans-11/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [phase-01-rtsp-compat.md](phase-01-rtsp-compat.md) | 未开始 | RTSP 厂商兼容与协议健壮性 |
| [phase-02-audio-transcode.md](phase-02-audio-transcode.md) | 未开始 | G711→AAC 实时转码 |
| [phase-03-recording-vod.md](phase-03-recording-vod.md) | 未开始 | 多格式录制 + RTSP VOD 回放 |
| [phase-04-webhook-api.md](phase-04-webhook-api.md) | 未开始 | HTTP Webhook 事件 + 管理 API |
| [phase-05-stream-lifecycle.md](phase-05-stream-lifecycle.md) | 未开始 | 流生命周期管理（无人观看关流、按需拉流） |
| [phase-06-media-processing.md](phase-06-media-processing.md) | 未开始 | 快照/截图 + 视频水印 |
| [phase-07-gb28181.md](phase-07-gb28181.md) | 未开始 | GB28181 协议集成 |

---

## 任务状态总表

| 阶段 | 任务 | 状态 |
|------|------|------|
| 1.1 | 宇视摄像头 SDP 编码探测兼容 | 未开始 |
| 1.2 | 大华摄像头 Digest 认证流程兼容 | 未开始 |
| 1.3 | 海康 NVR trackID/Content-Base 兼容 | 未开始 |
| 1.4 | RTCP RR 响应（UDP 播放必需） | 未开始 |
| 1.5 | RTSP 客户端 OPTIONS 心跳 | 未开始 |
| 1.6 | RTP 时间戳零值修正（GB28181 设备） | 未开始 |
| 2.1 | G711A/U→AAC 实时转码模块 | 未开始 |
| 3.1 | 录制引擎（FMP4/MP4/TS/FLV 格式） | 未开始 |
| 3.2 | 录制分片与保留策略 | 未开始 |
| 3.3 | RTSP VOD 回放（Seek/Pause/Speed） | 未开始 |
| 4.1 | Webhook 事件框架 | 未开始 |
| 4.2 | 流管理 API（addStreamProxy/delStreamProxy） | 未开始 |
| 4.3 | 按需拉流（on_stream_not_found） | 未开始 |
| 5.1 | 无人观看自动关流 | 未开始 |
| 5.2 | 流超时检测与清理 | 未开始 |
| 6.1 | 快照/截图 API | 未开始 |
| 6.2 | 视频水印/文字叠加 | 未开始 |
| 7.1 | GB28181 PS 封装/解封装 | 未开始 |
| 7.2 | GB28181 RTP 收发（UDP/TCP） | 未开始 |
| 7.3 | GB28181 信令对接 | 未开始 |

---

## 渐进式执行顺序

1. **Phase 01** — RTSP 厂商兼容：直接提升真实设备互操作性
2. **Phase 02** — 音频转码：解决 G711 设备无法跨协议播放的问题
3. **Phase 03** — 录制与 VOD：核心存储能力
4. **Phase 04** — Webhook + API：运维管理基础设施
5. **Phase 05** — 流生命周期：生产环境资源管理
6. **Phase 06** — 媒体处理：增值功能
7. **Phase 07** — GB28181：国标协议扩展
