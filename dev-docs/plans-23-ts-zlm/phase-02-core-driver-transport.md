# Phase 02 — TS Core / Driver 传输完善

- **状态**: 未开始
- **范围**: `cheetah-ts-core` 请求状态机、HTTP/HTTPS server、WS/WSS server、WebSocket framing、pull transport、写队列和慢客户端策略
- **完成标准**: HTTP-TS、HTTPS-TS、WS-TS、WSS-TS 在裸 TCP/TLS/WebSocket 客户端下均能正确播放，driver 集成测试覆盖请求、响应、封帧、关闭和背压

---

## 2.1 HTTP 请求路径与方法语义

**ZLMediaKit 参考**:

- `HttpSession::checkLiveStreamTS()` 识别 `.live.ts`
- `checkLiveStream()` 保留 query 给鉴权和媒体信息，但 stream id 不包含特殊后缀

**实现要求**:

- 支持 `/{app}/{stream}.ts`
- 支持 `/{app}/{stream}.live.ts`
- 支持嵌套 stream path：`/{app}/sub/path.ts`
- query 不进入 `StreamKeyParts.stream_path`
- `GET` 才触发 `PlayRequested`
- `HEAD` 只返回 200 header 并关闭，不启动订阅
- `OPTIONS` 返回 CORS 和 `Allow: GET, HEAD, OPTIONS`
- 非法路径返回 400，未知 method 返回 405

**测试要求**:

- `.ts` 和 `.live.ts` 解析
- `HEAD` 不产生 `PlayRequested`
- `GET` WebSocket upgrade 产生 `TsTransport::WebSocket`
- path traversal、percent encoding、空 app/stream 拒绝

---

## 2.2 HTTP/1.1 长连接输出

**ZLMediaKit 参考**:

- `sendResponse(200, false, content_type, ..., no_content_length=true)`
- `HttpSession::onWrite()` 裸写 TS bytes
- `setSocketFlags()` 为直播牺牲部分延迟换吞吐

**实现要求**:

- HTTP-TS 响应不带 `Content-Length`
- `Content-Type: video/mp2t`
- `Cache-Control: no-cache`
- `Access-Control-Allow-Origin: *`
- 如果客户端 `Connection: close`，连接断开时正常清理 session
- 每连接写队列有上限；队列满时关闭该连接并发 `ConnectionClosed`
- 大块 TS bytes 可合并写，但不得无限缓存

**测试要求**:

- curl 风格 HTTP GET 能收到 200 + TS sync byte
- 慢客户端写队列满后只关闭自身
- 断开连接后 module 侧 play session 结束

---

## 2.3 WebSocket-TS 封帧

**ZLMediaKit 参考**:

- `HttpSession::checkWebSocket()` 先返回 101
- `HttpSession::onWrite()` 使用 `WebSocketHeader::BINARY`
- `WebSocketSplitter::encode()` 支持 125/126/127 payload length
- `MAX_WS_PACKET = 4 * 1024 * 1024`

**实现要求**:

- WebSocket 握手成功后立即返回 101
- server 发出的 TS bytes 必须封装为 binary frame
- server 发 frame 不设置 mask
- 支持 payload length `<=125`、`126`、`127`
- 读客户端 frame 时要求客户端 mask；不 mask 的 data frame 关闭
- close frame：回 close frame 并关闭
- ping frame：回 pong
- pong frame：忽略
- text frame：协议错误关闭
- 单 frame payload 超过 `websocket_max_frame_bytes` 时关闭

**测试要求**:

- WebSocket upgrade response 的 `Sec-WebSocket-Accept` 正确
- SendBytes 后客户端解析到 binary frame，payload 以 `0x47` 开始
- ping 得到 pong
- close 得到 close
- 超大 payload 关闭

---

## 2.4 HTTPS / WSS 接线

**ZLMediaKit 参考**:

- `HttpSession` 通过 `overSsl()` 区分 `http/https` 与 `ws/wss`

**实现要求**:

- `TsModuleConfig.tls.enabled=true` 时启动独立 TLS listener
- TLS listener 同时支持 HTTPS-TS 和 WSS-TS
- `tls.handshake_timeout_ms` 生效
- cert/key 加载失败时 module start 返回错误
- 明文 listener 和 TLS listener 共享 driver event/command 模型

**测试要求**:

- 自签证书下 HTTPS GET 成功
- 自签证书下 WSS upgrade 成功
- 握手超时关闭连接
- TLS 配置错误启动失败

---

## 2.5 Pull Transport

**ZLMediaKit 参考**:

- `HttpTSPlayer::onResponseHeader()` 接受 200/206
- Content-Type 接受 `video/mp2t`、`video/mpeg`、`application/octet-stream`
- `TsPlayer::onResponseBody()` 首次 body 到达即 play success
- `TsPlayer::onResponseCompleted()` 空 body 视为失败

**实现要求**:

- URL scheme 支持 `http://`、`https://`、`ws://`、`wss://`
- HTTP pull 支持 status 200/206
- HTTP pull 支持 chunked transfer decoding
- HTTP pull 支持无 `Content-Length` 的直播 body
- HTTPS pull 支持 `insecure_tls` 跳过证书校验
- WS/WSS pull 发送标准 WebSocket GET upgrade
- WS/WSS pull 只把 binary frame payload 交给 demux
- text frame 忽略或诊断；close frame 结束；ping 回 pong
- 首次 body 到达前 EOF 返回错误
- 已收到 body 后 EOF 返回正常 closed diagnostic，由 supervisor 决定重连

**测试要求**:

- HTTP 200 body
- HTTP 206 body
- HTTP chunked body
- HTTPS insecure_tls body
- WS binary body
- WSS binary body
- 空 body EOF 失败

---

## 验证命令

```bash
cargo fmt
cargo clippy -p cheetah-ts-core
cargo test -p cheetah-ts-core
cargo clippy -p cheetah-ts-driver-tokio
cargo test -p cheetah-ts-driver-tokio
```
