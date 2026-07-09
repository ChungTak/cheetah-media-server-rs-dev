# Phase 02 — TS Core + Driver 传输层

- **状态**: 规划中
- **范围**: 新增 `cheetah-ts-core` 与 `cheetah-ts-driver-tokio`，实现 HTTP/HTTPS-TS、WS/WSS-TS server 和远端 pull client
- **完成标准**: 不接入 engine 的情况下，driver 可接受播放请求、发送 TS bytes、处理 WebSocket upgrade，并可从远端 HTTP/WS TS 读取 bytes

---

## 2.1 `cheetah-ts-core` Sans-I/O 状态机

新增 crate：

```text
crates/protocols/ts/core
```

职责：

- HTTP request head 解析后的状态处理
- `.ts` 路由解析
- WebSocket upgrade 校验
- CORS / HEAD / OPTIONS / 405 / 400 响应模型
- 播放请求事件输出
- driver command 转换为 HTTP body bytes 或 WS binary bytes

核心类型：

```rust
pub enum TsCoreInput {
    RequestHead(TsRequestHead),
    WebSocketMessage(TsWebSocketMessage),
    Command(TsCoreCommand),
}

pub enum TsCoreCommand {
    SendTsBytes(Bytes),
    Close,
}

pub enum TsCoreOutput {
    SendHttpResponse(TsResponseHead),
    SendBytes(Bytes),
    SendWebSocketBinary(Bytes),
    Event(TsCoreEvent),
    Close { reason: TsCloseReason },
}

pub enum TsCoreEvent {
    PlayRequested {
        stream_key: StreamKeyParts,
        transport: TsTransport,
    },
    PeerClosed,
}
```

路由规则：

- `GET /{app}/{stream}.ts` 播放
- `HEAD /{app}/{stream}.ts` 只返回响应头
- `OPTIONS` 返回 CORS preflight
- WebSocket upgrade 使用同一路径
- 其他方法返回 405

---

## 2.2 HTTP/WS Server Driver

新增 crate：

```text
crates/protocols/ts/driver-tokio
```

职责：

- TCP bind/accept
- HTTP/1.1 request head 读取
- WebSocket frame encode/decode
- 写队列和慢客户端处理
- driver command/event channel
- cancellation 与 graceful shutdown

HTTP 播放响应：

```text
HTTP/1.1 200 OK
Content-Type: video/mp2t
Connection: keep-alive
Cache-Control: no-cache
Access-Control-Allow-Origin: *
```

WebSocket 输出：

- 只发送 binary frame
- 每个 frame payload 必须是完整 TS packet 串
- ping/pong 由 driver 处理
- text message 默认关闭连接

背压策略：

- 每连接有界 `write_queue_capacity`
- queue full 时关闭该连接，不影响其他连接
- 单次 write 最大 chunk 默认为 128 KiB
- write timeout 默认为 10 秒

---

## 2.3 HTTPS/WSS TLS Server

参考：

- `crates/protocols/http-flv/driver-tokio/src/tls.rs`
- `crates/protocols/hls/driver-tokio/src/tls.rs`

实现要求：

- TLS 只在 driver 层
- `rustls` + `tokio-rustls`
- cert/key 读取和解析错误在 start 阶段返回
- handshake timeout 可配置
- TLS stream 包装为 `AsyncTcpStream`
- HTTPS-TS 和 WSS-TS 复用同一 core/session 逻辑

---

## 2.4 Pull Client

支持源：

```text
http://example.com/live/camera.ts
https://example.com/live/camera.ts
ws://example.com/live/camera.ts
wss://example.com/live/camera.ts
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
pub enum TsPullEvent {
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

---

## 2.5 Driver 集成测试

测试场景：

1. HTTP GET 后收到 `video/mp2t` 响应和 TS bytes
2. HEAD 只返回 headers，不返回 body
3. OPTIONS 返回 CORS
4. WebSocket upgrade 后收到 binary TS frame
5. WebSocket text message 默认关闭
6. write queue full 关闭慢客户端
7. HTTPS/WSS 使用临时测试证书完成 handshake
8. pull client 读取 Content-Length body
9. pull client 读取 chunked body
10. pull client 读取 WebSocket binary body

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-ts-core
cargo test -p cheetah-ts-core
cargo clippy -p cheetah-ts-driver-tokio
cargo test -p cheetah-ts-driver-tokio
```
