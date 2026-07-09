# Phase 02: RTP 四类传输矩阵

- 状态：计划中
- 范围：补齐 RTP over UDP、RTP over TCP interleaved、RTP over HTTP tunnel、RTP multicast 四类传输的 driver/runtime/module 资源管理。
- 完成标准：同一条本地 engine stream 能通过 UDP、TCP、HTTP tunnel、multicast 播放；RTSP publish 至少支持 UDP、TCP、HTTP tunnel；所有 socket、tunnel、multicast sender 都能在 PAUSE/TEARDOWN/stop 时释放。

## 目标文件与模块

重点修改：

```text
crates/runtime/cheetah-runtime-api/src/lib.rs
crates/runtime/cheetah-runtime-tokio/src/lib.rs
crates/protocols/rtsp/core/src/core/interleaved.rs
crates/protocols/rtsp/core/src/core/tunnel.rs
crates/protocols/rtsp/driver-tokio/src/server/mod.rs
crates/protocols/rtsp/driver-tokio/src/server/listener.rs
crates/protocols/rtsp/driver-tokio/src/server/connection.rs
crates/protocols/rtsp/driver-tokio/src/server/command.rs
crates/protocols/rtsp/module/src/module/transport_selection.rs
crates/protocols/rtsp/module/src/module/play.rs
crates/protocols/rtsp/module/src/module/publish.rs
crates/protocols/rtsp/module/src/session.rs
```

建议新增：

```text
crates/protocols/rtsp/driver-tokio/src/server/http_tunnel.rs
crates/protocols/rtsp/driver-tokio/src/server/tunnel_registry.rs
crates/protocols/rtsp/module/src/module/udp_ports.rs
crates/protocols/rtsp/module/src/module/multicast.rs
crates/protocols/rtsp/module/tests/http_tunnel.rs
crates/protocols/rtsp/module/tests/multicast.rs
```

## Runtime API 扩展

当前 `AsyncUdpSocket` 只有 `recv_from`、`send_to`、`local_addr`，不足以表达 multicast。新增 runtime-neutral 能力：

```rust
pub struct UdpSocketOptions {
    pub reuse_addr: bool,
    pub reuse_port: bool,
    pub multicast_ttl_v4: Option<u32>,
    pub multicast_loop_v4: Option<bool>,
}

pub trait AsyncUdpSocket: Send + Sync {
    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<UdpRecvMeta>;
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize>;
    fn local_addr(&self) -> io::Result<SocketAddr>;
    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()>;
    fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()>;
    fn set_multicast_ttl_v4(&self, ttl: u32) -> io::Result<()>;
}
```

如果不想扩大 trait，可新增 `RuntimeApi::bind_udp_with_options` 和 `MulticastUdpSocket` adapter；但不能直接把 Tokio socket 暴露给 module。

## UDP Unicast

目标行为：

- 支持 server publish UDP ingest：客户端 `ANNOUNCE -> SETUP client_port -> RECORD` 后向 server_port 发 RTP/RTCP。
- 支持 server play UDP egress：客户端 `DESCRIBE -> SETUP client_port -> PLAY` 后从 server_port 收 RTP/RTCP。
- 支持 outbound pull/push UDP，Phase 04 复用同一 socket primitive。
- 支持 UDP 打洞：SETUP 后可发送小型 probe packet 到对端 RTP/RTCP；是否启用由配置决定。
- 端口分配使用有界端口池，优先偶数 RTP + RTP+1 RTCP；如果系统随机端口不成对，需要重试有限次数。
- 目的地址默认取 RTSP TCP peer IP，除非 `destination` 被允许且通过安全校验。

响应示例：

```text
Transport: RTP/AVP;unicast;client_port=5000-5001;server_port=62000-62001;ssrc=11223344
```

## TCP Interleaved

目标行为：

- 支持 SETUP 中明确 `interleaved=x-y`。
- 配置允许时，客户端缺 `interleaved` 可按 track setup order 自动分配 `0-1`、`2-3`。
- interleaved payload 长度不超过 `u16::MAX` 和 configured max；超过则 packetizer 必须分包或返回错误，不可产生非法 frame。
- RTCP 与 RTP 通道冲突必须拒绝。
- PAUSE 后不继续发送 RTP，但可以按策略发送/接收 RTCP；TEARDOWN 发送 BYE 后关闭。

## RTSP-over-HTTP Tunnel

driver 设计：

```text
TCP accept
  -> peek first bytes
  -> RTSP direct: existing connection path
  -> HTTP GET/POST with x-sessioncookie: tunnel path
```

GET half：

- 校验 `GET <path> HTTP/1.0|1.1`。
- 必须存在 `x-sessioncookie`。
- 返回 `HTTP/1.0 200 OK`、`Content-Type: application/x-rtsp-tunnelled`、`Cache-Control: no-cache`。
- 作为逻辑 RTSP connection 的 write half。

POST half：

- 校验 `POST <path> HTTP/1.0|1.1`。
- 必须存在同一 `x-sessioncookie`。
- `Content-Type` 必须是 `application/x-rtsp-tunnelled`，兼容大小写。
- body 以 streaming base64 decode 后喂给逻辑 RTSP core。
- `Content-Length` 只作为上界提示；不能等完整 body 才处理。

Tunnel registry：

```rust
pub struct RtspTunnelRegistryConfig {
    pub max_pending_tunnels: usize,
    pub pending_timeout_ms: u64,
    pub max_decoded_chunk_bytes: usize,
    pub max_base64_buffer_bytes: usize,
}
```

资源规则：

- GET 先到或 POST 先到都可等待配对，但 pending 有上限和超时。
- 任一半连接关闭，逻辑连接关闭并清理另一半。
- 未配对 pending 超时返回/关闭，不泄漏 session。
- tunnel 内 `$` interleaved RTP/RTCP 复用现有 core event。

## Multicast PLAY

配置：

```rust
pub struct RtspMulticastConfig {
    pub enabled: bool,
    pub group_start: std::net::Ipv4Addr,
    pub group_end: std::net::Ipv4Addr,
    pub port_start: u16,
    pub port_end: u16,
    pub ttl: u8,
    pub interface: std::net::Ipv4Addr,
    pub max_groups: usize,
    pub idle_release_ms: u64,
}
```

行为：

- 只允许 administratively scoped IPv4 multicast 地址，默认 `239.0.0.0/8` 范围内配置。
- 每个 stream/track 分配 group + rtp/rtcp port，或同 stream 多轨共享 group 不同端口；首版建议每 track 单 port pair，状态简单。
- 多个 multicast player 复用 sender；PLAY 后 sender 开始订阅 engine stream。
- 对没有 multicast SETUP 的普通 UDP/TCP player 没有影响。
- TEARDOWN 只删除该 player session；最后一个 player 离开后按 idle grace 释放 sender。

响应示例：

```text
Transport: RTP/AVP;multicast;destination=239.1.2.3;port=62000-62001;ttl=16;ssrc=11223344
```

## 具体任务

### 2.1 标准化 UDP unicast endpoint 和端口池

- [ ] 新增 `udp_ports.rs`，实现 bounded even/odd port pair allocator。
- [ ] SETUP UDP 时使用 allocator 分配 server RTP/RTCP socket，响应回 `server_port`。
- [ ] 增加 destination 安全校验，默认只允许 peer IP。
- [ ] 增加 UDP 打洞配置和最小 probe 发送。
- [ ] 增加测试：端口成对、端口池耗尽返回 461/500、third-party destination 默认拒绝、PAUSE 不转发 RTP、TEARDOWN 释放端口。

### 2.2 强化 TCP interleaved 和大包/通道边界

- [ ] SETUP 复用 Transport candidate selection，支持缺省 interleaved 自动分配。
- [ ] 所有 interleaved channel 冲突都在 setup 前检测。
- [ ] packetize MTU 对 TCP 设置为不超过 `u16::MAX - RTP_HEADER_MARGIN`。
- [ ] 对单帧无法分包的 codec 返回可观测错误并跳帧，不关闭整个 module。
- [ ] 增加测试：缺省 interleaved、冲突通道、oversize frame、RTCP BYE channel、pause/play 后 seq/rtcp continuity。

### 2.3 实现 RTSP-over-HTTP tunnel

- [ ] core 新增 HTTP tunnel 纯 parser/base64 streaming helper，或 driver 内保持 Sans-I/O helper。
- [ ] driver listener 根据首包区分 RTSP direct 与 HTTP tunnel。
- [ ] 实现 tunnel registry，按 `x-sessioncookie` 配对 GET/POST。
- [ ] GET write half 输出 HTTP 200 后承载 RTSP response 与 interleaved media。
- [ ] POST read half streaming base64 decode 后喂给逻辑 RTSP core。
- [ ] module 不感知 tunnel，仍按 `RtspConnectionId` 处理事件。
- [ ] 增加端到端测试：HTTP tunnel DESCRIBE/SETUP/PLAY 收到 `$` RTP；HTTP tunnel ANNOUNCE/RECORD 能推入 engine。

### 2.4 实现 RTP multicast PLAY

- [ ] 扩展 runtime API 和 Tokio adapter 支持 multicast socket option。
- [ ] 新增 multicast address/port pool 与 sender registry。
- [ ] SETUP multicast 时分配或复用 sender，响应 destination/port/ttl/ssrc。
- [ ] PLAY 时启动或 attach multicast sender，订阅 engine stream 并 packetize 到 multicast group。
- [ ] TEARDOWN/timeout 后 detach，最后订阅者离开后延迟释放。
- [ ] 增加测试：SETUP multicast response、两个 player 复用 sender、最后 player 离开释放、multicast disabled 返回 461。

## 测试要求

- driver tests 使用真实 loop/socket，但 module 公共接口仍 runtime-neutral。
- HTTP tunnel tests 必须覆盖 GET 先到、POST 先到、cookie mismatch、base64 分片、pending timeout。
- Multicast tests 若 CI 环境不允许组播收包，至少做 socket option 和 sender registry 行为测试；本地 smoke 再做真实 UDP 收包。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-runtime-api
cargo test -p cheetah-runtime-api
cargo clippy -p cheetah-runtime-tokio
cargo test -p cheetah-runtime-tokio udp
cargo clippy -p cheetah-rtsp-driver-tokio
cargo test -p cheetah-rtsp-driver-tokio http_tunnel
cargo clippy -p cheetah-rtsp-module --tests
cargo test -p cheetah-rtsp-module udp_forwarding
cargo test -p cheetah-rtsp-module http_tunnel
cargo test -p cheetah-rtsp-module multicast
```
