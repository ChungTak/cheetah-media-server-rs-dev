# RTMP/RTSP 协议转换设计与开发计划总索引（对标 ZLMediaKit）

- **状态**: 未开始
- **目标**: 对标 ZLMediaKit 工程实践，完善 RTMP↔RTSP 协议转换的兼容性、秒开、时间戳精度和音视频处理
- **方法**: 对比 `vendor-ref/ZLMediaKit/src` 实现，逐项补齐本地协议转换缺口
- **完成标准**: 所有阶段任务通过 `cargo fmt` + `cargo clippy` + `cargo test`，RTMP→RTSP / RTSP→RTMP 端到端互操作验证通过

---

## 架构现状

本项目已实现 RTMP 和 RTSP 的独立 publish/subscribe 流程，通过共享的 `AVFrame + TrackInfo` 模型和 `StreamManager` 实现了基本的跨协议桥接：

```
RTMP Publish → [demux → normalize → AVFrame] → Engine Ring Buffer → [AVFrame → packetize] → RTSP Play
RTSP Publish → [depacketize → normalize → AVFrame] → Engine Ring Buffer → [AVFrame → mux] → RTMP Play
```

但对比 ZLMediaKit 的 `MultiMediaSourceMuxer` 架构，以下能力存在缺口或需要增强。

---

## 与 ZLMediaKit 对比后的主要缺口

| 能力 | ZLMediaKit 参考点 | 本地状态 | 计划处理 |
|------|-------------------|----------|----------|
| 跨协议 GOP 秒开（RTMP→RTSP） | PacketCache + RingBuffer GOP 边界标记 | ⚠️ 部分（bootstrap 仅限同协议格式） | Phase 01 |
| 跨协议 GOP 秒开（RTSP→RTMP） | 新订阅者从 keyframe 开始 + 序列头补发 | ⚠️ 部分（缺少跨协议参数集转换验证） | Phase 01 |
| 音视频 Track 同步等待 | MediaSink 等待所有 Track ready 后才分发 | ❌ 未实现（当前逐帧分发） | Phase 01 |
| 跨协议时间戳精度保持 | Stamp 类 ms↔RTP ticks 双向转换 + syncTo | ⚠️ 部分（有 EgressAdapterView 但缺 A/V sync） | Phase 02 |
| DTS 生成（PTS-only 源） | DtsGenerator 排序窗口生成 DTS | ⚠️ 部分（TimestampNormalizer 有基础 DTS 生成） | Phase 02 |
| B 帧 PTS 回退处理 | Stamp enableRollback + DtsGenerator | ⚠️ 部分（有 B_FRAME flag 但 egress 未特殊处理） | Phase 02 |
| 跨协议时间戳同步（A/V 对齐） | Stamp::syncTo() 音频同步到视频 | ❌ 未实现 | Phase 02 |
| 非标准 RTMP metadata 容错 | loadMetaData 失败后从首包推断 codec | ✅ 已有（从首包推断） | — |
| 非标准 SDP 兼容 | compat.rs quirks 处理 | ✅ 已有（strip_sdp_suffix 等） | — |
| 静音音频注入（跨协议） | MuteAudioMaker 视频流自动补 AAC | ⚠️ 部分（RTMP 有，RTSP egress 未验证） | Phase 03 |
| G.711↔AAC 跨协议转码 | Factory 插件 + 转码 muxer | ❌ 未实现 | Phase 03 |
| Opus↔AAC 跨协议转码 | 同上 | ❌ 未实现 | Phase 03 |
| 按需 muxing（无订阅者不转换） | rtsp_demand/rtmp_demand 选项 | ❌ 未实现（当前始终分发） | Phase 04 |
| 直接代理模式（同协议零转码） | directProxy 跳过 demux/remux | ✅ 已有（RTMP side_data 零拷贝） | — |
| 跨协议 Track 动态变更通知 | Track 变更时重新生成 SDP/序列头 | ⚠️ 部分（RTMP 有 track 变更检测） | Phase 01 |
| RTCP 反馈驱动关键帧请求 | PLI/FIR → 请求发布者发送 IDR | ❌ 未实现 | Phase 04 |
| 非标准 H.265 RTMP→RTSP 转换 | Enhanced RTMP HEVC → RTP HEVC | ⚠️ 部分（各自支持但跨协议路径未端到端验证） | Phase 01 |
| 非标准 AV1 RTMP→RTSP 转换 | Enhanced RTMP AV1 → RTP AV1 | ⚠️ 部分（同上） | Phase 01 |

---

## 总体约束

1. 严格遵循 `core + driver + module` 三段式架构
2. 协议转换通过共享 `AVFrame + TrackInfo` 模型实现，不引入直接的 RTMP→RTSP 转换路径
3. 时间戳转换和同步逻辑统一放在 `cheetah-codec`
4. 参数集转换（AVCC↔Annex-B、SDP↔序列头）统一放在 `cheetah-codec`
5. 音频转码作为独立可选模块，不在协议热路径中
6. 所有新增能力默认启用合理行为，通过配置可调

---

## 参考来源

| 来源 | 路径 |
|------|------|
| ZLMediaKit 协议转换中枢 | `vendor-ref/ZLMediaKit/src/Common/MultiMediaSourceMuxer.h/.cpp` |
| ZLMediaKit 时间戳处理 | `vendor-ref/ZLMediaKit/src/Common/Stamp.h/.cpp` |
| ZLMediaKit GOP 缓存 | `vendor-ref/ZLMediaKit/src/Common/PacketCache.h` |
| ZLMediaKit RTMP 解复用 | `vendor-ref/ZLMediaKit/src/Rtmp/RtmpDemuxer.h/.cpp` |
| ZLMediaKit RTMP 复用 | `vendor-ref/ZLMediaKit/src/Rtmp/RtmpMuxer.h/.cpp` |
| ZLMediaKit RTSP 解复用 | `vendor-ref/ZLMediaKit/src/Rtsp/RtspDemuxer.h/.cpp` |
| ZLMediaKit RTSP 复用 | `vendor-ref/ZLMediaKit/src/Rtsp/RtspMuxer.h/.cpp` |
| ZLMediaKit Track 就绪 | `vendor-ref/ZLMediaKit/src/Common/MediaSink.h/.cpp` |
| ZLMediaKit Codec 工厂 | `vendor-ref/ZLMediaKit/src/Extension/Factory.h/.cpp` |
| 本项目 RTMP 模块 | `crates/protocols/rtmp/` |
| 本项目 RTSP 模块 | `crates/protocols/rtsp/` |
| 本项目媒体内核 | `crates/foundation/codec/` |
| 本项目前序计划 | `dev-docs/plans-13/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [phase-01-cross-protocol-gop-and-track.md](phase-01-cross-protocol-gop-and-track.md) | ✅ 已完成 | 跨协议 GOP 秒开、Track 就绪同步、非标准编码器端到端验证 |
| [phase-02-timestamp-precision.md](phase-02-timestamp-precision.md) | ✅ 已完成 | 跨协议时间戳精度、A/V 同步、B 帧处理、DTS 生成增强 |
| [phase-03-audio-compat.md](phase-03-audio-compat.md) | ✅ 已完成 | 跨协议静音注入、G.711↔AAC 转码、Opus↔AAC 转码 |
| [phase-04-advanced-features.md](phase-04-advanced-features.md) | ✅ 已完成 | 按需 muxing、RTCP 反馈驱动 IDR 请求、性能优化 |

---

## 任务状态总表

| 阶段 | 任务 | 状态 |
|------|------|------|
| 1.1 | 跨协议 GOP 秒开：RTMP 推流 → RTSP 拉流秒开 | ✅ 已完成 |
| 1.2 | 跨协议 GOP 秒开：RTSP 推流 → RTMP 拉流秒开 | ✅ 已完成 |
| 1.3 | Track 就绪同步等待机制 | ✅ 已完成 |
| 1.4 | H.265/AV1/VP9 跨协议端到端验证与修复 | ✅ 已完成 |
| 1.5 | 跨协议 Track 动态变更通知 | ✅ 已完成 |
| 2.1 | 跨协议时间戳精度保持（ms↔RTP ticks 无损转换） | ✅ 已完成 |
| 2.2 | 跨协议 A/V 时间戳同步对齐 | ✅ 已完成 |
| 2.3 | B 帧 PTS 回退在 RTSP egress 的正确处理 | ✅ 已完成 |
| 2.4 | PTS-only 源的 DTS 生成增强 | ✅ 已完成 |
| 2.5 | 时间戳断流恢复与连续性保证 | ✅ 已完成 |
| 3.1 | 跨协议静音音频注入（RTSP egress 验证） | ✅ 已完成 |
| 3.2 | G.711A/U↔AAC 跨协议实时转码 | ✅ 已完成 |
| 3.3 | Opus↔AAC 跨协议实时转码 | ✅ 已完成 |
| 4.1 | 按需 muxing（无订阅者不执行协议转换） | ⏭️ 跳过（本地已是按需模式） |
| 4.2 | RTCP PLI/FIR 驱动关键帧请求 | ✅ 已完成 |
| 4.3 | 跨协议转换性能基准与优化 | ⏭️ 延后（需要压测环境） |

---

## 渐进式执行顺序

1. **Phase 01** — GOP 秒开与 Track 同步：直接提升跨协议首帧延迟和播放体验
2. **Phase 02** — 时间戳精度：解决长时间播放的音画同步和时间戳漂移问题
3. **Phase 03** — 音频兼容：解决 G.711/Opus 设备跨协议播放问题
4. **Phase 04** — 高级特性：性能优化和生产环境增强
