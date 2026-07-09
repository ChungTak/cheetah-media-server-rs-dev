# fMP4 协议实现计划（对标 SimpleMediaServer）

- **状态**: 规划中
- **目标**: 新增独立 fMP4 协议能力，支持 HTTP(S)-fMP4 / WS(S)-fMP4 直播播放与远端拉流，复用统一媒体模型 `AVFrame + TrackInfo`
- **方法**: 参考 `vendor-ref/simple-media-server/Src/Mp4`、`vendor-ref/simple-media-server/Src/Http/HttpConnection.cpp`、`vendor-ref/simple-media-server/Src/Http/Websocket*`，结合本项目现有 HLS fMP4 mux/demux、TS/HTTP-FLV 传输实现
- **完成标准**: 标准 ISO BMFF/fMP4 mux/demux、HTTP/WS fMP4 播放、远端 fMP4 拉流均通过单元/集成/端到端测试，并通过 ffplay/VLC/SMS 样例互操作验证

---

## V1 范围

首版固定支持：

1. 本地流通过 HTTP-fMP4 / HTTPS-fMP4 播放
2. 本地流通过 WS-fMP4 / WSS-fMP4 播放
3. 远端 HTTP(S)-fMP4 / WS(S)-fMP4 拉流并发布为本地 engine stream
4. H264/H265/AAC/G711/OPUS/MJPEG/MP3/VP8/VP9/AV1/MP2 编码的 fMP4 封装与解封装
5. 多轨道模式，支持多个 video/audio track

首版不做：

1. VOD MP4 文件录制、回看、DVR
2. HTTP POST / WebSocket binary 客户端推 fMP4 到本服务端
3. 转码；不被 fMP4 或播放器原生支持的编码仅做稳定传输和诊断
4. DASH manifest、MSE player 页面、CMAF low-latency blocking playlist
5. 多 period / 多 presentation 的业务编排

---

## 与 SimpleMediaServer 对比后的主要缺口

| 能力 | SMS 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| 独立 HTTP-fMP4 协议媒体源 | `Mp4/Fmp4MediaSource.*` | 无独立 fMP4 module | Phase 03 |
| HTTP `.mp4` 直播播放 | `HttpConnection::handleFmp4()` | 无 | Phase 02/03 |
| WebSocket `.mp4` 直播播放 | `HttpConnection` + WebSocket | 无 | Phase 02/03 |
| HTTPS/WSS fMP4 | HTTP TLS/WebSocket 基础能力 | TS/HLS/HTTP-FLV 有类似实现 | Phase 02 |
| fMP4 mux | `Mp4/Fmp4Muxer.*` | HLS core 内已有局部实现 | Phase 01 |
| fMP4 demux | `Mp4/Fmp4Demuxer.*` | HLS core 内已有局部实现 | Phase 01 |
| MP4 box parser/writer | `Mp4/Mp4Box.*` | 分散在 HLS fMP4 实现 | Phase 01 |
| codec sample entry 矩阵 | `Mp4Box.cpp` / `Mp4Muxer.cpp` | 缺 MJPEG/MP2，部分编码不完整 | Phase 01 |
| H26x length-prefixed 样本 | `Fmp4Muxer::inputFrame_l()` | HLS 已有 helper | Phase 01 |
| init segment + media fragment 长连接 | `onPlayFmp4()` | 无独立长连接输出 | Phase 03 |
| ring / 慢客户端隔离 | `Fmp4MediaSource::RingType` | engine 订阅已有基础能力 | Phase 03 |
| HTTP chunked fMP4 拉流 | SMS HTTP 客户端形态 | 无 | Phase 02/03 |
| WS binary fMP4 拉流 | SMS WebSocket 基础能力 | 无 | Phase 02/03 |

---

## 标准与非标准兼容点

### 标准 fMP4 基线

- init segment 使用 `ftyp + moov`，`moov` 内包含 `mvex/trex`
- media segment 使用 `styp + sidx + moof + mdat` 或兼容 `moof + mdat`
- 每个 `traf` 包含 `tfhd`、`tfdt`、`trun`
- `tfhd` 使用 `default-base-is-moof`
- `trun` 使用 `data-offset`，B-frame 使用 signed composition time offset
- H264/H265/H266 样本进入 MP4 前必须是 4 字节 length-prefixed NALU
- 时间戳使用 track timescale，输出来源为 canonical `AVFrame.pts_us/dts_us`

### 落地非标准 / 兼容优先

- SMS 使用 `.mp4` 暴露 HTTP/WS fMP4 直播，首版兼容该 URL 形态
- HTTP 播放先发送 init segment，再持续发送 media fragment；非 WebSocket 默认 chunked
- `styp/sidx` 可配置输出；输入端必须兼容无 `styp`、无 `sidx` 的 `moof+mdat`
- H265 输入兼容 `hvc1/hev1/dvh1/dvhe`；输出默认 `hvc1`
- H264 输入兼容 `avc1/avc2/avc3/avc4`；输出默认 `avc1`
- MP2 使用 `mp4a + esds ObjectType 0x6B`；MP3 输出使用 `ObjectType 0x69`，输入兼容 `0x69/0x6B`
- MJPEG 输出默认 `mp4v + esds ObjectType 0x6C`；输入兼容 QuickTime `jpeg/mjpa/mjpb`
- G711A/G711U 使用 `alaw/ulaw`
- Opus 使用 `Opus + dOps`
- VP8/VP9 使用 `vp08/vp09 + vpcC`；AV1 使用 `av01 + av1C`
- 输入允许任意切片、粘包、半包、重复 init、unknown box；demux 负责有界缓存与诊断

---

## 总体约束

1. 严格遵守 `core + driver + module` 三段式架构
2. ISO BMFF/fMP4 容器 mux/demux 属于 `cheetah-codec`，不要只放在 HLS 或 fMP4 私有实现里
3. `cheetah-fmp4-core` 只处理 Sans-I/O HTTP/WS 请求状态、事件和输出动作，不依赖 Tokio、socket、engine
4. `cheetah-fmp4-driver-tokio` 负责 socket、TLS、HTTP/1.1、WebSocket、chunked、写队列、backpressure
5. `cheetah-fmp4-module` 负责 engine 订阅/发布、会话、拉流任务、鉴权和配置
6. module 公共接口不得暴露 `tokio::*` 或 `tokio_util::*`
7. 所有进入 engine 的远端 fMP4 内容必须收敛为 `AVFrame + TrackInfo`
8. 多轨道、缓存、队列和 box reassembly buffer 必须有上界

---

## 参考来源

| 来源 | 路径 |
|------|------|
| SMS fMP4 muxer | `vendor-ref/simple-media-server/Src/Mp4/Fmp4Muxer.*` |
| SMS fMP4 demuxer | `vendor-ref/simple-media-server/Src/Mp4/Fmp4Demuxer.*` |
| SMS fMP4 media source | `vendor-ref/simple-media-server/Src/Mp4/Fmp4MediaSource.*` |
| SMS MP4 box / codec tag | `vendor-ref/simple-media-server/Src/Mp4/Mp4Box.*` |
| SMS HTTP fMP4 route | `vendor-ref/simple-media-server/Src/Http/HttpConnection.cpp` |
| 本项目 HLS fMP4 实现 | `crates/protocols/hls/core/src/fmp4_mux.rs`、`fmp4_demux.rs` |
| 本项目 TS HTTP/WS 参考 | `crates/protocols/ts/` |
| 本项目 HTTP-FLV pull 参考 | `crates/protocols/http-flv/module/src/pull.rs` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [fmp4-architecture.md](fmp4-architecture.md) | 规划中 | fMP4 总体架构、crate 边界、数据流、路由、兼容策略 |
| [fmp4-sms-gap-analysis.md](fmp4-sms-gap-analysis.md) | 规划中 | SMS 行为、标准/非标准落地点、本项目缺口 |
| [phase-01-codec-fmp4-container.md](phase-01-codec-fmp4-container.md) | 规划中 | `cheetah-codec` fMP4 mux/demux、MJPEG、MP2、HLS 复用 |
| [phase-02-core-driver-transport.md](phase-02-core-driver-transport.md) | 规划中 | `cheetah-fmp4-core`、HTTP/HTTPS、WS/WSS driver、pull client |
| [phase-03-module-play-pull.md](phase-03-module-play-pull.md) | 规划中 | fMP4 module 播放输出、远端拉流发布、多轨道接入 |
| [phase-04-compat-interop-testing.md](phase-04-compat-interop-testing.md) | 规划中 | SMS/ffmpeg/VLC 互操作、故障样例、性能与运维验证 |

---

## 渐进式执行顺序

1. **Phase 01** — 先把 fMP4 容器能力收敛到 `cheetah-codec`，避免 HLS/fMP4 双实现分叉
2. **Phase 02** — 建立 fMP4 core/driver 传输壳，跑通 HTTP/WS 长连接与 pull client
3. **Phase 03** — 接入 engine，实现本地播放输出与远端拉流发布
4. **Phase 04** — 用 SMS/ffmpeg/VLC 和故障样例补齐真实互操作性与鲁棒性
