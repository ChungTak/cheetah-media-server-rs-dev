# GB28181 与 RTP 协议实现计划（对标 SimpleMediaServer）

- **状态**: 已完成
- **目标**: 新增独立 GB28181 与 RTP 协议能力，覆盖 UDP/TCP RTP(PS/TS/ES/Ehome) 推流、RTSP/RTMP/HLS 与 RTP 双向桥接、GB28181 主动拉流、双向语音对讲和多轨道模式
- **方法**: 参考 `vendor-ref/simple-media-server/Src/GB28181`、`vendor-ref/simple-media-server/Src/Rtp`、`vendor-ref/simple-media-server/Src/Api/GB28181Api.cpp`、`vendor-ref/simple-media-server/Src/Api/RtpApi.cpp`，结合本项目现有 `cheetah-codec`、RTSP module、TS module 和控制面 API
- **完成标准**: 标准 GB28181 与 RTP 能力可用，SMS 风格非标准兼容落地，RTP/GB28181 与 RTSP/RTMP/HLS 互转通过单元、集成、端到端和互操作测试

---

## V1 范围

首版固定支持：

1. UDP/TCP RTP 推流服务端，支持 `ps`、`ts`、`es`、`ehome`
2. UDP/TCP RTP 转推客户端，支持主动模式与被动模式
3. 本地流转 RTP 推流，支持 RTSP/RTMP/HLS/内部流桥接
4. RTP 推流进入本地引擎后转 RTSP/RTMP/HLS 等协议输出
5. GB28181 主动拉流、被动收流、基础 SIP 控制、媒体会话编排
6. 双向语音对讲
7. 多视频/多音频轨道模式
8. H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1 编码矩阵

首版分期约束：

1. **Phase 01-03** 先完成 RTP 媒体面与跨协议桥接
2. **Phase 04** 再补 GB28181 SIP 控制面和主动拉流
3. **Phase 05** 补双向语音、Ehome 兼容、真实设备互操作

首版不做：

1. 国标级别完整平台管理面和设备目录同步平台间联级
2. 云台控制、录像查询、订阅告警等非媒体主路径
3. 转码；不被目标协议原生支持的编码仅保证稳定传输和诊断
4. 浏览器 WebRTC 播放/推流替代国标媒体面

---

## 与 SimpleMediaServer 对比后的主要缺口

| 能力 | SMS 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| 独立 RTP server/client | `Rtp/RtpServer.*`、`Rtp/RtpContext.*`、`Api/RtpApi.cpp` | 无独立 RTP 协议 crate | Phase 02/03 |
| RTP over TCP 2-byte framing | `Rtp/RtpParser.cpp`、`Rtp/RtpEncodeTrack.cpp` | 无统一实现 | Phase 01/02 |
| RTP PS/TS/ES ingest | `RtpDecodeTrack.cpp` | TS 仅支持 RTP-TS，PS 被拒绝 | Phase 01/02 |
| RTP send/recv/sendrecv 模式 | `RtpContext.*` | 无 | Phase 02/03 |
| RTCP 与 SSRC 路由 | `RtpManager.cpp` | RTSP 内部有 RTCP，未抽象成独立 RTP 协议 | Phase 02 |
| GB28181 收流/推流 API | `Api/GB28181Api.cpp` | 无 | Phase 03/04 |
| GB28181 主动拉流 | `GB28181ClientPull.*` | 无 | Phase 04 |
| GB28181 被动收流 | `GB28181Server.*` | 无 | Phase 03/04 |
| 双向语音对讲 | GB28181 发送/接收路径 | 无 | Phase 05 |
| Ehome RTP 兼容 | `Ehome2/`、`Ehome5/` | 无 | Phase 05 |
| 多轨道 PS/TS/ES | SMS mux/demux + media source | TS demux 已较完整，PS/ES 缺口大 | Phase 01 |
| RESTful 控制 | `Api/RtpApi.cpp`、`Api/GB28181Api.cpp` | 控制面框架已有，模块路由未实现 | Phase 03/04 |

---

## 标准与非标准兼容点

### 标准基线

- RTP 基于 RFC 3550，支持 UDP 与 TCP 传输
- PS/TS/ES 媒体输入最终统一收敛为 `AVFrame + TrackInfo`
- GB28181 控制面遵循 SIP/SDP/INVITE/ACK/BYE/REGISTER/Keepalive 等基本流程
- 同一 `StreamKey` 保持单发布者独占

### 落地非标准 / 兼容优先

- 兼容 SMS 风格 REST API 与字段命名
- 兼容 RTP over TCP 前置 2 字节长度头
- 兼容 SMS 默认 payload mode 为 `ps`
- 兼容厂商乱序、粘包、半包、前导垃圾、重复 init、异常 source address 切换等脏输入
- 兼容 `socketType` 数字值、`transportMode` 数字或字符串值、`payloadType` 大小写混用
- 兼容未严格对齐 RFC 的 G711、MP3、VP8/VP9/AV1 RTP/PS/TS 承载，只要能稳定识别并诊断
- GB28181 入口允许历史厂商 quirks，内部统一归一化，出口保持稳定

---

## 总体约束

1. 严格遵守 `core + driver + module` 三段式架构
2. RTP/PS/TS/ES/Ehome 媒体承载能力尽量放入 `cheetah-codec` 或协议 `core`，不要散落在 module
3. `cheetah-rtp-core` 和 `cheetah-gb28181-core` 都必须是 Sans-I/O
4. `driver-tokio` 层负责 socket、timer、spawn、RTCP、RTP over TCP framing
5. `module` 层负责引擎接入、REST API、pull/push job、设备会话编排、鉴权与配置
6. `cheetah-sdk`、`cheetah-engine`、`*-module` 公共接口不得暴露 `tokio::*`
7. 兼容逻辑必须集中到 codec compat、RTP compat 或 GB28181 compat 层
8. 所有缓存、乱序窗口、组包缓存、TCP reassembly buffer、队列都必须 bounded

---

## 参考来源

| 来源 | 路径 |
|------|------|
| SMS RTP server/client | `vendor-ref/simple-media-server/Src/Rtp/` |
| SMS GB28181 media/control | `vendor-ref/simple-media-server/Src/GB28181/` |
| SMS RTP API | `vendor-ref/simple-media-server/Src/Api/RtpApi.cpp` |
| SMS GB28181 API | `vendor-ref/simple-media-server/Src/Api/GB28181Api.cpp` |
| SMS Ehome | `vendor-ref/simple-media-server/Src/Ehome2/`、`vendor-ref/simple-media-server/Src/Ehome5/` |
| 本项目 RTP/PS/TS 基础 | `crates/foundation/cheetah-codec/src/rtp.rs`、`ps.rs`、`ts_demux.rs`、`ts_mux.rs` |
| 本项目 RTP-TS Sans-I/O | `crates/protocols/ts/core/src/rtp_ts.rs` |
| 本项目 RTSP RTP 参考 | `crates/protocols/rtsp/module/src/media/` |
| 本项目控制面 HTTP API | `crates/system/cheetah-control/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [gb28181-rtp-architecture.md](gb28181-rtp-architecture.md) | 规划中 | 总体架构、crate 边界、数据流、配置、REST 路由 |
| [gb28181-rtp-sms-gap-analysis.md](gb28181-rtp-sms-gap-analysis.md) | 规划中 | SMS 行为、兼容点、本地缺口 |
| [phase-01-codec-rtp-ps-ts-es.md](phase-01-codec-rtp-ps-ts-es.md) | 已完成 | `cheetah-codec` 媒体内核和 RTP payload/container 补齐 |
| [phase-02-rtp-core-driver-transport.md](phase-02-rtp-core-driver-transport.md) | 已完成 | `cheetah-rtp-core`、`cheetah-rtp-driver-tokio`、UDP/TCP/RTCP |
| [phase-03-rtp-module-rest-bridge.md](phase-03-rtp-module-rest-bridge.md) | 已完成 | cheetah-rtp-module、REST API、跨协议桥接 |
| [phase-04-gb28181-sip-control.md](phase-04-gb28181-sip-control.md) | 已完成 | GB28181 SIP control、主动拉流、设备会话 |
| [phase-05-voice-talk-ehome-interop-testing.md](phase-05-voice-talk-ehome-interop-testing.md) | 已完成 | 语音对讲、Ehome 兼容、真实设备/SMS 互通验证 |

---

## 渐进式执行顺序

1. **Phase 01** — 先补齐 `cheetah-codec` 的 RTP/PS/TS/ES 多编码、多轨和兼容能力
2. **Phase 02** — 建立 `cheetah-rtp-core + cheetah-rtp-driver-tokio`，跑通 UDP/TCP RTP/RTCP 媒体面
3. **Phase 03** — 接入 engine 和控制面，形成 RTP server/client 与跨协议桥接
4. **Phase 04** — 实现 GB28181 SIP 控制面、主动拉流和国标媒体会话编排
5. **Phase 05** — 补语音对讲、Ehome 兼容、设备互操作、故障样例和生产化验证
