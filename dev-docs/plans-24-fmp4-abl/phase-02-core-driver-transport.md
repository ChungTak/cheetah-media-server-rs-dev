# Phase 02 — Core 与 Driver 传输补强

- **状态**: 规划中
- **范围**: 对齐 ABL 的 HTTP-MP4 直播传输、chunked 行为、TLS、WebSocket 和 pull 字节流兼容。
- **完成标准**: driver 层在不依赖 engine 业务状态时，能稳定处理 ABL 风格 HTTP-MP4 / WS-fMP4 通信。

## 2.1 HTTP 路由与响应

保持现有路径：

- `/{app}/{stream}.mp4`
- `/{app}/{stream}.live.mp4`

补强行为：

- ABL 主路径 `.mp4` 必须长期可用。
- `HEAD` 和 `OPTIONS` 响应继续保留，但不创建播放 session。
- 路径中 `%`、`..`、超长 URL、非法 header 必须快速拒绝。

## 2.2 HTTP chunked 直播

对齐 ABL `NetServerHTTP_MP4` 的关键行为：

- HTTP 200 后使用 `Transfer-Encoding: chunked`。
- 先发 init segment，再发 media fragment。
- chunk 编码逻辑要覆盖大 payload 分段发送。
- 慢客户端填满 write queue 时只关闭该连接，不影响其他连接。
- 允许较大的单次 payload，但总 queue 和 frame 大小必须有上界。

## 2.3 HTTPS/WSS

保持当前 TLS server 基础，并补全验证：

- HTTP/HTTPS 和 WS/WSS 共用相同 core 会话语义。
- cert/key 路径错误、handshake timeout、TLS listener bind 失败必须形成清晰诊断。
- 集成测试覆盖 HTTPS GET 和 WSS upgrade。

## 2.4 WebSocket framing

继续沿用当前实现，并补强实战边界：

- masked client frame。
- continuation reassembly。
- ping/pong。
- close。
- text frame 默认关闭。
- message 上限默认 4 MiB。
- RSV 非 0 拒绝。
- server 输出 binary frame 不 mask。

补测试：

- 分片 binary message 重组后交给上层。
- 超上限 continuation 触发关闭。
- 非 masked client frame 被拒绝。

## 2.5 Pull client

当前 pull 已支持 `http/https/ws/wss` 和 chunked，后续继续补强：

- HTTP 3xx/4xx/5xx 明确报错。
- header 上限和 body 读取边界测试。
- WebSocket continuation、ping/pong、close 的回归测试。
- 远端重复 init、任意 chunk 切分、长连接 `moof/mdat` 流式输入样例。

## 2.6 与 ABL 的差异性选择

不照搬 ABL 的地方：

- 不使用阻塞式 sleep 节流 chunk。
- 不依赖全局发送线程池。
- 不在 driver 内维护录像/回放状态。

采用 Cheetah 方式：

- 每连接 write queue。
- runtime-neutral cancel。
- backpressure 失败时关闭单连接。

## 2.7 验收

```bash
cargo clippy -p cheetah-fmp4-core
cargo test -p cheetah-fmp4-core
cargo clippy -p cheetah-fmp4-driver-tokio
cargo test -p cheetah-fmp4-driver-tokio
```

重点场景：

- GET `.mp4` 收到 chunked init。
- `.live.mp4` 与 `.mp4` 行为一致。
- HTTPS/WSS 可成功握手。
- WebSocket continuation、ping/pong、close 正常。
- pull client 不把 chunk header 误交给 demux。
