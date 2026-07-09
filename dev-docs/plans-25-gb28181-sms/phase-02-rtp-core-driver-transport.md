# Phase 02 — RTP Core + Driver 传输层

- **状态**: 已完成
- **范围**: 新增 `cheetah-rtp-core` 与 `cheetah-rtp-driver-tokio`，实现 UDP/TCP RTP/RTCP、session 路由、主动/被动 client/server 传输壳
- **完成标准**: 不接入 engine 的情况下，driver 可建立 RTP server/client、解析 UDP/TCP 输入、发送 RTP/RTCP、维护 SSRC/session 生命周期

---

## 2.1 `cheetah-rtp-core` Sans-I/O 状态机

新增 crate：

```text
crates/protocols/rtp/core
```

职责：

- RTP/RTCP session 路由
- SSRC、source address、payload mode、transport mode 状态管理
- TCP 2-byte framing 解析与输出模型
- session timeout、diagnostic 和 event 输出
- 与共享媒体层的 `TrackInfo + AVFrame` 事件对接

核心类型：

```rust
pub enum RtpCoreInput {
    UdpPacket(RtpDatagram),
    TcpBytes(RtpTcpChunk),
    Tick { now_ms: u64 },
    Command(RtpCoreCommand),
}

pub enum RtpCoreCommand {
    CreateServer(RtpServerSpec),
    CreateClient(RtpClientSpec),
    SendFrame(RtpSendFrame),
    StopSession(RtpSessionKey),
}

pub enum RtpCoreOutput {
    SendUdp(RtpUdpSend),
    SendTcp(RtpTcpSend),
    SendRtcp(RtcpSend),
    Event(RtpCoreEvent),
    Diagnostic(RtpCoreDiagnostic),
    CloseSession(RtpSessionKey),
}
```

路由规则：

- 优先按显式 `session_key` 路由
- 未显式绑定时按 `ssrc` 路由
- 再次未知时可按默认 `/live/{ssrc}` 创建接收上下文
- UDP 会话锁定首个来源地址；兼容模式允许重绑定一次

---

## 2.2 Transport Mode 与 Payload Mode

支持模式：

```text
transport_mode:
  recv_only
  send_only
  send_recv

payload_mode:
  ps
  ts
  es
  ehome
```

设计要求：

- `recv_only` 只 ingest 不出站
- `send_only` 只从本地流向远端发 RTP
- `send_recv` 同一 session 同时收发
- `ps/ts/es/ehome` 决定内部 media pipeline 和默认 codec 提示
- raw ES 模式若无 codec hint，核心层只输出 diagnostic，不自行猜测高层业务

---

## 2.3 UDP/TCP/RTCP Driver

新增 crate：

```text
crates/protocols/rtp/driver-tokio
```

职责：

- UDP bind/recv/send
- TCP bind/accept/connect
- TCP 2-byte framing read/write
- RTCP SR/RR/SDES/BYE 最小可用集
- 有界 write queue、backpressure、graceful shutdown
- timer、spawn、cancellation 统一走 runtime 抽象

实现要求：

- UDP/TCP 同端口和分离端口都可配置
- `socketType=both` 时同时开启 UDP/TCP 收流
- RTCP 可选独立 UDP 端口
- 单连接 `write_queue_capacity` 有界
- 读缓存、TCP remain buffer、RTCP packet 缓存有上限

---

## 2.4 Client / Server 模型

server：

- 支持 `create` 和 `stop`
- 支持显式 `appName + streamName + ssrc` 映射
- 也支持先开端口，后按到包的 `ssrc` 自动建接收上下文

client：

- 支持 `create`、`start`、`stop`
- `create` 阶段仅分配本地 socket 和 session key
- `start` 阶段真正开始主动连接或主动发送
- `stop` 释放 socket、session、publisher/subscriber 句柄

返回信息：

- `session_key`
- `ssrc`
- `local_ip`
- `local_port`
- `rtcp_port`
- `transport_mode`
- `payload_mode`

---

## 2.5 Driver 集成测试

测试场景：

1. UDP server 接收 RTP-PS 并产出 event
2. UDP server 接收 RTP-TS 并产出 event
3. TCP server 接收 2-byte framed RTP 并支持半包/粘包
4. send_only session 正常发 RTP
5. send_recv session 同时收发
6. source address 漂移默认拒绝
7. `socketType=both` 同时起 UDP/TCP
8. RTCP 基础回应可用
9. idle timeout 自动清理 session
10. write queue full 只关闭单连接

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-rtp-core
cargo test -p cheetah-rtp-core
cargo clippy -p cheetah-rtp-driver-tokio
cargo test -p cheetah-rtp-driver-tokio
```
