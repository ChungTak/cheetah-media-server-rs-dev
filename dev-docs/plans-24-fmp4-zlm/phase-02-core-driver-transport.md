# Phase 02 — Core 与 Driver 传输补强

- **状态**: 规划中
- **范围**: 完善 `cheetah-fmp4-core` 与 `cheetah-fmp4-driver-tokio` 的 HTTP/HTTPS-fMP4、WS/WSS-fMP4、pull client、WebSocket framing、TLS 和背压。
- **完成标准**: 不依赖真实 engine 时，driver 能完整处理 HTTP/WS/TLS 播放和远端 pull 字节流。

## 2.1 Core 路由与响应

保持 Sans-I/O，只处理：

- `GET /{app}/{stream}.mp4`
- `GET /{app}/{stream}.live.mp4`
- `HEAD`
- `OPTIONS`
- WebSocket upgrade
- 400 / 405 / CORS / close reason

补充测试：

- `.live.mp4` WebSocket upgrade。
- query string 保留但不影响 stream key。
- `%`、`..`、空 app、空 stream、超长路径拒绝。
- `HEAD` 不触发播放 session。
- WebSocket 缺少 version/key 返回 400。

## 2.2 HTTP/1.1 Server

driver 负责：

- request line/header 读取上限。
- HTTP chunked response 编码。
- 每连接有界 write queue。
- queue full 关闭单连接。
- connection close 事件必须通知 module。
- 可配置 write timeout。

HTTP 响应头：

```text
Content-Type: video/mp4
Connection: keep-alive
Cache-Control: no-cache
Transfer-Encoding: chunked
Access-Control-Allow-Origin: *
```

集成测试：

- GET 后收到 chunked init segment。
- 多个 chunk 可被客户端正确解码。
- 慢客户端填满 write queue 后只关闭该连接。

## 2.3 HTTPS/WSS Server

当前 fMP4 `tls.rs` 仍是占位，需要实现：

- `rustls + tokio-rustls` TLS acceptor。
- cert/key PEM 读取。
- handshake timeout。
- 独立 TLS listen 地址。
- HTTPS 与 WSS 复用同一 core/session 逻辑。
- TLS 配置错误在 module start 阶段返回。

测试：

- 临时自签证书启动 HTTPS。
- HTTPS GET 收到 fMP4 response。
- WSS upgrade 后收到 WebSocket binary。
- handshake timeout 和证书路径错误可诊断。

## 2.4 WebSocket Framing

对齐 ZLM `WebSocketSplitter` 的实际能力：

- 支持 masked client frame。
- 支持 continuation reassembly。
- binary continuation 聚合后再交给上层。
- ping 自动 pong。
- close 自动回 close 并关闭连接。
- text 默认关闭连接。
- 单 message 默认上限 4 MiB。
- RSV 非 0 默认拒绝，除非未来显式支持扩展。

server 输出：

- server-to-client binary frame 不 mask。
- 每个 binary payload 是完整 init segment 或完整 media segment。

pull client 输出：

- client-to-server pong/close 必须 mask。
- mask key 使用随机值，不能固定。
- 校验 `Sec-WebSocket-Accept`。

## 2.5 Pull Client

HTTP pull 必须支持：

- `Content-Length` body。
- `Transfer-Encoding: chunked` body。
- 无长度长连接 body。
- 3xx 默认不跟随，输出明确错误。
- 4xx/5xx 返回 status error。
- response header 上限 32 KiB。

WS pull 必须支持：

- upgrade 101 校验。
- `Sec-WebSocket-Accept` 校验。
- binary frame。
- continuation reassembly。
- ping/pong。
- close。
- message 上限 4 MiB。

输出事件建议：

```rust
pub enum Fmp4PullEvent {
    ResponseHead { status: u16, headers: Vec<(String, String)> },
    Bytes(Bytes),
    Closed { reason: String },
}
```

## 2.6 验收命令

```bash
cargo fmt
cargo clippy -p cheetah-fmp4-core
cargo test -p cheetah-fmp4-core
cargo clippy -p cheetah-fmp4-driver-tokio
cargo test -p cheetah-fmp4-driver-tokio
```

