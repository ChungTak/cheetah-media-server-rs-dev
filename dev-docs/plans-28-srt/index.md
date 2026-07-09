# SRT 协议实现计划

- **状态**: 部分完成
- **目标**: 集成 `shiguredo_srt`，实现 Cheetah 的 SRT Listener/Caller 双向桥接能力，使 SRT 摄入可转换为 RTSP/RTMP/HLS/WebRTC 等协议，并支持本地流通过 SRT 输出。
- **方法**: 遵循 `core + driver + module` 三段式架构；SRT 协议状态机复用 Sans-I/O crate `shiguredo_srt`；MPEG-TS demux/mux、时间戳归一化、参数集处理和 codec 识别回到 `cheetah-codec`。
- **完成标准**: FFmpeg/OBS/libsrt 可通过 SRT 推流到 Cheetah，并通过 RTSP/RTMP/HLS/WebRTC 播放；Cheetah 本地流可推送到远端 SRT listener；所有阶段通过对应 `cargo fmt`、`cargo clippy`、`cargo test` 和外部互操作验证。

---

## 范围边界

本计划按 `shiguredo_srt` 当前公开能力实现，不在第一阶段自研该库未支持的 SRT 扩展。

| 能力 | 本计划处理 | 说明 |
|------|------------|------|
| Listener handshake | ✅ 实现 | 服务端监听 UDP，接受 OBS/FFmpeg/libsrt caller |
| Caller handshake | ✅ 实现 | 远端 SRT 拉流、推流和 relay job |
| LiveCC | ✅ 实现 | 复用 `shiguredo_srt::LiveCc` |
| ACK/NAK/ACKACK | ✅ 实现 | 由 `shiguredo_srt::SrtConnection` 驱动 |
| RTT / Flow Window | ✅ 实现 | 统计上报到 metrics |
| TSBPD | ✅ 实现 | 输出到 TS demux 前按库事件交付 |
| TLPKTDROP | ✅ 实现 | 与 module 断流/丢包指标关联 |
| AES-128/256 | ✅ 实现 | 配置 passphrase 和 key length |
| KM Refresh | ✅ 实现 | 处理 `KeyRefreshNeeded` 事件 |
| Stream ID | ✅ 实现 | 映射到 `StreamKey`、mode、鉴权信息 |
| FileCC | ❌ 非目标 | `shiguredo_srt` 当前未覆盖；后续单独评估 |
| Rendezvous | ❌ 非目标 | `shiguredo_srt` 当前未覆盖；后续单独评估 |
| Group Membership | ❌ 非目标 | `shiguredo_srt` 当前未覆盖；后续单独评估 |
| 任意二进制 payload 跨协议转换 | ❌ 非目标 | v1 只支持 MPEG-TS over SRT 转媒体流 |

---

## 总体约束

1. 严格遵守 `cheetah-srt-core`、`cheetah-srt-driver-tokio`、`cheetah-srt-module` 三段式。
2. `cheetah-srt-core` 只做 Sans-I/O 封装、Stream ID 解析、URL/配置模型和事件/命令定义，不持有 socket，不启动 task，不调用系统时间。
3. UDP socket、timer、connection map、send queue、backpressure、Tokio task 只在 `cheetah-srt-driver-tokio`。
4. module 只做引擎接入、发布租约、订阅、鉴权、配置和作业编排，不直接依赖 `tokio::net`、`tokio::time`、`tokio::sync` 公共类型。
5. SRT ingress 默认承载 MPEG-TS；TS demux 使用 `cheetah-codec::MpegTsDemuxer`，输出统一为 `AVFrame + TrackInfo`。
6. SRT egress 从引擎订阅 `AVFrame + TrackInfo`，使用 `cheetah-codec::MpegTsMuxer` 封装为 MPEG-TS 后交给 driver 发送。
7. SRT 转 RTSP/RTMP/HLS/WebRTC 不做协议间直连；统一路径为 `SRT -> AVFrame/TrackInfo -> engine -> target module`。
8. 同一 `StreamKey` 仍遵守单发布者独占语义，不允许 SRT module 绕过发布租约。

---

## 参考来源

| 来源 | 路径 |
|------|------|
| SRT RFC draft | <https://haivision.github.io/srt-rfc/draft-sharabayko-srt.html> |
| `shiguredo_srt` docs | <https://docs.rs/shiguredo_srt/latest/shiguredo_srt/> |
| `shiguredo_srt` crates.io | <https://crates.io/crates/shiguredo_srt> |
| `shiguredo_srt` GitHub | <https://github.com/shiguredo/srt-rs> |
| 本项目架构 | `SystemArchitecture.md` |
| 本项目媒体模型 | `crates/foundation/cheetah-codec/` |
| 本项目 TS 参考 | `crates/protocols/ts/` |
| 本项目 RTMP/RTSP/HLS module 参考 | `crates/protocols/{rtmp,rtsp,hls}/module/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [srt-design.md](srt-design.md) | 已完成 | 总体设计、分层边界、Stream ID、数据流、配置、codec 支持和兼容策略 |
| [phase-01-crates-and-core.md](phase-01-crates-and-core.md) | 已完成 | 新增 crate、workspace、server feature、core API、URL/Stream ID 解析 |
| [phase-02-driver-tokio.md](phase-02-driver-tokio.md) | 部分完成 | Tokio UDP driver、connection loop、timer、加密、统计和背压 |
| [phase-03-ingest-and-conversion.md](phase-03-ingest-and-conversion.md) | 已完成 | SRT 摄入、MPEG-TS demux、发布到引擎、转 RTSP/RTMP/HLS/WebRTC |
| [phase-04-egress-and-relay.md](phase-04-egress-and-relay.md) | 部分完成 | 本地流 SRT 输出、pull/push/relay jobs、重试和断开语义 |
| [phase-05-tests-interop-ops.md](phase-05-tests-interop-ops.md) | 部分完成 | 测试、外部互操作、运维指标、验收命令 |
| [srt-ops-interop.md](srt-ops-interop.md) | 已完成 | 外部互操作、弱网、长稳、metrics、health 和 artifact 清单 |

---

## 任务状态总表

| 阶段 | 任务 | 状态 |
|------|------|------|
| 1.1 | 新增 `crates/protocols/srt/{core,driver-tokio,module}` | 已完成 |
| 1.2 | 增加 workspace members、server feature、模块注册 | 已完成 |
| 1.3 | 固定 `shiguredo_srt` 依赖版本并封装 core API | 已完成 |
| 1.4 | 实现 SRT URL / Stream ID 解析和配置校验 | 已完成 |
| 2.1 | 实现 Listener UDP accept loop 和 connection map | 已完成 |
| 2.2 | 实现 Caller 连接和 driver command/event channel | 已完成 |
| 2.3 | 驱动 `ConnectionOutput::{SendPacket, SetTimer, ClearTimer}` | 已完成 |
| 2.4 | 实现加密、KM Refresh、统计、连接上限和背压 | 部分完成：已覆盖基础加密配置校验、AES-128/AES-256 本机加密 payload 回环、passphrase 不匹配失败、KM Refresh 事件、bytes/packets、RTT、jitter、丢包、重传、队列深度统计、连接上限、send queue capacity 边界、idle/connect timeout；2026-06-15 已执行 loopback netem `loss 1% delay 50ms 10ms reorder 1%`、`loss 5% delay 50ms 10ms reorder 5%`、`rate 1200kbit delay 20ms 5ms` 和 `loss 12% delay 80ms 30ms reorder 10%` 短互操作，SRT -> RTMP/HLS 数据面可读；慢 peer send queue full 指标已验证；`shiguredo_srt` 当前 stats 未暴露独立 TLPKTDROP 计数字段，长稳统计仍待外部验证 |
| 3.1 | SRT ingress 接收 payload 并喂给 MPEG-TS demuxer | 已完成 |
| 3.2 | `TrackInfo` 更新和 `AVFrame` 发布时间戳归一化 | 已完成 |
| 3.3 | 验证 SRT -> RTSP/RTMP/HLS/WebRTC 跨协议播放 | 已完成：2026-06-12 已用 FFmpeg 推 SRT MPEG-TS 到 Cheetah；RTMP 已验证 HEVC、H264/AAC 和 H264/MP3，HLS 已验证 H264/AAC playlist 和 TS segment，RTSP 已通过临时非特权端口 `8554` 验证 H264/AAC 播放；control 502 判定为本机代理环境问题，使用 `--noproxy '*'` 后正常；2026-06-15/16 已验证 SRT 摄入流可创建 WebRTC WHEP answer 并被 Chrome headless 播放 H264 视频 RTP；已修复 WebRTC driver answer 无 local candidate、缺少 `a=end-of-candidates`、默认 AAC 无 Opus 转码时关闭整个会话的问题。当前 WebRTC 浏览器音频为降级丢弃，AAC/MP3 -> Opus 转码仍属后续能力 |
| 3.4 | 适配更多音视频 codec 的 SRT MPEG-TS 主路径 | 已完成：`cheetah-codec` TS mux/demux 矩阵覆盖 H264/H265/H266/AV1/VP8/VP9/MJPEG 与 AAC/G711A/G711U/Opus/MP3/MP2/ADPCM；H266 egress 补 AUD + VPS/SPS/PPS，MJPEG/ADPCM 增加私有 descriptor，MPEG audio 首帧头细分 MP2/MP3 并推导采样率/声道；FFmpeg 生成的 VP8/VP9/AV1 MPEG-TS 会使用 `stream_type=0x06` 且无 descriptor，已在 TS demux 首个 PES 做延迟 codec 探测，AV1 同步提取 sequence header 使 track 进入 `Ready`；2026-06-16 已用真实 FFmpeg SRT publish 验证 H265、VP8、VP9、AV1 在 control streams 中均为 `Ready`；SRT module egress Ready gating 已覆盖扩展 passthrough codec |
| 4.1 | 从引擎订阅本地流并 MPEG-TS mux | 已完成：2026-06-12 已验证 RTMP 本地发布流可被 SRT request/play 订阅并封装为 MPEG-TS 输出 |
| 4.2 | 实现 SRT caller push、listener request/play | 已完成：listener request/play 已用 `ffprobe srt://127.0.0.1:9000?...m=request` 拉到 H264/AAC；caller push job 已用 `srt-live-transmit` 远端 listener 验证可输出 H264/AAC TS；重连模板已完成 |
| 4.3 | 实现 SRT relay jobs 和重试退避 | 已完成基础功能：已展开 relay ingress/egress job，并接入 job 断开后的指数退避重连；2026-06-15 修复 egress 在 tracks Ready 前过早初始化 muxer 的竞态后，source listener -> Cheetah relay -> target listener 本机链路输出 `/tmp/cheetah_srt_relay_out.ts` 42KB，ffprobe 可识别 H264 与 AAC 48k；断线长稳验证未执行 |
| 5.1 | core/property/fuzz 测试 | 部分完成：core/unit/property 已覆盖，fuzz workspace 已建立并完成 3 个 target 的短 `-runs=100` smoke；`cargo fuzz run` 默认仓库根 `fuzz/` 布局与当前独立子 workspace 不匹配，长时间 coverage fuzz run 未执行 |
| 5.2 | driver 集成测试 | 部分完成：握手、双向 payload、AES-128/AES-256 加密 payload、passphrase mismatch、stats、send queue capacity 边界、idle/connect timeout、max_connections、配置错误已覆盖；2026-06-15 当前 shell 已确认 `sudo -n true` 与 `sudo -n tc` 可用，并完成 loopback netem 1% loss、5% loss、12% loss 和 1200kbit rate limit 短互操作验证；慢 peer send queue full 通过 `send_queue_capacity=0` 验证；100 路短连接反复 publish 压测通过；24h 长稳仍待外部环境验证 |
| 5.3 | module E2E 和外部实体互操作测试 | 部分完成：FFmpeg SRT publish -> Cheetah -> RTMP probe 已验证，FFmpeg H264/AAC SRT publish -> Cheetah -> HLS playlist/segment、RTSP probe 和 Chrome WHEP H264 视频 RTP 已验证；FFmpeg H264/MP3 SRT publish -> RTMP probe 已验证；2026-06-16 已补 FFmpeg H265、VP8、VP9、AV1 MPEG-TS over SRT publish -> Cheetah control streams codec/readiness 验证；原生 `srt-live-transmit` publish、OBS GUI profile SRT publish、SRT egress listener、relay source->Cheetah->target listener、1%/5%/12% loss loopback netem、1200kbit rate limit、慢 peer send queue full 和 100 路短连接 publish 短互操作已验证；WebRTC answer candidate/EOC 输出、默认 AAC 无转码降级保视频路径均已修复并补回归测试；24h 长稳与 WebRTC AAC/MP3 -> Opus 转码仍待后续环境/能力验证 |
| 5.4 | metrics、health、日志和运维文档 | 部分完成：运维/互操作清单已建立，SRT module 已暴露 `/srt/metrics` 和 `/srt/metrics.json`，覆盖连接、bytes/packets、RTT、jitter、lost/duplicate packets、retransmit、queue depth、key refresh、disconnect、driver error、send queue full；模块级 health 仍复用 engine 全局健康状态，外部 scrape/告警未验证 |

---

## 渐进式执行顺序

1. **Phase 01** — 先落 crate 和 core 边界，锁定 `shiguredo_srt` 依赖、配置、Stream ID 语义。
2. **Phase 02** — 实现 driver，先完成 SRT 双端连接和数据通道，不接引擎。
3. **Phase 03** — 完成 ingress，将 SRT MPEG-TS 转为 `AVFrame + TrackInfo` 并发布到引擎，验证其他协议播放。
4. **Phase 04** — 完成 egress 和 relay，将本地流封装为 MPEG-TS 并通过 SRT 输出。
5. **Phase 05** — 补齐互操作、fuzz、运维指标和长稳测试。

---

## 总体验收命令

每个阶段至少运行：

```bash
cargo fmt
cargo clippy -p cheetah-srt-core
cargo clippy -p cheetah-srt-driver-tokio
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-core
cargo test -p cheetah-srt-driver-tokio
cargo test -p cheetah-srt-module
```

涉及 `cheetah-codec` 的 MPEG-TS 或时间戳改动时追加：

```bash
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo test -p cheetah-ts-core
```

外部互操作阶段追加：

```bash
ffmpeg -re -stream_loop -1 -i sample.mp4 -c copy -f mpegts "srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/test,m=publish"
ffplay rtsp://127.0.0.1:554/live/test
ffplay rtmp://127.0.0.1/live/test
ffplay http://127.0.0.1:8088/live/test/index.m3u8
```
