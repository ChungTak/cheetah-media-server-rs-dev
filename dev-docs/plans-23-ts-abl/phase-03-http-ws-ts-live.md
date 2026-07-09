# Phase 03 — HTTP(S)-TS 与 WS(S)-TS 直播完善

- **状态**: 计划中
- **范围**: 对齐 ABL HTTP-TS/WS-TS 输出和拉流行为，强化 header、批量发送、WebSocket 控制帧、pull 兼容和慢客户端隔离。
- **完成标准**: HTTP(S)-TS 与 WS(S)-TS 输出/拉流均能通过本地集成测试和 ffprobe/ffplay/VLC/ABL/ZLM 互操作验证。

---

## 3.1 HTTP-TS 输出

**ABL 参考**: `NetServerHTTP_TS.cpp` 输出 `Content-Type: video/mp2t; charset=utf-8`、CORS、keep-alive，并把 TS packet 批量缓存后发送。

**本地要求**:

1. 响应保持标准 `Content-Type: video/mp2t`，测试兼容 `video/mp2t; charset=utf-8`。
2. 不发送 `Content-Length`，保持直播 body。
3. 支持 `GET /{app}/{stream}.ts` 和 `GET /{app}/{stream}.live.ts`。
4. `HEAD` 只回 header，不启动 play session。
5. `OPTIONS` 返回 CORS 和 `Allow`。
6. 可配置 `send_batch_bytes`，批量输出不得突破单连接内存预算。
7. 写失败或队列满时只关闭当前连接。

---

## 3.2 WS-TS 输出

**ABL 参考**: `NetServerWS_TS.cpp` 独立维护 WebSocket 握手状态、key、protocol 和 TS 缓冲。

**本地要求**:

1. WebSocket 101 response 必须包含正确 `Sec-WebSocket-Accept`。
2. 支持可选 `Sec-WebSocket-Protocol` 回显策略，默认不回显未知 protocol。
3. TS payload 必须用 binary frame 发送，server frame 不 mask。
4. 支持 125/126/127 三种长度编码。
5. driver 必须持续读取客户端控制帧：ping 回 pong，close 回 close 并关闭，text/data unmasked 关闭。
6. `websocket_max_frame_bytes` 生效。

---

## 3.3 HTTP(S)-TS pull

**本地要求**:

1. 接受 HTTP status 200/206。
2. 支持 chunked transfer decoding，包括 chunk extension 和 trailer。
3. 无 `Content-Length` 的直播 body 持续读取。
4. Content-Type 接受 `video/mp2t`、`video/mpeg`、`application/octet-stream`，其它类型只 warning。
5. 首次 body 前 EOF 返回错误；收到 body 后 EOF 返回 closed event。
6. HTTPS 支持 `insecure_tls`，默认校验证书。

---

## 3.4 WS(S)-TS pull

**本地要求**:

1. 生成随机 `Sec-WebSocket-Key`，不要使用固定 key。
2. 校验远端 `Sec-WebSocket-Accept`。
3. 只将 binary frame payload 交给 TS demux。
4. ping 回 pong，pong 忽略，close 结束。
5. text frame 输出 diagnostic，不作为 TS bytes。
6. 超大 frame 返回错误并触发 pull retry。

---

## 3.5 module play session

**本地要求**:

1. 新连接先发送 PAT/PMT。
2. bootstrap 优先从关键帧开始。
3. `pat_pmt_interval_ms` 到期或关键帧到达时补发 PAT/PMT。
4. 音频缺失场景不强制造静音；如后续支持，必须在 codec/foundation 层提供可配置 gap filler。
5. 连接关闭后取消对应 subscriber。
6. 慢客户端不影响其它订阅者。

---

## 测试要求

1. curl/ffprobe HTTP-TS 可收到 `0x47` TS sync。
2. HTTPS 自签证书播放。
3. WebSocket client 收到 binary frame。
4. WSS 自签证书播放。
5. HTTP chunked pull。
6. WS pull ping/pong/close。
7. 慢客户端写队列满后只关闭自身。

---

## 验证命令

```bash
cargo fmt
cargo clippy -p cheetah-ts-core
cargo clippy -p cheetah-ts-driver-tokio
cargo clippy -p cheetah-ts-module
cargo test -p cheetah-ts-core
cargo test -p cheetah-ts-driver-tokio
cargo test -p cheetah-ts-module
```
