# TS 协议完善计划（对标 ZLMediaKit）

- **状态**: ✅ 完成
- **目标**: 在现有 TS 协议实现基础上，对标 ZLMediaKit 的 HTTP-TS、WebSocket-TS、TS mux/demux、TS 拉流和媒体源缓存机制，补齐标准能力与真实落地兼容性
- **方法**: 对比 `vendor-ref/ZLMediaKit/src` 中 `Http/HttpSession.*`、`Http/HttpTSPlayer.*`、`Http/TsPlayer*`、`TS/TSMediaSource*`、`Record/MPEG.*`、`Http/WebSocketSplitter.*`，结合本项目现有 `cheetah-ts-*` 与 `cheetah-codec` 实现逐项完善
- **完成标准**: HTTP(S)-TS、WS(S)-TS 播放和远端拉流通过单元/集成/端到端测试，并用 ffmpeg/ffplay/VLC/ZLMediaKit 样例验证 H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1/MP2、多轨道和故障输入场景

---

## V1 完善范围

本轮只完善已存在的 TS 协议能力，不重新设计协议三段式结构。

1. 本地流通过 HTTP-TS / HTTPS-TS 播放
2. 本地流通过 WS-TS / WSS-TS 播放
3. 远端 HTTP(S)-TS / WS(S)-TS 拉流并发布为本地 engine stream
4. H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1/MP2 编码封装与解封装
5. 多 video/audio elementary stream 的多轨道模式
6. 真实客户端兼容：`.ts` 与 ZLMediaKit 风格 `.live.ts` 路径、宽松 Content-Type、chunked body、WebSocket binary frame、PAT/PMT 周期补发、慢客户端隔离

本轮不做：

1. 客户端通过 HTTP POST 或 WebSocket binary 向本服务推 TS
2. TS 录制、DVR、VOD 文件服务
3. 多节目 MPTS 的业务编排；demux 可解析多个 program，但 module 首版只发布第一个可用 program 或显式选择的 program
4. 转码；不兼容播放器的编码只保证稳定转封装、诊断和不拖垮会话

---

## 与 ZLMediaKit 对比后的主要缺口

| 能力 | ZLMediaKit 参考 | 本地状态 | 计划处理 |
|------|----------------|----------|----------|
| TS 媒体源 ring/cache | `TSMediaSource` + `PacketCache<TSPacket>` | ⚠️ 依赖 engine ring，无 TS packet 缓存策略 | Phase 03 |
| 按需 TS mux | `TSMediaSourceMuxer::onReaderChanged()` + `ts_demand` | ❌ 未实现 | Phase 03 |
| HTTP-TS URL 兼容 | `HttpSession::checkLiveStreamTS()` 使用 `.live.ts` | ⚠️ 仅 `.ts` | Phase 02 |
| WebSocket-TS 握手 | `HttpSession::checkWebSocket()` 先回复 101 | ⚠️ 有握手，需补 header/行为 | Phase 02 |
| WS binary 封帧 | `HttpSession::onWrite()` + `WebSocketSplitter::encode()` | ❌ 目前存在裸 TS 风险 | Phase 02 |
| WS close/ping/pong | `WebSocketSplitter` decode/encode | ⚠️ core 有状态，driver 待闭环 | Phase 02 |
| HTTP 响应头 | `sendResponse(200, no_content_length=true)` | ⚠️ 基础可用，需补 `.live.ts`/close/HEAD 语义 | Phase 02 |
| 播放性能优化 | `setSocketFlags()`、批量 flush | ❌ 未显式支持 | Phase 02/03 |
| HTTP-TS 拉流状态 | `TsPlayer::onResponseBody()` 首包即成功 | ⚠️ 有 pull，但状态语义不足 | Phase 03 |
| 拉流 Content-Type 宽松判断 | `HttpTSPlayer::onResponseHeader()` | ❌ 仅状态码判断 | Phase 03 |
| 拉流 200/206 兼容 | `HttpTSPlayer` 接受 200/206 | ❌ 只接受 200 | Phase 03 |
| 空 body 失败 | `TsPlayer::onResponseCompleted()` | ❌ 未实现 | Phase 03 |
| TS 拉流 demux flush | `TsPlayerImp::onShutdown()` flush decoder | ⚠️ demux flush 需要 module 接入 | Phase 03 |
| MPEG muxer 多轨道 | `MpegMuxer::_tracks` 按 frame index 映射 | ⚠️ 已有多轨雏形，需补边界 | Phase 01 |
| H264/H265 同时间戳合帧 | `FrameMerger` 合并 SPS/PPS/IDR | ⚠️ 依赖上游 canonical frame | Phase 01 |
| AAC ADTS 要求 | `CHECK(frame->prefixSize())` | ⚠️ mux 需明确 ADTS 包装 | Phase 01 |
| stream_type 矩阵 | `Frame.h CODEC_MAP` + `mpeg-proto.h` | ⚠️ 已覆盖大半，需校准 | Phase 01 |
| PMT private descriptor | `libmpeg` Opus/AV1/VPx 私有兼容 | ⚠️ 部分支持，需测试闭环 | Phase 01 |
| PAT/PMT/PCR 补发 | `mpeg_muxer_input()` 自动输出表 | ⚠️ 初始发送，周期补发待接入 | Phase 03 |
| continuity / sync 诊断 | `TSDecoder`/libmpeg 容错 | ⚠️ 有诊断，需更完整故障样例 | Phase 04 |
| 背压隔离 | RingBuffer 读者 detach | ⚠️ engine subscriber 有界队列，driver 慢客户端策略待测 | Phase 04 |

---

## 标准与非标准兼容点

### 标准 TS 基线

- TS packet 固定 188 字节，sync byte `0x47`
- 支持 PAT、PMT、PES、adaptation field、PCR、PTS/DTS
- 支持 null packet PID `0x1FFF`
- 支持 continuity counter wrap 与缺包诊断
- 支持 PTS/DTS 33-bit wrap
- PCR 默认使用首个视频轨；无视频时使用首个音频轨
- PES length 为 0 的视频流按下一 PES start 或 flush 输出

### ZLMediaKit / libmpeg 兼容优先

- HTTP-TS 播放路径兼容 `/{app}/{stream}.ts` 和 `/{app}/{stream}.live.ts`
- WebSocket-TS 使用标准 101 upgrade，payload 必须用 binary frame 承载 TS bytes
- HTTP-TS 拉流接受状态码 200/206
- Content-Type 接受 `video/mp2t`、`video/mpeg`、`application/octet-stream`，其它类型只告警不立即拒绝
- 首次收到 TS body 才认为拉流成功；连接正常 EOF 但未收到 body 视为失败
- G711A 使用 stream_type `0x90`，G711U 使用 `0x91`
- OPUS 输入兼容 stream_type `0x9C`，输出优先 `0x06` + registration descriptor `"Opus"`
- VP8 使用 `0x9D`，VP9 使用 `0x9E`，AV1 使用 `0x9F`
- MP2 使用 `0x03`，MP3 使用 `0x04`，输入侧兼容历史流中 MP3/MP2 混用
- PAT/PMT CRC 默认宽松诊断，strict 模式才拒绝
- 输入允许前导垃圾、粘包、半包、HTTP chunked 切片和 TS sync 丢失后重同步

---

## 总体约束

1. 严格遵守 `core + driver + module` 三段式架构
2. MPEG-TS mux/demux、时间戳、参数集、ADTS、codec stream_type 映射归属 `cheetah-codec`
3. `cheetah-ts-core` 保持 Sans-I/O，只处理 HTTP/WS 请求状态、事件和输出动作
4. `cheetah-ts-driver-tokio` 负责 socket、TLS、HTTP/1.1、WebSocket framing、chunked、写队列和 backpressure
5. `cheetah-ts-module` 负责 engine 订阅/发布、播放会话、拉流任务、配置和鉴权
6. module 不直接使用 `tokio::*` 公共类型或 `tokio::spawn`；任务派生统一通过 `RuntimeApi`
7. 所有远端 TS 输入进入 engine 前必须转换为 `AVFrame + TrackInfo`
8. 所有缓存、队列、reassembly buffer、WebSocket frame buffer 必须有上界

---

## 参考来源

| 来源 | 路径 |
|------|------|
| ZLM HTTP live 路由 | `vendor-ref/ZLMediaKit/src/Http/HttpSession.cpp` |
| ZLM WebSocket 编解码 | `vendor-ref/ZLMediaKit/src/Http/WebSocketSplitter.*` |
| ZLM HTTP-TS 拉流 | `vendor-ref/ZLMediaKit/src/Http/HttpTSPlayer.*` |
| ZLM TS 播放器 | `vendor-ref/ZLMediaKit/src/Http/TsPlayer.*`、`TsPlayerImp.*` |
| ZLM TS 媒体源 | `vendor-ref/ZLMediaKit/src/TS/TSMediaSource.h` |
| ZLM TS 媒体源 muxer | `vendor-ref/ZLMediaKit/src/TS/TSMediaSourceMuxer.h` |
| ZLM MPEG muxer | `vendor-ref/ZLMediaKit/src/Record/MPEG.*` |
| ZLM codec/stream_type | `vendor-ref/ZLMediaKit/src/Extension/Frame.*`、`3rdpart/media-server/libmpeg/include/mpeg-proto.h` |
| 本项目 TS 实现 | `crates/protocols/ts/` |
| 本项目共享 TS 容器 | `crates/foundation/cheetah-codec/src/ts_*.rs` |
| 本项目 HTTP 长连接参考 | `crates/protocols/http-flv/`、`crates/protocols/hls/driver-tokio/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [ts-architecture.md](ts-architecture.md) | 未开始 | TS 总体架构、crate 边界、ZLM 对照数据流、配置与运行模型 |
| [phase-01-codec-ts-container.md](phase-01-codec-ts-container.md) | ✅ 完成 | `cheetah-codec` MPEG-TS mux/demux、codec matrix、时间戳与容错 |
| [phase-02-core-driver-transport.md](phase-02-core-driver-transport.md) | ✅ 完成 | `cheetah-ts-core`、HTTP/HTTPS、WS/WSS、WebSocket framing、传输背压 |
| [phase-03-module-play-pull.md](phase-03-module-play-pull.md) | ✅ 完成 | TS module 播放输出、远端拉流发布、按需 mux、多轨道编排 |
| [phase-04-compat-interop-testing.md](phase-04-compat-interop-testing.md) | ✅ 完成 | ZLM/ffmpeg/VLC 互操作、故障样例、fuzz、性能和文档同步 |

---

## 任务状态总表

| 阶段 | 任务 | 状态 |
|------|------|------|
| 1.1 | 校准 stream_type / descriptor / codec matrix | ✅ 完成 |
| 1.2 | 补齐 AAC ADTS、H26x AUD/参数集、MP2/MP3 边界 | ✅ 完成 |
| 1.3 | PAT/PMT/PES/PCR/PTS/DTS 鲁棒性增强 | ✅ 完成 |
| 1.4 | demux 重同步、CRC、continuity、reassembly 上限 | ✅ 完成 |
| 1.5 | HLS 私有 TS 实现收敛到共享 `cheetah-codec` | ✅ 完成 |
| 2.1 | `.live.ts` 路径、HEAD/OPTIONS/GET 语义修正 | ✅ 完成 |
| 2.2 | WebSocket binary frame 发送与 close/ping/pong | ✅ 完成 |
| 2.3 | HTTPS/WSS TLS listener 接线 | ✅ 完成 |
| 2.4 | HTTP chunked、keep-alive、慢客户端写队列策略 | ✅ 完成 |
| 2.5 | HTTP(S)/WS(S)-TS pull 传输层完善 | ✅ 完成 |
| 3.1 | 本地 stream 订阅、PAT/PMT 周期补发、bootstrap | ✅ 完成 |
| 3.2 | 远端 TS pull demux 后发布到 engine | ✅ 完成 |
| 3.3 | 多轨道 `update_tracks` 累积与轨道变化处理 | ✅ 完成 |
| 3.4 | ZLM `ts_demand` 风格按需 mux 策略 | ✅ 完成 |
| 3.5 | 配置校验、service registry、会话清理 | ✅ 完成 |
| 4.1 | ZLM/ffmpeg fixture corpus | ✅ 完成 |
| 4.2 | codec matrix 与多轨互操作测试 | ✅ 完成 |
| 4.3 | fault robustness 和 fuzz/property tests | ✅ 完成 |
| 4.4 | 背压、慢客户端、内存上限、性能测试 | ✅ 完成 |
| 4.5 | `SystemArchitecture.md` 与 README/配置文档同步 | ✅ 完成 |

---

## 渐进式执行顺序

1. **Phase 01** — 先把 TS 容器能力收敛在 `cheetah-codec`，避免 HLS/TS 双实现继续分叉
2. **Phase 02** — 修正 core/driver 传输语义，确保 HTTP-TS 与 WS-TS 在真实客户端可用
3. **Phase 03** — 接入 engine 播放和拉流业务，补齐多轨道、周期 PAT/PMT、按需 mux
4. **Phase 04** — 用 ZLMediaKit、ffmpeg、VLC 和故障样例完成真实互操作与鲁棒性闭环
