# TS 架构完善设计（对标 ZLMediaKit）

- **状态**: 未开始
- **范围**: 明确 TS 协议完善后的分层边界、数据流、配置、兼容层和测试入口
- **完成标准**: 后续 phase 可以按本文件定义的边界执行，不需要重新决定 crate 职责或跨层依赖方向

---

## 架构目标

ZLMediaKit 的 TS 链路核心是：

1. `MpegMuxer` 将 frame 封装为 TS packet
2. `TSMediaSource` 用 ring/cache 管理直播 TS packet
3. `HttpSession::checkLiveStreamTS()` 将 HTTP/WS 播放者挂到 TS ring
4. `HttpTSPlayer` / `TsPlayerImp` 拉取 HTTP-TS，demux 后作为普通媒体源发布

本项目对应实现必须保持：

1. TS 容器逻辑在 `cheetah-codec`
2. 协议状态机在 `cheetah-ts-core`
3. 网络与 TLS/WebSocket 在 `cheetah-ts-driver-tokio`
4. engine 订阅/发布与按需策略在 `cheetah-ts-module`

---

## 目标数据流

### 本地播放 HTTP-TS

```text
publisher -> engine ring(AVFrame + TrackInfo)
          -> cheetah-ts-module subscribe
          -> cheetah-codec MpegTsMuxer
          -> cheetah-ts-driver-tokio HTTP response body
          -> player
```

关键要求：

- 新连接先发 PAT/PMT，再发 bootstrap/live frame
- `pat_pmt_interval_ms` 到期或关键帧到达时补发 PAT/PMT
- 慢客户端只关闭自身连接，不影响其他订阅者

### 本地播放 WS-TS

```text
publisher -> engine ring
          -> cheetah-ts-module
          -> cheetah-codec MpegTsMuxer
          -> cheetah-ts-driver-tokio WebSocket binary frame
          -> browser/player
```

关键要求：

- HTTP upgrade 后立即返回 101，避免按需播放场景 pending
- TS bytes 必须封装为 WebSocket binary frame，不能裸写
- driver 处理 close/ping/pong，payload 上限默认 4 MiB

### 远端拉流 HTTP(S)/WS(S)-TS

```text
remote server -> cheetah-ts-driver-tokio pull client
              -> cheetah-codec MpegTsDemuxer
              -> AVFrame + TrackInfo
              -> cheetah-ts-module publisher sink
              -> engine
```

关键要求：

- HTTP 接受 200/206
- Content-Type 宽松判断，只对明显异常记录告警
- chunked body、半包、粘包、前导垃圾由 pull + demux 组合消化
- demux flush 在连接结束时执行，避免丢尾帧

---

## 文件边界

| 层 | 文件/目录 | 职责 |
|----|-----------|------|
| codec | `crates/foundation/cheetah-codec/src/ts_common.rs` | TS 常量、stream_type、descriptor、CRC、timestamp、packet writer |
| codec | `crates/foundation/cheetah-codec/src/ts_mux.rs` | `AVFrame + TrackInfo` 到 TS packet |
| codec | `crates/foundation/cheetah-codec/src/ts_demux.rs` | TS bytes 到 `TrackInfo` / `AVFrame` |
| core | `crates/protocols/ts/core/src/request.rs` | HTTP target、headers、WebSocket upgrade 解析 |
| core | `crates/protocols/ts/core/src/session.rs` | Sans-I/O TS 播放会话状态机 |
| driver | `crates/protocols/ts/driver-tokio/src/server.rs` | HTTP/WS server、连接读写、backpressure |
| driver | `crates/protocols/ts/driver-tokio/src/tls.rs` | HTTPS/WSS listener 和 TLS handshake |
| driver | `crates/protocols/ts/driver-tokio/src/pull.rs` | HTTP(S)/WS(S)-TS pull transport |
| module | `crates/protocols/ts/module/src/config.rs` | 配置模型、默认值、校验 |
| module | `crates/protocols/ts/module/src/module.rs` | engine 订阅/发布、play session、pull job |

---

## 配置模型

保留现有配置字段并补齐语义：

| 字段 | 目标语义 |
|------|----------|
| `enabled` | 是否启动 TS module |
| `listen` | HTTP/WS-TS 监听地址 |
| `tls.enabled` / `tls.listen` | HTTPS/WSS-TS 监听地址 |
| `tls.cert_path` / `tls.key_path` | TLS 证书和私钥 |
| `tls.handshake_timeout_ms` | TLS 握手超时 |
| `write_queue_capacity` | 单连接写队列上限 |
| `read_buffer_size` | HTTP request / pull read buffer |
| `subscriber_queue_capacity` | engine subscriber 队列上限 |
| `bootstrap_max_frames` | 新订阅者最多 bootstrap frame 数 |
| `play_wait_source_timeout_ms` | 等待源出现的超时 |
| `max_tracks` | mux/demux 允许的最大轨道数 |
| `strict_crc` | PAT/PMT CRC 错误是否拒绝 |
| `max_reassembly_bytes` | 每 PID PES 重组上限 |
| `pat_pmt_interval_ms` | HTTP/WS 播放中 PAT/PMT 周期补发间隔 |
| `pull_jobs` | 远端 TS 拉流任务 |

建议新增字段：

| 字段 | 默认值 | 原因 |
|------|--------|------|
| `accepted_path_suffixes` | `[".ts", ".live.ts"]` | 兼容 ZLMediaKit 播放 URL |
| `websocket_max_frame_bytes` | `4194304` | 对齐 ZLM `MAX_WS_PACKET` |
| `pull_content_type_warn_only` | `true` | 对齐 ZLM 宽松 Content-Type |

---

## 兼容层命名

新增兼容逻辑必须显式命名，不允许散落临时分支：

- `TsPathCompat`：`.ts` / `.live.ts` 路径识别
- `TsStreamTypeCompat`：非标准 stream_type 与 descriptor 映射
- `TsPullHttpCompat`：200/206、Content-Type、chunked、空 body 语义
- `TsWebSocketCompat`：binary frame、close/ping/pong、payload 上限
- `TsDemuxCompat`：CRC 宽松、sync 重同步、continuity 诊断

---

## 依赖约束

1. `cheetah-codec` 不依赖 TS protocol crate
2. `cheetah-ts-core` 不依赖 Tokio、engine、driver
3. `cheetah-ts-driver-tokio` 可依赖 Tokio、tokio-rustls，但不持有业务状态
4. `cheetah-ts-module` 不直接暴露 `tokio::*` 公共类型，任务派生通过 `RuntimeApi`
5. HLS 若复用 TS 容器，只依赖 `cheetah-codec` 的共享 API

---

## 验收矩阵

| 方向 | HTTP | HTTPS | WS | WSS |
|------|------|-------|----|-----|
| 本地播放 | 必测 | 必测 | 必测 | 必测 |
| 远端拉流 | 必测 | 必测 | 必测 | 必测 |
| ZLM 互操作 | 必测 | 可选 | 必测 | 可选 |
| ffmpeg/ffplay | 必测 | 可选 | 可选 | 可选 |

| 编码 | mux | demux | 多轨 | 互操作 |
|------|-----|-------|------|----------|
| H264 | 必测 | 必测 | 必测 | ffmpeg/VLC/ZLM |
| H265 | 必测 | 必测 | 必测 | ffmpeg/VLC/ZLM |
| AAC | 必测 | 必测 | 必测 | ffmpeg/VLC/ZLM |
| G711A/U | 必测 | 必测 | 必测 | ZLM/GB 场景 |
| OPUS | 必测 | 必测 | 必测 | ZLM/libmpeg |
| MP3 | 必测 | 必测 | 必测 | ffmpeg |
| MP2 | 必测 | 必测 | 必测 | ffmpeg |
| VP8/VP9 | 必测 | 必测 | 可选 | ffmpeg |
| AV1 | 必测 | 必测 | 可选 | ffmpeg |
