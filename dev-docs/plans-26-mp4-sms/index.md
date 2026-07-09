# MP4 点播与多格式录制实现计划（对标 SimpleMediaServer）

- **状态**: 已完成
- **目标**: 新增 MP4 文件点播与统一录制能力，支持 `FLV/HLS/MP4/PS` 录制，支持 `RTSP/RTMP/HTTP-FLV/WS-FLV` 点播 MP4 文件并提供 seek，补齐 SMS 风格兼容行为与测试体系
- **方法**: 参考 `vendor-ref/simple-media-server/Src/Mp4`、`vendor-ref/simple-media-server/Src/Record`、`vendor-ref/simple-media-server/Src/Api/VodApi.cpp`、`vendor-ref/simple-media-server/Src/Api/RecordApi.cpp`，结合本项目现有 `cheetah-codec`、`cheetah-fmp4-*`、`cheetah-hls-*`、`cheetah-rtmp-*`、`cheetah-rtsp-*`、`cheetah-http-flv-*`
- **完成标准**: MP4 VOD、统一录制任务、SMS 风格 API、非标准兼容、单元测试、属性测试、fuzz 和跨协议集成验证全部落地

---

## V1 范围

首版固定支持：

1. MP4 文件点播，覆盖 `RTSP`、`RTMP`、`HTTP-FLV`、`WS-FLV`
2. 点播控制支持 `start`、`seek`、`pause`、`scale`、`stop`
3. 统一录制任务，输出 `FLV`、`HLS`、`MP4`、`PS`
4. 多轨道模式，媒体统一收敛为 `AVFrame + TrackInfo`
5. 编码矩阵覆盖 `H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1`
6. SMS 风格 `vod` 与 `record` 控制 API
7. SMS 风格 `file/`、`record/` URI 兼容
8. 单元测试、集成测试、属性测试和 fuzz 测试

首版分期约束：

1. **Phase 01** 先补 `cheetah-codec` 的 MP4/录制容器能力
2. **Phase 02** 再补统一录制模块和文件元数据管理
3. **Phase 03** 建立 MP4 VOD 三段式 crate
4. **Phase 04** 接入 `RTSP/RTMP/HTTP-FLV/WS-FLV`
5. **Phase 05** 补齐非标准兼容、互操作和 fuzz/fixture 体系

首版不做：

1. 转码；目标协议或目标容器不支持的组合只返回明确诊断
2. Progressive HTTP `.mp4` byte-range 下载服务
3. 完整 DVR/云存储回放目录服务
4. FLV/PS 文件点播首版主路径；回放协议首批聚焦 MP4 文件

---

## 与 SimpleMediaServer 对比后的主要缺口

| 能力 | SMS 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| Classic MP4 muxer | `Mp4/Mp4Muxer.*`、`Mp4/Mp4FileWriter.*` | 只有轻量 `mp4.rs` 与完整 fMP4 mux | Phase 01 |
| Classic MP4 demuxer / reader | `Mp4/Mp4Demuxer.*`、`Mp4/Mp4FileReader.*` | 无完整 sample table/index/seek | Phase 01 / 03 |
| 统一录制基类 | `Record/Record.*` | 无统一 record task registry | Phase 02 |
| FLV 录制 | `Record/RecordFlv.*` | 无独立 FLV 文件录制任务 | Phase 02 |
| HLS 录制任务 | `Record/RecordHls.*` | HLS file output 存在，但不是统一录制模型 | Phase 02 |
| MP4 录制任务 | `Record/RecordMp4.*` | 无 | Phase 02 |
| PS 录制任务 | `Record/RecordPs.*` | `cheetah-codec::ps` 已有基础能力，但未文件化 | Phase 01 / 02 |
| VOD API | `Api/VodApi.cpp` | 无 | Phase 03 / 04 |
| Record API | `Api/RecordApi.cpp` | 无 | Phase 02 |
| MP4 文件点播 | `RecordReaderMp4.*` | 无 | Phase 03 / 04 |
| RTSP/RTMP/HTTP-FLV/WS-FLV 回放桥接 | `RecordReader*` + media source | 现有模块只播放 live stream | Phase 04 |

---

## 标准与非标准兼容点

### 标准基线

- MP4 文件读写遵循 ISO BMFF 的基本 box 组织和 sample table 语义
- HLS 录制输出遵循 live/VOD playlist 约束，完成时补 `EXT-X-ENDLIST`
- 所有媒体输入输出都统一为 `AVFrame + TrackInfo`
- VOD 会话与录制任务都必须 bounded，不能出现无界缓存或无界目录扫描

### 落地非标准 / 兼容优先

- 兼容 SMS 风格 API、字段名和 URI 约定
- 兼容 `moov` 在前或在后、`free/skip/uuid/wide`、`largesize`、缺失 `stss`
- 兼容异常 `ctts`、损坏 sample table、重复 init、尾部残缺文件，只要能 bounded 诊断并安全失败
- 兼容 FLV Enhanced codec 映射和国内扩展 codec id
- 兼容 HLS 录制保留 TS legacy 模式，但默认走 fMP4 segment 提高 codec 覆盖
- 兼容 GB28181/国标场景需要的 PS 录制路径

---

## 总体约束

1. 严格遵守 `core + driver + module` 三段式架构
2. 录制模块是跨协议系统模块，不复制协议层 socket/runtime 逻辑
3. `cheetah-mp4-core` 必须是 Sans-I/O，不依赖 Tokio、socket、EngineContext
4. Classic MP4 reader/writer、FLV writer、PS writer 尽量下沉到 `cheetah-codec`
5. 点播协议桥接通过 engine stream 和 runtime-neutral API 完成，不在协议模块内部保存私有文件播放状态机
6. 兼容逻辑必须集中到 codec compat、vod compat 或 record compat 层
7. 所有索引、队列、扫描、重组缓存和目录枚举都必须有上界

---

## 参考来源

| 来源 | 路径 |
|------|------|
| SMS MP4 | `vendor-ref/simple-media-server/Src/Mp4/` |
| SMS Record | `vendor-ref/simple-media-server/Src/Record/` |
| SMS VOD API | `vendor-ref/simple-media-server/Src/Api/VodApi.cpp` |
| SMS Record API | `vendor-ref/simple-media-server/Src/Api/RecordApi.cpp` |
| 本项目 codec MP4/fMP4 | `crates/foundation/cheetah-codec/src/mp4.rs`、`fmp4_mux.rs`、`fmp4_demux.rs` |
| 本项目 codec FLV/PS | `crates/foundation/cheetah-codec/src/flv.rs`、`ps.rs` |
| 本项目 HLS | `crates/protocols/hls/` |
| 本项目 RTSP/RTMP/HTTP-FLV | `crates/protocols/rtsp/`、`rtmp/`、`http-flv/` |
| 本项目控制面 | `crates/system/cheetah-control/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [mp4-vod-record-architecture.md](mp4-vod-record-architecture.md) | 已完成 | 总体架构、crate 边界、数据流、配置、REST 路由 |
| [mp4-vod-record-sms-gap-analysis.md](mp4-vod-record-sms-gap-analysis.md) | 已完成 | SMS 行为、兼容点、本地缺口 |
| [phase-01-codec-container-writers.md](phase-01-codec-container-writers.md) | 已完成 | `cheetah-codec` 的 MP4/FLV/HLS/PS 容器能力 |
| [phase-02-record-module-multiformat.md](phase-02-record-module-multiformat.md) | 已完成 | 统一录制模块、文件元数据、任务 API |
| [phase-03-mp4-vod-core-driver.md](phase-03-mp4-vod-core-driver.md) | 已完成 | `cheetah-mp4-core`、`cheetah-mp4-driver-tokio`、`cheetah-mp4-module` |
| [phase-04-cross-protocol-vod-seek.md](phase-04-cross-protocol-vod-seek.md) | 已完成 | RTSP/RTMP/HTTP-FLV/WS-FLV 点播和 seek 接入 |
| [phase-05-compat-interop-fuzz.md](phase-05-compat-interop-fuzz.md) | 已完成 | 兼容、互操作、fixture、属性测试、fuzz |

---

## 渐进式执行顺序

1. **Phase 01** — 先补齐 `cheetah-codec` 的 classic MP4、FLV、PS、HLS 录制导出能力
2. **Phase 02** — 建立统一 `record` 模块与 SMS 风格文件/任务控制 API
3. **Phase 03** — 建立 MP4 VOD 三段式 crate，打通文件 reader、seek、session lifecycle
4. **Phase 04** — 接入 `RTSP/RTMP/HTTP-FLV/WS-FLV` 协议播放和 seek 控制
5. **Phase 05** — 补齐 SMS 非标准兼容、真实样例、fuzz 和生产化验证
