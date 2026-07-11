# Phase 02 — NACK/ARQ、延迟缓冲、Listener 加固与弱网观测

- **状态**: 待执行
- **范围**: 对齐 ZLM 在可靠传输与缓冲上的工程目标——可配置 latency/buffer、可观测 NAK/重传/丢包、Listener 超时与上限可靠；用弱网矩阵验收。不自研 NACK 状态机。
- **完成标准**: driver stats 与 module metrics 覆盖重传/丢包/RTT/jitter/loss list；latency 与 buffer 配置生效；1%/5%/12% loss 与限速场景有自动化或可重复脚本；peer 版本拒绝（若 Phase 01 类型已就绪）在 driver 闭环。

---

## 实现概览

ZLM 在 `NackContext` + `PacketQueue` + `PacketSendQueue` 中手写 ARQ。Cheetah 对应能力在 `shiguredo_srt` 内（ACK/NAK/retransmit/TSBPD/TLPKTDROP）。本阶段目标是：

1. **配置对齐** ZLM 的 `latencyMul` / `pktBufSize` / `timeoutSec` 语义。
2. **观测对齐** 运维能看到与 ZLM 同等有用的丢包/重传数据。
3. **验收对齐** 弱网下推拉仍可读，不静默卡死。

参考：

| 来源 | 路径 |
|------|------|
| ZLM NACK | `vendor-ref/ZLMediaKit/srt/NackContext.*` |
| ZLM 队列 | `PacketQueue.*`、`PacketSendQueue.*` |
| ZLM Transport | `SrtTransport.cpp` ACK/NAK 路径 |
| 本地 driver | `crates/protocols/srt/driver-tokio/src/driver.rs` |
| 本地 metrics | `crates/protocols/srt/module/src/metrics.rs` |
| 库 | `shiguredo_srt` `srt_connection` / `srt_receiver` / `srt_sender` / stats |
| 基线互操作 | [plans-28-srt/srt-ops-interop.md](../plans-28-srt/srt-ops-interop.md) |

---

## 2.1 配置透传

**文件**: `module/src/config.rs`、`driver-tokio/src/config.rs`、`driver.rs` 构建 `ConnectionOptions`

| 配置 | 对齐 ZLM | 映射 |
|------|----------|------|
| `latency_ms` | TSBPD delay（ZLM 还有 `latencyMul * RTT` 建议） | `tsbpd_delay` |
| `latency_mul` | `srt.latencyMul` 默认 4 | 文档 + 可选用于动态建议；首版可只存配置与 metrics 展示 |
| `pkt_buf_size` / `recv_buffer_packets` | `srt.pktBufSize` 默认 8192 | 收发缓冲上界（库选项或 driver 侧 cap） |
| `idle_timeout_ms` / `connect_timeout_ms` | `timeoutSec` | 已有，补边界测试 |
| `max_connections` | 连接上限 | 已有，补满载拒绝指标 |
| `send_queue_capacity` | 发送背压 | 已有，保留 overflow 策略 |

校验：

- 所有缓冲/队列 > 0 且有合理上限。
- `latency_ms` 落在库可接受范围（注意 `u16` 截断，已有 `min(u16::MAX)`）。

---

## 2.2 Stats 与 Metrics 增强

### Driver `SrtDriverStats`（扩展/确认）

已有字段继续保留：

- bytes/packets in/out
- sender: packets_in_buffer、loss_list、total_retransmits、total_sent
- receiver: buffer、loss_list、total_received/lost/duplicates、rtt、rtt_var、loss_rate、jitter

建议增量（以 `shiguredo_srt` 实际暴露为准）：

| 字段 | 用途 |
|------|------|
| `nak_sent` / `nak_received` | 对齐 ZLM NAK 路径可观测 |
| `packets_retransmitted_rx` | 收到的重传标记包（若可得） |
| `tlpktdrop_count` | 过期丢弃；库无则文档标注 N/A |
| `peer_srt_version` | 握手后记录 |
| `connected_ms` | 会话时长 |

### Module metrics

- 增量聚合 retransmit / lost / nak。
- `handshake_reject_total{reason=version|auth|streamid|capacity|...}`。
- Prometheus 名保持 `srt_*` 前缀，避免破坏已有 scrape。

### 事件

可选：`SrtDriverEvent::Diagnostic { peer_id, kind, message }` 用于版本拒绝、缓冲满、idle timeout 等，便于 module 打点。

---

## 2.3 Peer 版本拒绝闭环

依赖 Phase 01 版本类型。

**Driver 行为**：

1. 握手完成后读取 peer HS 扩展 `srt_version`（`shiguredo_srt` API 调研：`HsExtensionData` / connection 状态）。
2. 若 `< min_peer_srt_version` → 关闭连接，事件 `Disconnected` 或 `Error` 带 reason `peer_version_too_old`。
3. 若无扩展且 `require_peer_version_extension=true` → 同样拒绝。
4. 本端宣告 version 使用配置 `local_srt_version`（若库允许设置 `ConnectionOptions.srt_version`）。

测试：

- 单元：版本比较纯函数。
- 集成：若可构造低版本 peer（测试桩或改 options）则断言拒绝；否则用 mock/解析 fixture。

---

## 2.4 Listener 加固

确认并补测：

| 项 | 期望 |
|----|------|
| `ListenerStarted` | 绑定成功可观测 |
| induction/conclusion 并发 | 多 caller 同时连不串 session |
| `max_connections` | 超额拒绝/忽略并计数 |
| idle timeout | 无数据超时断开 |
| connect timeout | caller 未完成握手断开 |
| send queue full | 指标 + 可选断连（已有 `disconnect_on_send_queue_overflow`） |

对齐 ZLM：`SrtTransportManager` 按 socket id / sync cookie 查找；本地按 `SocketAddr`/peer_id 管理即可，不强制复制 cookie map，但 conclusion 重传必须稳定（库职责）。

---

## 2.5 弱网验收矩阵

将 [plans-28-srt](../plans-28-srt/index.md) 已做过的 netem 收编为 **正式回归**（脚本或 ignored 集成测试 + 文档命令）：

| 场景 | netem 示例 | 期望 |
|------|------------|------|
| 轻损 | `loss 1% delay 50ms 10ms reorder 1%` | 推流可 demux；重传计数上升；播放可读 |
| 中损 | `loss 5% ... reorder 5%` | 仍可建立并出帧；延迟增加可接受 |
| 重损 | `loss 12% delay 80ms 30ms reorder 10%` | 不 panic；允许花屏/卡顿；连接可恢复或明确断开 |
| 限速 | `rate 1200kbit delay 20ms 5ms` | 背压/丢包策略生效；不拖死 event loop |
| 慢 peer | `send_queue_capacity` 极小 | queue full 指标；策略符合配置 |

实现建议：

- `driver-tokio/tests/` 或 `module/tests/` 下 `#[ignore]` + `REQUIRED_NETEM=1`。
- 无 root/`tc` 时跳过并打印原因（与现有环境一致）。

---

## 2.6 与 ZLM NACK 行为对照（验收用，非实现清单）

| ZLM 行为 | Cheetah 验收点 |
|----------|----------------|
| 丢包进入 nack map | `receiver_packets_in_loss_list` 或 lost 递增 |
| RTT 后重复 NAK | 持续 loss 时 retransmit/nak 相关计数增长 |
| 收到后 drop seq | loss list 下降；duplicates 可统计 |
| TLPKTDROP 过期丢 | 高延迟下不无限涨缓冲；有上界 |
| 发送侧重传缓存上界 | send buffer / queue 有 cap |

若实测库行为与上表严重不符，记录 **库 gap** 并开 follow-up（升级/补丁），**不在本 phase 复制 NackContext**。

---

## 2.7 测试清单

- driver：stats 字段非零（loopback 加人工 loss 或 mock）。
- driver：max_connections、idle、connect timeout 回归。
- driver/module：metrics 导出含 retransmit/lost。
- version：比较与拒绝路径。
- 弱网：上表矩阵（ignore 可接受，但命令写入 ops）。

---

## 2.8 验收命令

```bash
cargo fmt
cargo clippy -p cheetah-srt-driver-tokio
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-driver-tokio
cargo test -p cheetah-srt-module

# 可选弱网（需 tc 权限）
# sudo tc qdisc add dev lo root netem loss 5% delay 50ms 10ms
# ... 运行 ffmpeg publish + ffplay / 集成测试 ...
# sudo tc qdisc del dev lo root
```

---

## 关键文件

| 动作 | 路径 |
|------|------|
| 改 | `crates/protocols/srt/driver-tokio/src/{config,driver}.rs` |
| 改 | `crates/protocols/srt/module/src/{config,metrics,http}.rs` |
| 增 | driver/module 弱网或 stats 测试 |
| 参考 | `vendor-ref/ZLMediaKit/srt/NackContext.*`、`SrtTransport.cpp` |
| 参考 | `shiguredo_srt` stats / ConnectionOptions |

---

## 本阶段不做

- 不实现 FEC（Phase 04）。
- 不改 streamid 默认 mode 以外的业务（应在 Phase 01 完成）。
- 不在 cheetah 内重写 NAK 调度状态机。
- 不要求 24h 长稳（可在 Phase 05 记录）。
