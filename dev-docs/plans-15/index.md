# RTMP/RTSP 协议转换完善计划（对标 ABLMediaServer）

- **状态**: 未开始
- **目标**: 对标 ABLMediaServer 工程实践，补齐 RTMP↔RTSP 协议转换的生产级兼容性、鲁棒性和运维能力
- **方法**: 对比 `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer` 实现，逐项补齐缺口
- **完成标准**: 所有阶段任务通过 `cargo fmt` + `cargo clippy` + `cargo test`，端到端互操作验证通过

---

## 与 ABLMediaServer 对比后的主要缺口

| 能力 | ABLMediaServer 参考点 | 本地状态 | 计划处理 |
|------|----------------------|----------|----------|
| 音视频时间戳同步校正 | `SyncVideoAudioTimestamp` ±10ms/500ms | ⚠️ 有 AvSyncAligner 但未集成到 egress | Phase 01 |
| 自动帧率检测（PTS 采样） | `CalcFlvVideoFrameSpeed` 250 样本 | ❌ 未实现 | Phase 01 |
| 写入错误容忍 | 30 次连续写入失败才断开 | ❌ 未实现 | Phase 01 |
| G711→AAC 实时转码 | FAAC 编码，8kHz→AAC | ⚠️ 有管线框架但无 AAC 编码器实现 | Phase 02 |
| AAC→G711 实时转码 | FFmpeg 解码 AAC→PCM→G711 | ❌ 未实现 | Phase 02 |
| 音视频选择性禁用 | `disableVideo`/`disableAudio` | ❌ 未实现 | Phase 02 |
| 无人观看超时关流 | `maxTimeNoOneWatch` | ❌ 未实现 | Phase 03 |
| Webhook 事件系统 | on_publish/on_play/on_disconnect | ❌ 未实现 | Phase 03 |
| 自动录制（推流触发） | `pushEnable_mp4` | ❌ 未实现 | Phase 03 |
| RTMP 302 重定向 | 拉流客户端支持 302 | ❌ 未实现 | Phase 03 |
| SPS/PPS 缺失时自动补发 | `findVpsSpsPps` + `pSPSPPSBuffer` 补发 | ✅ 已有（ParameterSetCache） | — |
| GOP 缓存秒开 | `ForceSendingIFrame` + `pVideoGopFrameBuffer` | ✅ 已有（bootstrap + ring buffer） | — |
| 静音音频注入 | `flvPlayAddMute` | ✅ 已有（enable_add_mute / enable_mute_audio） | — |
| 重复流拒绝 | 同 URL 推流立即拒绝 | ✅ 已有（单发布者独占） | — |
| H.265 over RTMP | Enhanced FLV HEVC | ✅ 已有（Enhanced RTMP） | — |
| G.711 in FLV | 非标准 FLV 音频 ID | ✅ 已有（G711A/G711U） | — |
| RTMPS 推拉流 | SSL/TLS 客户端+服务端 | ✅ 已有 | — |
| 跨协议参数集补发 | SPS/PPS 在 RTSP play 时补发 | ✅ 已有（play_parameter_set_caches） | — |
| B 帧 PTS 回退处理 | 视频允许 PTS 非单调 | ✅ 已有（skip monotonic repair） | — |
| 增量 RTP 时间戳生成 | 帧计数 × 步长 | ✅ 已有（IncrementalRtpTimestampGenerator） | — |
| RTCP 关键帧请求 | PLI/FIR 转发 | ✅ 已有（request_keyframe API） | — |

---

## 总体约束

1. 严格遵循 `core + driver + module` 三段式架构
2. 时间戳同步校正在 module egress 层实现，不修改 `cheetah-codec` 核心归一化逻辑
3. G711↔AAC 转码通过 `AacEncoder`/`OpusDecoder` trait 插件化，AAC 编码器通过 feature flag 引入
4. Webhook 通过引擎 EventBus + 独立 HTTP 客户端模块实现
5. 所有新增能力默认禁用，通过配置开启

---

## 参考来源

| 来源 | 路径 |
|------|------|
| ABLMediaServer RTMP 接收 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/CNetRtmpServerRecv.cpp` |
| ABLMediaServer RTSP 发送 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/NetClientSendRtsp.cpp` |
| ABLMediaServer 媒体源 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/CMediaStreamSource.cpp` |
| ABLMediaServer 版本信息 | `vendor-ref/ABLMediaServer-src-2026-05-09/版本信息.txt` |
| 本项目前序计划 | `dev-docs/plans-14/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [phase-01-egress-robustness.md](phase-01-egress-robustness.md) | 未开始 | A/V 同步校正集成、帧率检测、写入容错 |
| [phase-02-audio-transcode.md](phase-02-audio-transcode.md) | 未开始 | G711↔AAC 转码实现、音视频选择性禁用 |
| [phase-03-ops-integration.md](phase-03-ops-integration.md) | 未开始 | Webhook 事件、自动录制、无人观看关流、302 重定向 |

---

## 任务状态总表

| 阶段 | 任务 | 状态 |
|------|------|------|
| 1.1 | A/V 时间戳同步校正集成到 RTMP/RTSP egress | 未开始 |
| 1.2 | 自动帧率检测（PTS 差值采样） | 未开始 |
| 1.3 | 写入错误容忍（N 次失败才断开） | 未开始 |
| 2.1 | G711→AAC 实时转码（FAAC/fdk-aac 集成） | 未开始 |
| 2.2 | AAC→G711 实时转码（AAC 解码 + PCM→G711） | 未开始 |
| 2.3 | 拉流/推流音视频选择性禁用 | 未开始 |
| 3.1 | Webhook 事件框架（on_publish/on_play/on_disconnect） | 未开始 |
| 3.2 | 推流自动录制（pushEnable_mp4） | 未开始 |
| 3.3 | 无人观看超时关流 | 未开始 |
| 3.4 | RTMP 拉流 302 重定向支持 | 未开始 |

---

## 渐进式执行顺序

1. **Phase 01** — Egress 鲁棒性：直接提升跨协议播放质量和连接稳定性
2. **Phase 02** — 音频转码：解决 G.711 设备跨协议播放问题
3. **Phase 03** — 运维集成：生产环境管理能力
