# Phase 02 — NACK/ARQ 观测、延迟缓冲、版本拒绝、弱网（可执行）

- **状态**: 完成  
- **依赖**: Phase 01 版本纯函数与配置字段建议已合并（至少 `min_peer_srt_version` 存在）  
- **兼容规范**: [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md) §4、§5  

## 完成标准（DoD）

- [ ] latency / pkt_buf 配置进入 driver `ConnectionOptions` 或等价上界  
- [ ] metrics 含 retransmit、lost、（若可得）nak；reject 计数  
- [ ] peer 版本过低可拒绝 **或** 文档+代码注释明确「库 API 不可用」并有纯函数测试  
- [ ] 弱网矩阵命令写入测试注释或 `#[ignore]` 测试  
- [ ] clippy/test 通过  

---

## 背景：NACK 不在本 crate 重写

参考规范 §4 描述的丢失表 / RTT 再 NAK / 重传，由 **`shiguredo_srt`** 完成。

智能体任务：

1. 确认库 stats 字段映射完整  
2. 配置 TSBPD delay 与缓冲上界  
3. 用丢包实验证明 retransmit/lost 上升且不 panic  
4. 不要新建 `NackContext` 模块，除非提交证据说明库行为错误且无法升级  

---

## 任务 2.1 — 配置透传

### 文件

- `module/src/config.rs`：`latency_mul: u32`（默认 4）、`pkt_buf_size: usize`（默认 8192）  
- `driver-tokio/src/config.rs`：对齐字段  
- `module` 中 `driver_config(&SrtModuleConfig) -> SrtDriverConfig`（在 `module.rs` 搜索现有函数）  

### 映射

| Module 配置 | Driver | shiguredo / 行为 |
|-------------|--------|------------------|
| `latency_ms` | `latency_ms` | `ConnectionOptions.tsbpd_delay`（已有） |
| `pkt_buf_size` | `recv_buffer_packets` | 上界；若库无独立字段，用于 send 队列与文档 |
| `idle_timeout_ms` | 已有 | 空闲断开 |
| `max_connections` | 已有 | 超额拒绝 |

实现 `driver_config` 时：`recv_buffer_packets = config.pkt_buf_size`（或 max 现有默认）。

---

## 任务 2.2 — Stats / Metrics

### 文件

- `driver-tokio/src/driver.rs`：`SrtDriverStats`  
- `module/src/metrics.rs`、`http.rs`  

### 步骤

1. 阅读 `slot.connection.sender_stats()` / `receiver_stats()` 已有映射（约 driver.rs 680 行附近）。  
2. 列出库公开的全部计数器；能映射的都映射。  
3. Module metrics 增量：  

```text
srt_retransmit_total
srt_receiver_lost_total
srt_receiver_duplicate_total
srt_handshake_reject_total{reason=...}  // 至少 reason 标签或后缀
```

4. `http.rs` Prometheus 文本增加对应行（仿现有 `srt_retransmit_total`）。  

### 测试

- 单元：`metrics.add_stats_delta` 对 retransmit/lost 累加正确（已有 retransmit 测可扩展）。  

---

## 任务 2.3 — Peer 版本拒绝

### 调研步骤（必须先做，结果写在 PR/提交说明）

```bash
# 在本地 cargo registry 中查看 ConnectionOptions / peer 版本 API
rg -n "srt_version|HsExtension|peer_stream" \
  ~/.cargo/registry/src/*/shiguredo_srt-2026.1.0-canary.1/src
```

### 若库暴露 peer `srt_version`

1. Connected 或握手完成时读取 peer version。  
2. `parse_srt_version(&config.min_peer_srt_version)`  
3. 若 `!version_at_least(peer, min)` → `Close { reason: "reject:peer_version_too_old" }` + metrics。  
4. `require_peer_version_extension=true` 且无版本 → 同样拒绝。  
5. 集成测试：如可设置 caller options 低版本则覆盖；否则单测比较逻辑。  

### 若库不暴露

1. 在 `driver.rs` 顶部模块文档注释说明阻塞点。  
2. 仍完成 core 版本单测 + 配置字段存储 + metrics 标签预留。  
3. Phase 02 DoD 勾选「库 API 不可用」分支。  

本端宣告：若 `ConnectionOptions` 有 `srt_version` 字段，设为 `parse(local_srt_version)`。

---

## 任务 2.4 — Listener 加固测试

### 文件

- `driver-tokio/tests/driver_smoke.rs` 或新建 `timeouts.rs`  

### 用例

| 用例 | 做法 | 期望 |
|------|------|------|
| max_connections | 配 1，连 2 | 第二路 Error/拒绝，第一路仍工作 |
| connect_timeout | 极短 timeout，不完成握手 | Disconnected/Error |
| idle_timeout | 连上后不发数据 | 超时断开 |
| send_queue | capacity=0 或极小 | queue full 指标或断开 |

沿用现有测试风格（本机 loopback）。

---

## 任务 2.5 — 弱网矩阵（可 ignore）

新建 `driver-tokio/tests/netem_lossy.rs` 或 module 侧：

```rust
#[tokio::test]
#[ignore = "requires: CHEETAH_SRT_NETEM=1 and tc netem on lo"]
async fn lossy_5_percent_retransmit_observed() { ... }
```

文档化命令（写入测试文件头注释）：

```bash
sudo tc qdisc add dev lo root netem loss 5% delay 50ms 10ms reorder 5%
# 启动 server + ffmpeg publish + 检查 /srt/metrics retransmit/lost
sudo tc qdisc del dev lo root
```

矩阵（与参考 §4 验收对齐）：

| 场景 | netem |
|------|-------|
| 轻 | loss 1% delay 50ms 10ms reorder 1% |
| 中 | loss 5% reorder 5% |
| 重 | loss 12% delay 80ms 30ms reorder 10% |
| 限速 | rate 1200kbit |

期望：进程不 panic；中等损失下业务仍可能 demux；metrics 反映损失/重传。

---

## 验收命令

```bash
cargo fmt
cargo clippy -p cheetah-srt-driver-tokio
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-driver-tokio
cargo test -p cheetah-srt-module
# 可选
# CHEETAH_SRT_NETEM=1 cargo test -p cheetah-srt-driver-tokio -- --ignored
```

## 本阶段不做

- 自研 NACK 状态机  
- FEC  
- streamid 语义（应已在 01）  
