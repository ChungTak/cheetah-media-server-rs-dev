# 点播与多格式录制实现计划（对标 ZLMediaKit）

- **状态**: 已完成
- **目标**: 新增 MP4 文件点播与统一录制能力，支持 `FLV/HLS/HLS-FMP4/MP4/PS/TS/FMP4` 录制格式扩展，首批验收 `FLV/HLS/MP4/PS`，支持 `RTSP/RTMP/HTTP-FLV/WS-FLV` 播放 MP4 文件并支持 seek
- **方法**: 参考 `vendor-ref/ZLMediaKit/src/Record`、`server/WebApi.cpp`、`src/Rtmp/RtmpSession.cpp`、`src/Rtsp/RtspSession.cpp`、`3rdpart/media-server/libmov`、`libflv`、`libmpeg`，结合本项目现有 `cheetah-codec`、`cheetah-fmp4-*`、`cheetah-hls-*`、`cheetah-rtmp-*`、`cheetah-rtsp-*`、`cheetah-http-flv-*`
- **完成标准**: MP4 VOD、统一录制任务、ZLM 风格 API、非标准兼容、单元测试、属性测试、fuzz 和跨协议集成验证全部落地

---

## V1 范围

首版固定支持：

1. MP4 文件点播，覆盖 `RTSP`、`RTMP`、`HTTP-FLV`、`WS-FLV`
2. 点播控制支持 `start`、`seek`、`pause`、`speed`、`stop`
3. 统一录制任务，首批输出 `FLV`、`HLS`、`MP4`、`PS`
4. 录制格式 registry 保留 `HLS-FMP4`、`TS`、`FMP4` 扩展位
5. 多轨道模式，媒体统一收敛为 `AVFrame + TrackInfo`
6. 编码矩阵覆盖 `H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1`
7. ZLM 风格 record / loadMP4File / seekRecordStamp / setRecordSpeed API 兼容
8. RTMP `mp4:` URI、RTSP `Range/Scale`、RTMP `seek/onPlayCtrl` 兼容
9. 单元测试、集成测试、属性测试和 fuzz 测试

首版分期约束：

1. **Phase 01** 先补 `cheetah-codec` 的 MP4 与录制容器能力
2. **Phase 02** 再补统一录制模块和文件元数据管理
3. **Phase 03** 建立 MP4 VOD 三段式 crate
4. **Phase 04** 接入 `RTSP/RTMP/HTTP-FLV/WS-FLV`
5. **Phase 05** 补齐 ZLM 非标准兼容、互操作和 fuzz/fixture 体系

首版不做：

1. 转码；目标协议或目标容器不支持的组合只返回明确诊断
2. 完整 HTTP 文件服务器替代现有控制面下载能力
3. 云存储、对象存储和 DVR 平台级检索
4. FLV/PS 文件点播主路径；文件点播首批聚焦 MP4

---

## 与 ZLMediaKit 对比后的主要缺口

| 能力 | ZLM 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| MP4 recorder | `src/Record/MP4Recorder.*` | 无统一 MP4 文件录制任务 | Phase 01/02 |
| MP4 muxer | `src/Record/MP4Muxer.*`、`3rdpart/media-server/libmov` | 只有轻量 `mp4.rs` 与 fMP4 mux | Phase 01 |
| MP4 demuxer / reader | `src/Record/MP4Demuxer.*`、`MP4Reader.*` | 无 classic MP4 index/seek/VOD reader | Phase 01/03 |
| 多 MP4 串联回放 | `MultiMP4Demuxer` | 无 | Phase 03 |
| HLS / HLS-FMP4 录制 | `src/Record/HlsRecorder.h` | HLS 有 file output，但不是统一录制模型 | Phase 01/02 |
| FLV 录制 | `src/Rtmp/FlvMuxer.*` | 无统一 record module 接入 | Phase 01/02 |
| PS 录制 | `3rdpart/media-server/libmpeg`、RTP-PS 路径 | `cheetah-codec::ps` 有基础能力，未文件化 | Phase 01/02 |
| Record API | `server/WebApi.cpp` `/index/api/startRecord` 等 | 无 | Phase 02 |
| MP4 load API | `server/WebApi.cpp` `/index/api/loadMP4File` | 无 | Phase 03 |
| RTSP seek/speed | `src/Rtsp/RtspSession.cpp` | RTSP 已有 Range 解析雏形，无 VOD source | Phase 04 |
| RTMP seek/speed | `src/Rtmp/RtmpSession.cpp` | RTMP live 播放已有，无 VOD source | Phase 04 |
| RTMP mp4 URI 兼容 | `RtmpSession::getStreamId` | 无 | Phase 04 |

---

## 标准与非标准兼容点

### 标准基线

- MP4 文件读写遵循 ISO BMFF 的 box 和 sample table 语义
- RTSP seek 基于 `Range: npt=`，speed 基于 `Scale`
- RTMP seek 使用 `seek` command，speed 使用 `onPlayCtrl`
- HLS 录制完成后输出 VOD playlist 和 `EXT-X-ENDLIST`
- 所有媒体输入输出都统一为 `AVFrame + TrackInfo`

### 落地非标准 / 兼容优先

- 兼容 ZLM `/index/api/*` 字段、数字 type 和 `customized_path`
- 兼容 RTMP 播放 `rtmp://host/record/mp4:0`、`mp4:0.mp4` 还原为 `0.mp4`
- 兼容 MP4 `moov` 在前或在后、`free/skip/uuid/wide`、`largesize`、缺失 `stss`
- 兼容异常 `ctts`、损坏 sample table、尾部残缺文件，只要能 bounded 诊断并安全失败
- 兼容 FLV Enhanced codec 和国内扩展 codec id
- 兼容 HLS MPEG-TS 与 HLS-FMP4 双录制路径
- 兼容 GB28181/国标场景需要的 PS 录制路径

---

## 总体约束

1. 严格遵守 `core + driver + module` 三段式架构
2. `cheetah-mp4-core` 必须是 Sans-I/O，不依赖 Tokio、socket、EngineContext
3. 录制模块是跨协议系统模块，不复制协议层 socket/runtime 逻辑
4. Classic MP4 reader/writer、FLV writer、PS writer 尽量下沉到 `cheetah-codec`
5. 点播协议桥接通过 engine stream 和 runtime-neutral API 完成
6. 兼容逻辑必须集中到 codec compat、vod compat 或 record compat 层
7. 所有索引、队列、扫描、重组缓存和目录枚举都必须有上界

---

## 参考来源

| 来源 | 路径 |
|------|------|
| ZLM Record | `vendor-ref/ZLMediaKit/src/Record/` |
| ZLM Web API | `vendor-ref/ZLMediaKit/server/WebApi.cpp` |
| ZLM RTMP seek / URI compat | `vendor-ref/ZLMediaKit/src/Rtmp/RtmpSession.cpp` |
| ZLM RTSP seek / speed | `vendor-ref/ZLMediaKit/src/Rtsp/RtspSession.cpp` |
| ZLM MediaSource event | `vendor-ref/ZLMediaKit/src/Common/MediaSource.h` |
| ZLM MultiMediaSourceMuxer | `vendor-ref/ZLMediaKit/src/Common/MultiMediaSourceMuxer.*` |
| ZLM libmov / libflv / libmpeg | `vendor-ref/ZLMediaKit/3rdpart/media-server/` |
| 本项目 codec MP4/fMP4 | `crates/foundation/cheetah-codec/src/mp4.rs`、`fmp4_mux.rs`、`fmp4_demux.rs` |
| 本项目 codec FLV/PS | `crates/foundation/cheetah-codec/src/flv.rs`、`ps.rs` |
| 本项目 HLS/RTSP/RTMP/HTTP-FLV | `crates/protocols/hls/`、`rtsp/`、`rtmp/`、`http-flv/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [mp4-vod-record-architecture.md](mp4-vod-record-architecture.md) | 已完成 | 总体架构、crate 边界、数据流、配置、REST 路由 |
| [mp4-vod-record-zlm-gap-analysis.md](mp4-vod-record-zlm-gap-analysis.md) | 已完成 | ZLM 行为、兼容点、本地缺口 |
| [phase-01-codec-container-writers.md](phase-01-codec-container-writers.md) | 已完成 | `cheetah-codec` 的 MP4/FLV/HLS/PS/TS/FMP4 容器能力 |
| [phase-02-record-module-multiformat.md](phase-02-record-module-multiformat.md) | 已完成 | 统一录制模块、文件元数据、ZLM 风格任务 API |
| [phase-03-mp4-vod-core-driver.md](phase-03-mp4-vod-core-driver.md) | 已完成 | `cheetah-mp4-core`、`cheetah-mp4-driver-tokio`、`cheetah-mp4-module` |
| [phase-04-cross-protocol-vod-seek.md](phase-04-cross-protocol-vod-seek.md) | 已完成 | RTSP/RTMP/HTTP-FLV/WS-FLV 点播和 seek 接入 |
| [phase-05-compat-interop-fuzz.md](phase-05-compat-interop-fuzz.md) | 已完成 | 兼容、互操作、fixture、属性测试、fuzz |

---

## 渐进式执行顺序

1. **Phase 01** — 先补齐 `cheetah-codec` 的 classic MP4、FLV、PS、HLS/HLS-FMP4、TS、FMP4 录制导出能力
2. **Phase 02** — 建立统一 `record` 模块与 ZLM 风格 record 控制 API
3. **Phase 03** — 建立 MP4 VOD 三段式 crate，打通文件 reader、seek、session lifecycle 和多文件串联
4. **Phase 04** — 接入 `RTSP/RTMP/HTTP-FLV/WS-FLV` 协议播放和 seek/speed/pause 控制
5. **Phase 05** — 补齐 ZLM 非标准兼容、真实样例、fuzz 和生产化验证
