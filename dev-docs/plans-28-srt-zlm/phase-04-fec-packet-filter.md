# Phase 04 — FEC / Packet Filter

- **状态**: 待执行
- **范围**: 实现 SRT Packet Filter / FEC 能力（**超越 ZLMediaKit**：ZLM `srt.md` 明确 FEC 未实现）。覆盖配置、握手协商、编解码恢复、降级与指标。
- **完成标准**: 配置开启后可与支持 FEC 的对端协商；人为丢包下 recovered 计数上升且业务负载可恢复；对端不支持时可降级 ARQ-only；`fec.required=true` 时可拒绝不支持 peer。

---

## 背景

### 标准与参考

- SRT RFC draft：Packet Filter 握手扩展、FEC 控制/恢复包形态。
- Haivision SRT / libsrt：`SRTO_PACKETFILTER`、FEC 配置字符串（如 `fec,cols:10,rows:5`）。
- ZLM：`HSExt` 含 `HS_EXT_MSG_PACKET_FILTER` flag 位，**无实现**。
- `shiguredo_srt`（2026.1.0-canary.1）：**无 FEC / Packet Filter 实现**（源码与 README 仅 Live 主路径 ARQ）。

### 为何单独成 Phase

FEC 依赖协议库深度能力，风险与工作量高于 streamid/module 对齐；主路径 ARQ 已由 Phase 02 验收。FEC 作为可选增强，不阻塞 Phase 01–03 交付。

---

## 4.1 方案决策（实施前必须先做）

按优先级评估，结果写入实现 PR 描述：

| 方案 | 描述 | 优点 | 风险 |
|------|------|------|------|
| A. 上游扩展 | 向 `shiguredo_srt` 增加 filter API / 贡献补丁 | 边界正确、Sans-I/O 完整 | 周期不可控 |
| B. Vendor patch | 本仓库钉扎 patch / fork 版本 | 可落地 | 维护成本 |
| C. Driver 旁路 filter | 在 driver 收发路径外包一层 FEC | 不改库 | 易与库缓冲/序号语义冲突，**仅当可证明序号空间一致** |

**推荐顺序**: A 调研 → 短期 B 落地最小 FEC → 长期回馈上游。C 仅作最后手段。

决策输出物：

- 是否可设置 `PACKET_FILTER` HS flag
- 数据通道是否允许插入 FEC 包而不破坏 `SrtConnection` 状态
- 最小可行算法（见 4.3）

---

## 4.2 配置模型

```text
fec:
  enabled: false
  required: false          # true: 协商失败则断开
  algorithm: "xor"         # 首版建议 xor 行/列；可选 rs
  cols: 10
  rows: 5
  layout: "staircase"      # 或 row|column，按所选算法文档
  max_pending_groups: 64   # 上界
```

校验：

- `cols/rows >= 1` 且乘积有上界（防内存炸）。
- `enabled=false` 时不宣告 filter。
- 与 encryption 组合：先明确加密与 FEC 的先后（通常 FEC 在加密载荷之上或按 libsrt 约定；**实现前对照 RFC/libsrt 定稿**）。

Module 配置变更：`fec` 主开关 → `ModuleRestartRequired`。

---

## 4.3 最小可行 FEC（MVP）

目标：在可控实验室环境验证「丢 1 个数据包可由 FEC 恢复」。

建议 MVP：

1. **握手**: 宣告/解析 Packet Filter 扩展；记录 `FecNegotiateResult::{None, Negotiated{..}, Rejected}`。
2. **发送**: 按 `cols` 分组，生成行/列 XOR 校验包并发送。
3. **接收**: 缓存组内包；缺 1 包时用校验恢复；超窗丢弃并计 `unrecovered`。
4. **交付**: 恢复后的 payload 与正常包同一路径进入 module（TS demux）。

非 MVP（可 follow-up）：

- 完整 Reed-Solomon
- 自适应 FEC 码率
- 与 LiveCC 的联合调速

---

## 4.4 分层边界

### `cheetah-srt-core`

- `FecConfig`、`FecLayout`、纯函数：`validate_layout`、`xor_recover`（若算法放 core）。
- **禁止** socket / 时间 / 任务。

### `cheetah-srt-driver-tokio`

- 将 FEC 选项传入连接。
- 在包路径挂载 filter（取决于 4.1 方案）。
- stats：`fec_negotiated`、`fec_recovered`、`fec_unrecovered`、`fec_groups_active`。

### `cheetah-srt-module`

- 配置 schema、metrics 导出、日志。
- 不实现 FEC 算法本体。

---

## 4.5 降级与互操作策略

| 对端 | `fec.enabled` | `fec.required` | 结果 |
|------|---------------|----------------|------|
| 支持 | true | false | 协商 FEC |
| 不支持 | true | false | ARQ-only，连接成功 |
| 不支持 | true | true | 拒绝连接 |
| 任意 | false | * | 不宣告 FEC |

互操作对象：

- 带 FEC 的 `libsrt` / `srt-live-transmit`
- 支持 SRT FEC 的 FFmpeg 构建（需文档标注 configure 选项）
- **不对** ZLM 要求 FEC（ZLM 无此能力）

---

## 4.6 指标与诊断

```text
srt_fec_negotiated                    # 0/1 或 counter
srt_fec_packets_recovered_total
srt_fec_packets_unrecovered_total
srt_fec_groups_dropped_total          # 超时/上界
srt_fec_negotiate_fail_total{reason}
```

日志：协商结果、恢复成功/失败、组超时。

---

## 4.7 测试清单

| 用例 | 说明 |
|------|------|
| layout 校验 | cols/rows 边界 |
| XOR 恢复单元 | 人为删 1 包可恢复 |
| 多丢不可恢复 | unrecovered++ |
| 协商降级 | 对端无 filter → ARQ-only |
| required 失败 | 断开 |
| 与加密组合 | 若支持，passphrase 路径不崩 |
| 弱网 + FEC | loss 5% 下 recovered>0 且 TS 可读（实验） |
| fuzz | 畸形 FEC 配置 / 脏 FEC 包不 panic |

---

## 4.8 验收命令

```bash
cargo fmt
cargo clippy -p cheetah-srt-core
cargo clippy -p cheetah-srt-driver-tokio
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-core
cargo test -p cheetah-srt-driver-tokio
cargo test -p cheetah-srt-module

# 外部（示例，取决于 libsrt FEC 构建）
# srt-live-transmit 带 packetfilter 参数 ...
```

---

## 关键文件（预期）

| 动作 | 路径 |
|------|------|
| 增/改 | `crates/protocols/srt/core/src/fec.rs`（建议） |
| 改 | `core/src/{lib,config}.rs` |
| 改 | `driver-tokio/src/{config,driver}.rs` + 可能的 `fec_filter.rs` |
| 改 | `module/src/{config,metrics,http}.rs` |
| 改 | Cargo 依赖（若 vendor `shiguredo_srt`） |
| 参考 | SRT draft Packet Filter；libsrt FEC 文档 |
| 参考 | `vendor-ref/ZLMediaKit/srt/HSExt.hpp` flag 位 |

---

## 风险

1. 与 `shiguredo_srt` 序号/缓冲强耦合，旁路实现易引入重复交付或乱序。
2. FEC + 加密顺序错误会导致全链路失败。
3. 外部互操作客户端构建碎片化，验收环境需固定版本。
4. 过度 FEC 增加带宽与 CPU，必须有上界与默认关闭。

---

## 本阶段不做

- Group Membership / Rendezvous。
- 自适应码率 FEC。
- 修改 ZLM 参考代码。
- 在无库支撑时强行宣称「完整 Haivision FEC 兼容」——MVP 需在文档标明支持的 layout 与对端矩阵。
