# TS 协议实现计划（对标 SimpleMediaServer）

- **状态**: Phase 01-03 完成，Phase 04 待开始
- **目标**: 新增独立 TS 协议能力，支持 HTTP(S)-TS / WS(S)-TS 直播播放与远端拉流，复用统一媒体模型 `AVFrame + TrackInfo`
- **方法**: 参考 `vendor-ref/simple-media-server/Src/Mpeg`、`vendor-ref/simple-media-server/Src/HttpStream/HttpTsClient.*`、`vendor-ref/simple-media-server/Src/Http/Websocket*`，结合本项目现有 HLS TS mux/demux 与 HTTP-FLV 传输实现
- **完成标准**: 标准 MPEG-TS mux/demux、HTTP/WS TS 播放、远端 TS 拉流均通过单元/集成/端到端测试，并通过 ffplay/VLC/SMS 样例互操作验证

---

## V1 范围

首版固定支持：

1. 本地流通过 HTTP-TS / HTTPS-TS 播放
2. 本地流通过 WS-TS / WSS-TS 播放
3. 远端 HTTP(S)-TS / WS(S)-TS 拉流并发布为本地 engine stream
4. H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1/MP2 编码的 TS 封装与解封装
5. 多轨道模式，支持多个 video/audio elementary stream

首版不做：

1. HTTP POST / WebSocket binary 客户端推 TS 到本服务端
2. TS 文件录制、回看、DVR
3. 多节目 MPTS 的业务编排；demux 可识别多 program，但 module 首版只发布第一个可用 program
4. 转码；不被 TS 或播放器原生支持的编码仅做稳定传输和诊断

---

## 与 SimpleMediaServer 对比后的主要缺口

| 能力 | SMS 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| 独立 TS 协议媒体源 | `Mpeg/TsMediaSource.*` | ❌ 无独立 TS module | Phase 03 |
| HTTP-TS 拉流 | `HttpStream/HttpTsClient.*` | ❌ 无 | Phase 02/03 |
| HTTP-TS 播放 | `TsMediaSource` ring + HTTP 输出 | ❌ 无独立输出 | Phase 02/03 |
| WebSocket TS 播放 | `Http/Websocket*` + source ring | ❌ 无 | Phase 02/03 |
| HTTPS/WSS TS | HTTP TLS/WebSocket 基础能力 | ⚠️ HLS/HTTP-FLV 有类似实现 | Phase 02 |
| MPEG-TS mux | `Mpeg/TsMuxer.*` | ⚠️ HLS core 内已有局部实现 | Phase 01 |
| MPEG-TS demux | `Mpeg/TsDemuxer.*` | ⚠️ HLS core 内已有局部实现 | Phase 01 |
| 多编码 stream_type | `Mpeg/Mpeg.h` | ⚠️ 已部分覆盖，缺 MP2 类型 | Phase 01 |
| 多轨道 PID 分配 | `TsMuxer::_mapStreamId` | ⚠️ HLS 有多轨雏形 | Phase 01 |
| continuity counter 诊断 | `TsPacket::demux()` | ⚠️ 需要系统化 | Phase 01 |
| 未对齐 TS 输入重同步 | `HttpTsClient::onRecvContent()` + demux | ⚠️ HLS demux 有基础实现 | Phase 01 |
| HTTP chunked 输入 | `HttpChunkedParser` | ⚠️ HTTP-FLV pull 有类似能力 | Phase 02 |
| PAT/PMT 周期补发 | `TsMuxer::_first/keyframe` | ⚠️ HLS segment 起始补发 | Phase 01/03 |
| H264/H265 AUD 注入 | `TsMuxer::make_pes_packet()` | ✅ HLS TS 已有 | Phase 01 抽取复用 |
| AAC ADTS 封装 | TS PES AAC 期望 ADTS | ✅ `cheetah-codec` 已有 helper | Phase 01 抽取复用 |
| 参数集补发 | SMS 依赖 frame/track | ✅ HLS 已有 | Phase 01 抽取复用 |

---

## 标准与非标准兼容点

### 标准 TS 基线

- TS packet 固定 188 字节，sync byte `0x47`
- 支持 PAT、PMT、PES、adaptation field、PCR、PTS/DTS
- 支持 null packet PID `0x1FFF`
- 支持 continuity counter wrap 与缺包诊断
- 支持 PTS/DTS 33-bit wrap
- PCR 默认使用首个视频轨；无视频时使用首个音频轨

### 落地非标准 / 兼容优先

- G711A 使用 stream_type `0x90`，G711U 使用 `0x91`
- VP8 使用 `0x9D`，VP9 使用 `0x9E`，AV1 使用 `0x9F`
- Opus 输出优先使用 private stream `0x06` + registration descriptor `"Opus"`；输入同时兼容 SMS 的 `0x9C`
- MP2 输入兼容 `0x03`，MP3 输入兼容 `0x03/0x04`；输出 MP2 使用 `0x03`、MP3 使用 `0x04`
- PAT/PMT CRC 错误默认记录兼容告警并继续；strict 模式可拒绝
- 输入数据允许任意切片、粘包、半包和前导垃圾，demux 负责重同步
- PES length 为 0 的视频流按下一 PES start 或流结束 flush
- H264/H265 PES 前补 AUD，关键帧前补参数集，提高播放器兼容性

---

## 总体约束

1. 严格遵守 `core + driver + module` 三段式架构
2. MPEG-TS 容器 mux/demux 属于 `cheetah-codec`，不要只放在 HLS 或 TS 私有实现里
3. `cheetah-ts-core` 只处理 Sans-I/O HTTP/WS 请求状态、事件和输出动作，不依赖 Tokio、socket、engine
4. `cheetah-ts-driver-tokio` 负责 socket、TLS、HTTP/1.1、WebSocket、chunked、写队列、backpressure
5. `cheetah-ts-module` 负责 engine 订阅/发布、会话、拉流任务、鉴权和配置
6. module 公共接口不得暴露 `tokio::*` 或 `tokio_util::*`
7. 所有进入 engine 的远端 TS 内容必须收敛为 `AVFrame + TrackInfo`
8. 多轨道、缓存、队列和重同步 buffer 必须有上界

---

## 参考来源

| 来源 | 路径 |
|------|------|
| SMS TS Muxer | `vendor-ref/simple-media-server/Src/Mpeg/TsMuxer.*` |
| SMS TS Demuxer | `vendor-ref/simple-media-server/Src/Mpeg/TsDemuxer.*` |
| SMS stream type 常量 | `vendor-ref/simple-media-server/Src/Mpeg/Mpeg.h` |
| SMS TS MediaSource | `vendor-ref/simple-media-server/Src/Mpeg/TsMediaSource.*` |
| SMS HTTP-TS Client | `vendor-ref/simple-media-server/Src/HttpStream/HttpTsClient.*` |
| SMS WebSocket | `vendor-ref/simple-media-server/Src/Http/Websocket*` |
| 本项目 HLS TS 实现 | `crates/protocols/hls/core/src/ts_mux.rs`、`ts_demux.rs` |
| 本项目 HTTP/WS 长连接参考 | `crates/protocols/http-flv/` |
| 本项目 TLS HTTP 参考 | `crates/protocols/hls/driver-tokio/src/tls.rs` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [ts-architecture.md](ts-architecture.md) | ✅ 完成 | TS 总体架构、crate 边界、数据流、路由、兼容策略 |
| [phase-01-codec-ts-container.md](phase-01-codec-ts-container.md) | ✅ 完成 | `cheetah-codec` MPEG-TS mux/demux、MP2、鲁棒性、fuzz |
| [phase-02-core-driver-transport.md](phase-02-core-driver-transport.md) | ✅ 完成 | `cheetah-ts-core`、HTTP/HTTPS、WS/WSS driver、pull client |
| [phase-03-module-play-pull.md](phase-03-module-play-pull.md) | ✅ 完成 | TS module 播放输出、远端拉流发布、多轨道接入 |
| [phase-04-compat-interop-testing.md](phase-04-compat-interop-testing.md) | 规划中 | SMS/ffmpeg/VLC 互操作、故障样例、性能与运维验证 |

---

## 任务状态总表

| 阶段 | 任务 | 状态 |
|------|------|------|
| 1.1 | 新增 `CodecId::MP2` / `FrameFormat::Mp2Frame` | ✅ 完成 |
| 1.2 | 抽取共享 MPEG-TS muxer 到 `cheetah-codec` | ✅ 完成 |
| 1.3 | 抽取共享 MPEG-TS demuxer 到 `cheetah-codec` | ✅ 完成 |
| 1.4 | stream_type 标准与非标准映射矩阵 | ✅ 完成 |
| 1.5 | PAT/PMT/PCR/PTS/DTS/continuity 鲁棒性测试 | ✅ 完成 |
| 1.6 | HLS 改为复用共享 TS 容器 API | ✅ 完成 |
| 2.1 | 新增 `cheetah-ts-core` Sans-I/O 请求/会话状态机 | ✅ 完成 |
| 2.2 | 新增 `cheetah-ts-driver-tokio` HTTP/WS server | ✅ 完成 |
| 2.3 | 新增 HTTPS/WSS TLS server | ✅ 完成 |
| 2.4 | 新增 HTTP(S)/WS(S)-TS pull client | ✅ 完成 |
| 2.5 | core/driver 单元与集成测试 | ✅ 完成 |
| 3.1 | 新增 `cheetah-ts-module` factory/config/schema | ✅ 完成 |
| 3.2 | 本地 stream 订阅并输出 TS | ✅ 完成 |
| 3.3 | 远端 TS 拉流并发布到 engine | ✅ 完成 |
| 3.4 | 多轨道播放/拉流编排 | ✅ 完成 |
| 3.5 | app feature 与 service registry 接入 | ✅ 完成 |
| 4.1 | SMS fixture 与 fault corpus | 未开始 |
| 4.2 | ffmpeg/ffplay/VLC 互操作矩阵 | 未开始 |
| 4.3 | fuzz/property-tests | 未开始 |
| 4.4 | 性能、背压、慢客户端验证 | 未开始 |
| 4.5 | 文档与架构同步 | 未开始 |

---

## 渐进式执行顺序

1. **Phase 01** — 先把 TS 容器能力收敛到 `cheetah-codec`，避免 HLS/TS 双实现分叉
2. **Phase 02** — 建立 TS core/driver 传输壳，先跑通 HTTP/WS 长连接与 pull client
3. **Phase 03** — 接入 engine，实现本地播放输出与远端拉流发布
4. **Phase 04** — 用 SMS/ffmpeg/VLC 和故障样例补齐真实互操作性与鲁棒性
