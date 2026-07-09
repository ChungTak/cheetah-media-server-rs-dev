# RTMP 协议完善设计与开发计划总索引（对标 ABLMediaServer）

- **状态**: 未开始
- **目标**: 对标 ABLMediaServer 工程实践，补齐 RTMP 协议兼容性、鲁棒性和生产运维特性
- **方法**: 对比 `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer` 实现，逐项补齐缺口
- **完成标准**: 所有阶段任务通过 `cargo fmt` + `cargo clippy` + `cargo test`，端到端互操作验证通过

---

## 与 ABLMediaServer 对比后的主要缺口

| 能力 | ABLMediaServer 参考点 | 本地状态 | 计划处理 |
|------|----------------------|----------|----------|
| 音视频时间戳同步校正 | `SyncVideoAudioTimestamp` ±10ms/500ms | ❌ 未实现 | Phase 01 |
| 自动帧率检测（PTS 采样） | `CalcFlvVideoFrameSpeed` 250 样本 | ❌ 未实现 | Phase 01 |
| SPS/PPS/VPS 自动补发 | I 帧缺失参数集时自动补发 | ✅ 已有（ParameterSetCache） | — |
| GOP 缓存秒开 | `ForceSendingIFrame` 缓存最近 GOP | ✅ 已有（bootstrap） | — |
| 静音音频注入 | `flvPlayAddMute` | ✅ 已有（enable_add_mute） | — |
| 写入错误容忍 | 30 次连续写入失败才断开 | ❌ 未实现 | Phase 01 |
| 重复流拒绝 | 同 URL 推流立即拒绝 | ✅ 已有（单发布者独占） | — |
| 音视频选择性禁用 | `disableVideo`/`disableAudio` | ❌ 未实现 | Phase 02 |
| G711→AAC 实时转码 | `G711ConvertAAC` | ❌ 未实现 | Phase 03 |
| Webhook 事件系统 | on_publish/on_play/on_disconnect 等 | ❌ 未实现 | Phase 04 |
| 自动录制（推流触发） | `pushEnable_mp4` | ❌ 未实现 | Phase 04 |
| 无人观看超时关流 | `maxTimeNoOneWatch` | ❌ 未实现 | Phase 04 |
| RTMP 302 重定向 | 拉流客户端支持 302 | ❌ 未实现 | Phase 02 |
| URL 查询参数保留 | `?token=xxx` 提取用于鉴权 | ✅ 已有（auth token） | — |
| H.265 over RTMP | Enhanced FLV HEVC | ✅ 已有（Enhanced RTMP） | — |
| G.711 in FLV | 非标准 FLV 音频 ID | ✅ 已有（G711A/G711U） | — |
| RTMPS 推拉流 | SSL/TLS 客户端+服务端 | ✅ 已有 | — |

---

## 总体约束

1. 严格遵循 `core + driver + module` 三段式架构
2. 时间戳同步校正在 `cheetah-codec` 或 module egress 层实现
3. Webhook 通过引擎 EventBus + 独立模块实现
4. 转码作为独立可选模块
5. 所有新增能力默认禁用，通过配置开启

---

## 参考来源

| 来源 | 路径 |
|------|------|
| ABLMediaServer RTMP 实现 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/CNetRtmpServerRecv.cpp` |
| ABLMediaServer RTMP 拉流 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/NetClientRecvRtmp.cpp` |
| ABLMediaServer RTMP 推流 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/NetClientSendRtmp.cpp` |
| ABLMediaServer 版本信息 | `vendor-ref/ABLMediaServer-src-2026-05-09/版本信息.txt` |
| 本项目前序计划 | `dev-docs/plans-11/`、`dev-docs/plans-12/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [phase-01-timestamp-robustness.md](phase-01-timestamp-robustness.md) | 未开始 | 音视频同步校正、帧率检测、写入容错 |
| [phase-02-client-enhancements.md](phase-02-client-enhancements.md) | 未开始 | 音视频选择性禁用、302 重定向、推流参数集跳过 |
| [phase-03-audio-transcode.md](phase-03-audio-transcode.md) | 未开始 | G711→AAC 实时转码 |
| [phase-04-ops-integration.md](phase-04-ops-integration.md) | 未开始 | Webhook 事件、自动录制、无人观看关流 |

---

## 任务状态总表

| 阶段 | 任务 | 状态 |
|------|------|------|
| 1.1 | 音视频时间戳同步校正（egress ±10ms/500ms） | 未开始 |
| 1.2 | 自动帧率检测（PTS 差值采样 250 帧） | 未开始 |
| 1.3 | 写入错误容忍（N 次失败才断开） | 未开始 |
| 2.1 | 拉流/推流音视频选择性禁用 | 未开始 |
| 2.2 | RTMP 拉流 302 重定向支持 | 未开始 |
| 2.3 | 推流客户端参数集帧跳过（避免重复 SPS/PPS） | 未开始 |
| 3.1 | G711A/U→AAC 实时转码模块 | 未开始 |
| 4.1 | Webhook 事件框架（on_publish/on_play/on_disconnect） | 未开始 |
| 4.2 | 推流自动录制（pushEnable_mp4） | 未开始 |
| 4.3 | 无人观看超时关流 | 未开始 |

---

## 渐进式执行顺序

1. **Phase 01** — 时间戳与鲁棒性：直接提升播放质量和连接稳定性
2. **Phase 02** — 客户端增强：提升代理/转发场景能力
3. **Phase 03** — 音频转码：解决 G711 跨协议播放问题
4. **Phase 04** — 运维集成：生产环境管理能力
