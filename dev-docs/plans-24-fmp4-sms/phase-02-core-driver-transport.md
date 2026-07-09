# Phase 02 — fMP4 Core + Driver 传输层

- **状态**: 规划中
- **范围**: 新增 `cheetah-fmp4-core` 与 `cheetah-fmp4-driver-tokio`，实现 HTTP/HTTPS-fMP4、WS/WSS-fMP4 server 和远端 pull client
- **完成标准**: 不接入 engine 的情况下，driver 可接受播放请求、发送 fMP4 bytes、处理 WebSocket upgrade，并可从远端 HTTP/WS fMP4 读取 bytes

---

## 2.1 `cheetah-fmp4-core` Sans-I/O 状态机

新增 crate：

```text
crates/protocols/fmp4/core
```

职责：

- HTTP request head 解析后的状态处理
- `.mp4` / `.live.mp4` 路由解析
- WebSocket upgrade 校验
- CORS / HEAD / OPTIONS / 405 / 400 响应模型
- 播放请求事件输出
- driver command 转换为 HTTP body bytes 或 WS binary bytes

核心类型：

```rust
pub enum Fmp4CoreInput {
    RequestHead(Fmp4RequestHead),
    WebSocketMessage(Fmp4WebSocketMessage),
    Command(Fmp4CoreCommand),
}

pub enum Fmp4CoreCommand {
    SendFmp4Bytes(Bytes),
    Close,
}

pub enum Fmp4CoreOutput {
    SendHttpResponse(Fmp4ResponseHead),
    SendBytes(Bytes),
    SendWebSocketBinary(Bytes),
    SendWebSocketPong(Bytes),
    Event(Fmp4CoreEvent),
    Close { reason: Fmp4CloseReason },
}

pub enum Fmp4CoreEvent {
    PlayRequested {
        stream_key: StreamKeyParts,
        transport: Fmp4Transport,
    },
    PeerClosed,
}
```

路由规则：

- `GET /{app}/{stream}.mp4` 播放
- `GET /{app}/{stream}.live.mp4` 播放别名
- `HEAD` 只返回响应头
- `OPTIONS` 返回 CORS preflight
- WebSocket upgrade 使用同一路径
- 其他方法返回 405

---

## 2.2 HTTP/WS Server Driver

新增 crate：

```text
crates/protocols/fmp4/driver-tokio
```

职责：

- TCP bind/accept
- HTTP/1.1 request head 读取
- HTTP chunked response 编码
- WebSocket frame encode/decode，支持 continuation reassembly
- 写队列和慢客户端处理
- driver command/event channel
- cancellation 与 graceful shutdown

HTTP 播放响应：

```text
HTTP/1.1 200 OK
Content-Type: video/mp4
Connection: keep-alive
Cache-Control: no-cache
Transfer-Encoding: chunked
Access-Control-Allow-Origin: *
```

WebSocket 输出：

- 只发送 binary frame
- 每个 frame payload 必须是完整 init segment 或完整 media segment
- ping/pong 由 driver 处理
- text message 默认关闭连接
- continuation frame 重组后再交给 core 或 pull demux

背压策略：

- 每连接有界 `write_queue_capacity`
- queue full 时关闭该连接，不影响其他连接
- 单次 write 最大 chunk 默认为 128 KiB
- write timeout 默认为 10 秒

---

## 2.3 HTTPS/WSS TLS Server

参考：

- `crates/protocols/ts/driver-tokio/src/tls.rs`
- `crates/protocols/http-flv/driver-tokio/src/tls.rs`
- `crates/protocols/hls/driver-tokio/src/tls.rs`

实现要求：

- TLS 只在 driver 层
- `rustls` + `tokio-rustls`
- cert/key 读取和解析错误在 start 阶段返回
- handshake timeout 可配置
- TLS stream 包装为 runtime-neutral driver 内部类型
- HTTPS-fMP4 和 WSS-fMP4 复用同一 core/session 逻辑

---

## 2.4 Pull Client

支持源：

```text
http://example.com/live/camera.mp4
https://example.com/live/camera.mp4
ws://example.com/live/camera.mp4
wss://example.com/live/camera.mp4
```

职责：

- 连接远端源
- 发送 GET 或 WebSocket upgrade
- 校验 HTTP status / WebSocket accept
- 解析 HTTP chunked body
- 将 body bytes 或 WS binary payload 送给上层
- 处理重连由 module supervisor 完成

输出模型：

```rust
pub enum Fmp4PullEvent {
    ResponseHead { status: u16, headers: Vec<(String, String)> },
    Bytes(Bytes),
    Closed { reason: String },
}
```

限制：

- response header 上限默认 32 KiB
- WS message 上限默认 4 MiB
- body read buffer 默认 64 KiB
- TLS 证书校验默认开启；测试可通过显式配置允许 insecure
- HTTP body、chunked frame、WS continuation 都不能绕过 `max_box_bytes`

---

## 2.5 Driver 集成测试

测试场景：

1. HTTP GET 后收到 `video/mp4` 响应和 chunked fMP4 bytes
2. HEAD 只返回 headers，不返回 body
3. OPTIONS 返回 CORS
4. WebSocket upgrade 后收到 binary init segment 和 media segment
5. WebSocket text message 默认关闭
6. WebSocket continuation binary message 可重组成完整 fMP4 bytes
7. write queue full 关闭慢客户端
8. HTTPS/WSS 使用临时测试证书完成 handshake
9. pull client 读取 Content-Length body
10. pull client 读取 chunked body
11. pull client 读取 WebSocket binary body
12. pull client 拒绝 oversized header / oversized WS message

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-fmp4-core
cargo test -p cheetah-fmp4-core
cargo clippy -p cheetah-fmp4-driver-tokio
cargo test -p cheetah-fmp4-driver-tokio
```
