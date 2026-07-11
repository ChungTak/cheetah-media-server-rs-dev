# SRT 差距分析（本地代码 vs 兼容规范）

兼容规范全文见 [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md)。  
本文只对照 **本仓库当前代码**，供智能体定位修改点。

---

## 1. 当前实现摘要

### 1.1 `cheetah-srt-core` — `stream_id.rs`

```text
ParsedSrtStreamId {
  stream_key, mode, user, host, session, extras
}
```

行为：

- 支持 `#!::` 与 **bare** key（无前缀也 OK）  
- `r` 整串进入 `stream_key`，**不拆** app/stream，**不校验两段**  
- `m` 映射为 mode 后从 fields **移除**，不进入 extras  
- `h` → `host`，module **未使用**  
- 缺 `m` → `mode=None`  

### 1.2 `cheetah-srt-module` — `config.rs` / `module.rs`

- `ingress.default_mode` 默认 **`"publish"`**（与规范相反）  
- `classify_stream`：`mode = parsed.mode.unwrap_or(default_mode)`  
- `stream_key_from_string("live/test")` → `StreamKey{namespace:live, path:test}`；单段 → `namespace=live`  
- `authorize_stream`：只看 `extras["token"]` 与 `user`  
- Connected 后 publish → demux；play → `run_play_session`  
- jobs forced mode 优先于 streamid  

### 1.3 `cheetah-srt-driver-tokio` — `driver.rs`

- UDP listener + connection map  
- `shiguredo_srt::SrtConnection` 驱动 ACK/NAK/retransmit  
- stats：retransmit、lost、rtt、jitter、loss_list 等  
- **无** peer 版本比较  
- **无** FEC  
- `ConnectionOptions { tsbpd_delay, passphrase, stream_id, ...Default }`  

### 1.4 依赖库 `shiguredo_srt`

- LiveCC、ACK/NAK、TSBPD、TLPKTDROP、AES、Stream ID 扩展  
- 默认 `srt_version = 0x010500`  
- **无** Packet Filter / FEC API  

---

## 2. 逐项差距 → 修改指引

| ID | 规范要求 | 当前 | 改哪里 | Phase |
|----|----------|------|--------|-------|
| G1 | 无 m → 拉流 | 默认 publish | `module/src/config.rs` Default；文档/测试 | 01 |
| G2 | r 必须 app/stream | 整串 | `core/src/stream_id.rs` | 01 |
| G3 | h → vhost | host 闲置 | stream_id + classify | 01 |
| G4 | auth 含 m 与全部其它 key | m 被吃掉 | stream_id 保留 auth_params | 01 |
| G5 | 严格 #!:: | bare OK | parse options | 01 |
| G6 | 版本 ≥1.3.0 策略 | 无 | core version + driver | 01/02 |
| G7 | NACK 可观测弱网 | 部分 | driver stats/metrics/tests | 02 |
| G8 | pktBuf/latencyMul | 仅 latency_ms | config 透传 | 02 |
| G9 | 非法/无源关闭 | 大体有，需对齐 reason | module | 03 |
| G10 | 代码可维护 | module.rs 过大 | 拆分 | 03 |
| G11 | FEC | 无 | core+driver+module | 04 |
| G12 | 客户端矩阵 | 零散 | tests/docs | 05 |

---

## 3. 行为对照：握手后

规范状态机见参考文档 §3。

本地 `handle_driver_event(Connected)` 已分支 publish/play，但：

- classify 默认 mode 错误（G1）  
- stream key 不来自严格 app/stream（G2）  
- auth 参数不全（G4）  

Payload 仅 ingress session 处理 — 符合「player 忽略推流数据」方向。

---

## 4. NACK 对照

| 规范行为 | 本地 |
|----------|------|
| 丢包 → NAK | 库内 |
| RTT 后再 NAK | 库内 |
| 发送重传 | 库内 |
| 指标 | driver stats 有 retransmit/lost；metrics 有 retransmit |
| 弱网正式矩阵 | 需 Phase 02 固化 |

**不要** 在 cheetah 复制完整 NackContext，除非实测库失败并记录证据。

---

## 5. 版本 / FEC 对照

| 项 | 规范 | 本地 |
|----|------|------|
| 宣告版本 | ≥1.3，建议 1.5.0 | 库默认 1.5.0 |
| 拒绝旧 peer | 需要 | 无 |
| PACKET_FILTER flag | FEC 用 | 未用 |
| FEC 编解码 | 本项目要做 | 无 |

---

## 6. 风险

1. 默认 mode 与严格 r 为 **行为破坏性变更** → 配置开关。  
2. FEC 可能受阻于 `shiguredo_srt` → Phase 04 决策表。  
3. `module.rs` 继续堆逻辑会违反 AGENTS 行数建议。  
4. VLC streamid 不在 URL，只在 HS：测试必须用真实 streamid 握手路径，不能只测 URL parser。  

---

## 7. 智能体定位命令

```bash
# 当前 streamid 解析
rg -n "parse_srt_stream_id|ParsedSrtStreamId|default_mode|classify_stream|authorize_stream" \
  crates/protocols/srt

# driver 选项与 stats
rg -n "ConnectionOptions|SrtDriverStats|tsbpd_delay|sender_total_retransmits" \
  crates/protocols/srt/driver-tokio

# 配置默认
rg -n "default_mode|latency_ms" crates/protocols/srt/module/src/config.rs
```
