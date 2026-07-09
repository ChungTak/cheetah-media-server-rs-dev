# Phase 02: HTTP/WS 播放输出 Core 与 Driver

- 状态：计划中
- 范围：新增 HTTP-FLV Sans-I/O core、Tokio server driver，并实现 HTTP GET 与 WebSocket 播放输出。
- 完成标准：本地已有 engine stream 时，HTTP-FLV module 能通过独立监听端口提供 `/{app}/{stream}.flv` HTTP 和 WebSocket 播放。

## 目标目录

新增 crate：

```text
crates/protocols/http-flv/core/
  Cargo.toml
  src/lib.rs
  src/request.rs
  src/session.rs
  src/error.rs

crates/protocols/http-flv/driver-tokio/
  Cargo.toml
  src/lib.rs
  src/server.rs
  src/connection.rs
  src/http1.rs
  src/websocket.rs
```

`core` 提供 HTTP-FLV 协议状态和事件；`driver-tokio` 提供网络监听和连接生命周期，不接触 engine。

## Core 输入输出模型

核心输入：

```rust
pub enum HttpFlvCoreInput<'a> {
    RequestHead(HttpRequestHead),
    BodyBytes(&'a [u8]),
    WebSocketMessage(WebSocketMessage),
    Command(HttpFlvCoreCommand),
}
```

核心输出：

```rust
pub enum HttpFlvCoreOutput {
    SendHttpResponse(HttpResponseHead),
    SendBytes(Bytes),
    SendWebSocketBinary(Bytes),
    Event(HttpFlvEvent),
    Close { reason: CloseReason },
}
```

核心事件：

```rust
pub enum HttpFlvEvent {
    PlayRequested {
        stream_key: StreamKeyParts,
        transport: HttpFlvTransport,
        play_mode: RtmpFlvPlayMode,
    },
    PullTag(FlvTag),
    PeerClosed,
}
```

`StreamKeyParts` 只保存 namespace/path 字符串，不依赖 `cheetah-sdk::StreamKey`，由 module 映射。

## Driver 行为

server driver 配置：

```rust
pub struct HttpFlvDriverConfig {
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub max_request_header_bytes: usize,
    pub max_body_buffer_bytes: usize,
    pub max_websocket_message_bytes: usize,
}
```

server event：

```rust
pub enum HttpFlvDriverEvent {
    ConnectionOpened { connection_id: HttpFlvConnectionId, peer: Option<SocketAddr> },
    ConnectionClosed { connection_id: HttpFlvConnectionId, reason: String },
    Core { connection_id: HttpFlvConnectionId, event: HttpFlvEvent },
}
```

server command：

```rust
pub enum HttpFlvDriverCommand {
    SendFlvBytes { connection_id: HttpFlvConnectionId, bytes: Bytes },
    CloseConnection { connection_id: HttpFlvConnectionId },
    Shutdown,
}
```

HTTP/WS 规则：

- HTTP/1.1 请求头 parser 支持 GET、OPTIONS、Host、Connection、Upgrade、Sec-WebSocket-*。
- GET `.flv` 非 WebSocket 时返回 streaming HTTP response。
- WebSocket upgrade 校验版本 13 和 key，成功后 binary frame 输出 FLV bytes。
- driver 写队列满时按配置关闭慢客户端，不能阻塞其他连接。

## 播放输出集成

module 通过 driver 事件收到 `PlayRequested` 后：

1. 查询当前 stream snapshot。
2. 订阅 stream，bootstrap policy 使用 live tail，容量覆盖 GOP bootstrap。
3. 通过共享 adapter 构造 FLV header、metadata、sequence header 并发送。
4. 消费 subscriber frame，执行 keyframe gate、timestamp rebase/clamp、mute AAC。
5. 调用 driver command 发送 FLV bytes。
6. subscriber 结束或连接关闭时释放资源。

driver 只负责发送 bytes，不理解 `AVFrame`。

## 具体任务

### 2.1 新增 HTTP-FLV Sans-I/O core

- [ ] 创建 `cheetah-http-flv-core` crate，依赖 `bytes`、`thiserror`、`cheetah-codec`、`cheetah-rtmp-core`，不依赖 Tokio。
- [ ] 实现 path/query parser：`/{app}/{stream}.flv`、`type=enhanced`、`type=fastPts`。
- [ ] 实现 HTTP request head 到 `PlayRequested` 事件映射。
- [ ] 实现 WebSocket request head 检查和 accept key 生成所需纯函数。
- [ ] 增加 core 单元测试覆盖合法路由、非法方法、非法路径、query mode、OPTIONS、WebSocket upgrade。

### 2.2 新增 Tokio HTTP/WS server driver

- [ ] 创建 `cheetah-http-flv-driver-tokio` crate，依赖 `cheetah-runtime-api`、`cheetah-http-flv-core`、`tokio`。
- [ ] 实现 `start_server(runtime_api, listen, config, cancel)`，接口风格对齐 RTMP/RTSP driver。
- [ ] 实现 HTTP/1.1 请求头读取、响应头写出、长连接 body 输出。
- [ ] 实现 WebSocket upgrade 与 binary frame write；首版只需要 server-to-client binary，ping/pong/close 做最小合规处理。
- [ ] 增加 driver 集成测试：HTTP GET 能收到 FLV header，WS 能收到 binary FLV header，慢客户端/断开连接能清理任务。

### 2.3 实现 HTTP/WS 播放输出集成

- [ ] 在 module 阶段集成前，先提供 driver test harness，用 synthetic FLV bytes 验证发送路径。
- [ ] 固定 driver command sender API，支持按 connection id 发送 bytes 和关闭连接。
- [ ] 确认 HTTP 和 WebSocket 输出使用相同 FLV byte stream builder。
- [ ] 增加 bounded write queue 测试，写满后关闭慢连接而不是阻塞 driver loop。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-http-flv-core
cargo test -p cheetah-http-flv-core
cargo clippy -p cheetah-http-flv-driver-tokio
cargo test -p cheetah-http-flv-driver-tokio
```
