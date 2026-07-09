# TS ABL 兼容增强架构

- **状态**: 计划中
- **范围**: 描述 ABL 兼容增强后的本地分层、数据流、兼容模块和配置边界。

---

## 总体原则

1. `cheetah-codec` 继续作为唯一 TS 容器实现位置。
2. `cheetah-ts-core` 只保留 Sans-I/O HTTP/WS 请求状态机，不接触 socket、runtime、engine。
3. `cheetah-ts-driver-tokio` 负责 HTTP/HTTPS/WS/WSS transport 和 pull transport。
4. RTP-TS 输入属于 driver/module 接入能力，RTP parsing 可在 `cheetah-codec` 或明确的 RTP helper 中复用，但不得写进 TS core。
5. `cheetah-ts-module` 只负责编排 engine 订阅/发布、pull/ingest job、配置和鉴权。

---

## 目标数据流

### HTTP(S)-TS / WS(S)-TS 直播输出

```text
publisher
  -> engine stream(AVFrame + TrackInfo)
  -> cheetah-ts-module subscribe
  -> cheetah-codec::MpegTsMuxer
  -> driver bounded write queue
  -> HTTP body or WebSocket binary frame
  -> player
```

增强点：

1. ABL 风格批量输出，但批量缓存有上限。
2. 写错误或队列满只关闭当前连接。
3. 每个连接都有独立 muxer 状态和 continuity counter。
4. 新连接先发 PAT/PMT，并等待关键帧或 bootstrap 帧。

### HTTP(S)-TS / WS(S)-TS 拉流

```text
remote HTTP/WS TS
  -> cheetah-ts-driver-tokio pull client
  -> cheetah-codec::MpegTsDemuxer
  -> TrackInfo + AVFrame
  -> cheetah-ts-module PublisherSink
  -> engine
```

增强点：

1. HTTP 200/206、chunked、无 Content-Length 直播 body。
2. Content-Type 宽松诊断，不立即拒绝。
3. WebSocket 只接收 binary frame 作为 TS payload。
4. EOF 前未收到任何 body 视为失败；收到过 body 后按重连策略处理。

### RTP-TS 输入

```text
UDP/TCP RTP packets
  -> RTP parser and SSRC router
  -> TS payload slicing and sync validation
  -> cheetah-codec::MpegTsDemuxer
  -> FrameRateEstimator + TrackInfo updates
  -> PublisherSink
  -> engine
```

增强点：

1. 支持 RTP header extension、CSRC 和 padding。
2. SSRC 分流，同一 SSRC 绑定一个 ingest session。
3. RTP payload 同时兼容 TS 和 PS 探测；本计划只实现 TS 分支，PS 分支只保留明确错误或转交未来 PS module。
4. 不要求每个 RTP payload 必须完整 188 对齐；对齐场景快路径，非对齐走 demux 重同步。
5. 统计 RTP timestamp/PTS 得到真实视频帧率，更新 TrackInfo 或 stream metadata。

---

## 兼容层命名

| 名称 | 归属 | 职责 |
|------|------|------|
| `TsStreamTypeCompat` | codec | ABL/libmpeg 私有 stream_type、descriptor 和未知 private stream 诊断 |
| `TsTimestampCompat` | codec | PTS/DTS wrap、G711 duration、AAC ADTS duration、真实帧率辅助 |
| `TsDemuxFaultCompat` | codec | sync loss、CRC loose/strict、continuity gap、PES overflow |
| `RtpTsCompat` | driver/module | RTP header 解析、SSRC 分流、PS/TS 探测、海康切包兼容 |
| `HttpTsCompat` | core/driver | `.ts`/`.live.ts`、header、chunked、Content-Type 兼容 |
| `WsTsCompat` | core/driver | binary frame、ping/pong/close、payload 上限、mask 校验 |
| `TsLiveOutputCompat` | module | 批量发送、写错误计数、PAT/PMT cadence、关键帧启动 |

---

## 配置建议

在现有 `TsModuleConfig` 基础上补语义或新增字段：

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `websocket_max_frame_bytes` | `4194304` | 对齐 ABL/ZLM 级别的 WS payload 上限 |
| `http_ts_content_type` | `video/mp2t` | 输出默认标准类型；兼容测试允许 `video/mp2t; charset=utf-8` |
| `send_batch_bytes` | `32768` | ABL 风格批量发送上限，必须小于写队列总内存预算 |
| `max_write_errors` | `1` | 写失败累计到阈值关闭连接 |
| `rtp_ts.enabled` | `false` | 是否启动 RTP-TS ingest |
| `rtp_ts.listen` | `0.0.0.0:0` | RTP-TS UDP/TCP 监听地址 |
| `rtp_ts.max_sessions` | `1024` | SSRC session 上限 |
| `rtp_ts.session_idle_timeout_ms` | `30000` | RTP session 空闲清理 |
| `rtp_ts.allow_unaligned_payload` | `true` | 兼容非 188 对齐 RTP payload |
| `frame_rate_estimator.window_frames` | `250` | ABL 近期版本对 RTP/RTMP 的平均窗口 |
| `frame_rate_estimator.warmup_frames` | `15` | 忽略启动初期不稳定间隔 |

---

## 测试边界

1. codec 层测试不启动网络。
2. driver 层测试可以启动本地 TCP/UDP/TLS/WebSocket。
3. module 层测试覆盖 engine publish/subscribe、pull job、RTP ingest job 和 lease release。
4. 互操作脚本使用 ffmpeg/ffprobe/VLC/ABL/ZLM，不能替代 Rust 自动测试。

---

## 风险

1. RTP-TS 与 RTSP/GB28181 RTP 代码可能复用边界不清。实现前应优先查找已有 RTP parser，避免重复协议栈。
2. VP8/VP9/AV1 in TS 的播放器兼容性不稳定。目标是可 mux/demux、可诊断，不承诺所有播放器直接播放。
3. 多节目 MPTS 容易扩大 scope。本轮 module 默认只发布单 program，多 program demux 先输出 diagnostic 和可配置选择。
