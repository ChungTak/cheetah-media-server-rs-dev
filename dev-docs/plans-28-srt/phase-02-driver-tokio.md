# Phase 02 — Tokio Driver

- **状态**: 部分完成
- **范围**: SRT UDP listener/caller、`shiguredo_srt::SrtConnection` 驱动循环、timer、连接表、背压、加密、统计
- **完成标准**: driver 集成测试可在本机创建 listener 和 caller，完成握手、数据收发、断开和统计上报；尚不要求接入引擎

---

## 2.1 Driver 配置和事件

`cheetah-srt-driver-tokio/src/config.rs`：

```rust
pub struct SrtDriverConfig {
    pub listen: SocketAddr,
    pub max_connections: usize,
    pub idle_timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub latency_ms: u64,
    pub stats_interval_ms: u64,
    pub recv_buffer_packets: usize,
    pub send_queue_capacity: usize,
    pub encryption: SrtDriverEncryption,
}
```

`cheetah-srt-driver-tokio/src/lib.rs` 导出：

```rust
pub use config::{SrtDriverConfig, SrtDriverEncryption};
pub use driver::{spawn_driver, SrtDriverHandle};
pub use event::{SrtDriverCommand, SrtDriverEvent, SrtPeerId};
```

命令：

```rust
pub enum SrtDriverCommand {
    ConnectCaller {
        peer_id: SrtPeerId,
        remote: SocketAddr,
        stream_id: Option<String>,
        options: SrtSessionOptions,
    },
    SendPayload {
        peer_id: SrtPeerId,
        payload: Bytes,
    },
    Close {
        peer_id: SrtPeerId,
        reason: String,
    },
}
```

事件：

```rust
pub enum SrtDriverEvent {
    ListenerStarted { local_addr: SocketAddr },
    CallerConnecting { peer_id: SrtPeerId, remote: SocketAddr },
    Connected { peer_id: SrtPeerId, remote: SocketAddr, stream_id: Option<String> },
    Payload { peer_id: SrtPeerId, payload: Bytes },
    KeyRefreshNeeded { peer_id: SrtPeerId },
    Stats { peer_id: SrtPeerId, stats: SrtDriverStats },
    Disconnected { peer_id: SrtPeerId, reason: String },
    Error { peer_id: Option<SrtPeerId>, message: String },
}
```

---

## 2.2 Listener UDP loop

实现 `spawn_driver(config) -> SrtDriverHandle`：

1. Bind UDP socket。
2. 创建 connection map：`HashMap<SocketAddr, ConnectionSlot>`。
3. 创建 bounded command/event channel。
4. 循环处理：
   - UDP datagram。
   - driver command。
   - timer tick / per-connection timer。
   - cancel signal。

连接创建：

- 收到未知 remote 的 SRT handshake packet 时创建 listener-side `SrtConnection`。
- 如果 `max_connections` 已满，丢弃并上报 `Error`。
- handshake 完成后发 `Connected`，携带 Stream ID。

连接路由：

- listener map key 使用 remote `SocketAddr`。
- caller map key 使用 `SrtPeerId`。
- 如果 NAT rebinding 后 remote 改变，v1 先按断开处理；后续单独做 source change 支持。

---

## 2.3 Caller 连接

Caller 用于：

- 从远端 SRT listener 拉流。
- 向远端 SRT listener 推流。
- relay job 的 source/target。

流程：

1. module 发送 `ConnectCaller`。
2. driver 创建 UDP socket 或复用配置 socket。
3. 创建 caller-side `SrtConnection`。
4. 立即 drain `ConnectionOutput`，发送 handshake packet。
5. 在 `connect_timeout_ms` 内未 `Connected` 则断开并上报错误。

当前实现已接入 caller-side connect deadline：超时后移除连接、清理 remote 映射，并发送 `Disconnected { reason: "connect timeout" }`。后续如需要区分告警级别，可再补充并行的 `Error` 诊断事件。

---

## 2.4 驱动 `shiguredo_srt`

`connection.rs` 负责把库输出变成 driver 动作：

```rust
fn drain_outputs(slot: &mut ConnectionSlot, out: &mut Vec<SrtDriverEvent>) {
    while let Some(action) = slot.connection.pop_output() {
        match action {
            ConnectionOutput::SendPacket(packet) => {
                slot.pending_udp.push_back(packet);
            }
            ConnectionOutput::SetTimer(timer) => {
                slot.timer_deadline = Some(timer);
            }
            ConnectionOutput::ClearTimer => {
                slot.timer_deadline = None;
            }
            ConnectionOutput::Event(event) => {
                translate_connection_event(slot.peer_id, event, out);
            }
        }
    }
}
```

实现要求：

- 所有调用都传入显式 `Timestamp` / `now`，driver 从 Tokio clock 读取后注入，core 不读系统时间。
- `DataReceived` 翻译为 `SrtDriverEvent::Payload`。
- `Connected` 翻译为 `SrtDriverEvent::Connected`。
- `Disconnected` 翻译为 `SrtDriverEvent::Disconnected`。
- `KeyRefreshNeeded` 翻译为 driver event，module 或 driver 按配置触发 key refresh。

---

## 2.5 Timer 模型

每个 connection 维护一个 deadline：

```rust
struct ConnectionSlot {
    peer_id: SrtPeerId,
    remote: SocketAddr,
    timer_deadline: Option<Instant>,
    last_activity: Instant,
    pending_udp: VecDeque<Bytes>,
    pending_payload: VecDeque<Bytes>,
}
```

driver loop 每轮计算最近 deadline：

- 最早 SRT timer。
- idle timeout。
- stats interval。
- command recv。
- UDP recv。

到期后向 `SrtConnection` 注入 timer input，并 drain outputs。

---

## 2.6 背压和资源上界

上界：

- `max_connections`
- `recv_buffer_packets`
- `send_queue_capacity`
- `pending_udp` per connection
- `pending_payload` per connection
- event channel capacity

策略：

- SRT payload send queue 满：按 egress 配置 `DropUntilNextKeyframe` 或断开。
- UDP send 失败：记录错误并触发连接关闭。
- event channel 满：对 stats/diagnostic 可丢弃，对 payload/connected/disconnected 不丢弃，必要时断开连接。
- 慢 SRT peer 不能阻塞 listener loop。

当前实现已在 listener 新 remote 和 caller `ConnectCaller` 两条路径检查 `max_connections`；超过上限时不会创建新连接，并上报 `SRT max_connections reached`。

当前实现也在 `SendPayload` 路径检查 `send_queue_capacity`；当 `shiguredo_srt` sender buffer 中未 ACK 包数量达到配置上限时，driver 拒绝新的 payload 并上报 `SRT send queue full`。本地集成测试已覆盖 `send_queue_capacity=0` 的边界拒绝行为；真实慢 peer 饱和压力仍需外部长跑验证。

---

## 2.7 加密和 Key Refresh

配置：

```rust
pub struct SrtDriverEncryption {
    pub enabled: bool,
    pub passphrase: String,
    pub key_length: SrtKeyLength,
}

pub enum SrtKeyLength {
    Aes128,
    Aes256,
}
```

行为：

- encryption disabled：不设置 passphrase。
- enabled 但 passphrase 为空：module 配置校验失败；直接使用 driver crate 时，driver 启动前也会拒绝并上报配置错误。
- Caller job URL 显式提供 `passphrase=` 空值时，module 在转换为 driver options 前返回配置错误。
- peer 加密配置不匹配：连接失败并上报明确错误。
- `KeyRefreshNeeded`：driver 调用 `shiguredo_srt` 对应 KM refresh API；如果 API 需要上层决策，则 module 收到事件后发送 command。

当前本地集成测试已覆盖：

- enabled 但 passphrase 为空时，driver 启动前拒绝并发出配置错误。
- AES-128 passphrase 匹配时，本机 caller/listener 可完成握手并完成加密 payload 回环。
- AES-256 passphrase 匹配时，本机 caller/listener 可完成握手并完成加密 payload 回环。
- passphrase 不匹配时，caller 不会进入 `Connected`，并在 connect deadline 后断开或收到错误。

---

## 2.8 统计

统计项：

- active connections
- bytes in/out
- packets in/out
- retransmit count
- lost packets / NAK count
- RTT EWMA
- send queue depth
- receive queue depth
- key refresh count
- disconnect reason

driver 周期性发送：

```rust
SrtDriverEvent::Stats { peer_id, stats }
```

module 负责转为 SRT module 本地 metrics 指标，不在 driver 直接依赖 SDK。

当前实现已覆盖 bytes/packets in/out 的周期性 `Stats` 事件，并从 `shiguredo_srt` 的 `sender_stats()` / `receiver_stats()` 采集 sender buffer depth、loss list depth、retransmit count、receiver lost/duplicate packet count、RTT、RTT variance、loss rate 和 jitter。`KeyRefreshNeeded` 由 driver 事件上报并在 module metrics 中计数。TLPKTDROP 与真实弱网下的 NAK/retransmit 行为仍需外部互操作和 netem 场景验证。

---

## 验证方法

单元和集成测试：

- listener bind 成功并发 `ListenerStarted`。
- caller/listener 本机握手成功。
- caller 发送 payload，listener 收到同样 payload。
- listener 发送 payload，caller 收到同样 payload。
- idle timeout 后发 `Disconnected`。
- `max_connections=1` 时第二个 caller 被拒绝。
- `send_queue_capacity=0` 时 payload 发送被拒绝并上报 `SRT send queue full`。
- stats 事件包含 bytes/packets、receiver packet/byte totals、RTT 和 jitter 字段。
- AES-128/AES-256 加密 passphrase 匹配时握手成功并可收发 payload。
- 加密 passphrase 不匹配时握手失败。

当前 driver 集成测试已覆盖 caller->listener、listener->caller 双向明文 payload 收发，以及 listener->caller 加密 payload 收发。

命令：

```bash
cargo fmt
cargo test -p cheetah-srt-driver-tokio
cargo clippy -p cheetah-srt-driver-tokio
```
