# Phase 04 — FEC / Packet Filter（可执行）

- **状态**: 部分完成（core 纯函数 XOR、配置/指标已落地；driver 集成因 `shiguredo_srt` 缺少 packet-filter / FEC API 阻塞，已记录 `reject:fec_required`）  
- **依赖**: Phase 01–03 主路径稳定（可并行调研）  
- **兼容规范**: [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md) §5.1–§5.2、§6  

## 完成标准（DoD）

- [ ] 完成并记录 **方案决策**（A/B/C）  
- [ ] `fec` 配置项 + validate  
- [ ] 至少一种恢复算法单测通过（XOR MVP）  
- [ ] `enabled=true` 时能协商或明确降级；`required=true` 失败断开  
- [ ] metrics：`srt_fec_packets_recovered_total` 等  
- [ ] clippy/test 通过  

---

## 任务 4.0 — 强制调研（先于编码实现集成）

### 目的

当前 workspace 依赖 `shiguredo_srt = "=2026.1.0-canary.1"` **不含 FEC**。必须先回答：

| 问题 | 如何查 |
|------|--------|
| 是否有 FILTER 扩展类型 | `rg -n "Filter|FILTER|PacketFilter|fec" ~/.cargo/registry/src/*/shiguredo_srt-*/src` |
| ConnectionOptions 能否设 flags | 读 `ConnectionOptions` 定义 |
| 数据路径能否插入 FEC 包 | 读 `SrtConnection` send/recv |

### 决策表（实现 PR 描述必须粘贴）

| 方案 | 何时选 | 动作 |
|------|--------|------|
| **A 上游扩展** | 可短期给 shiguredo_srt 加 API | fork/patch 提交 + 版本钉扎 |
| **B vendor patch** | 需改库但上游慢 | `[patch.crates-io]` 或 path 依赖本地 patched crate |
| **C driver 旁路** | 仅当能证明不破坏库序号/TSBPD | 在 SendPacket/收包前后包装；**高风险** |

默认推荐：**B 落地 MVP + 注明回馈上游**。

若结论为「本迭代无法安全集成」，则：

1. 仍完成 **core 纯函数 FEC + 配置 + 单测**  
2. driver 集成标 `TODO` + feature flag `fec` 默认关  
3. 在 index 状态写明阻塞原因  

---

## 任务 4.1 — 配置

### 文件

`module/src/config.rs`、`driver-tokio/src/config.rs`

```rust
pub struct SrtFecModuleConfig {
    pub enabled: bool,     // false
    pub required: bool,    // false
    pub cols: u32,         // 10
    pub rows: u32,         // 5
}

impl SrtFecModuleConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled { return Ok(()); }
        if self.cols == 0 || self.rows == 0 { return Err(...); }
        if self.cols.saturating_mul(self.rows) > 10_000 { return Err("fec matrix too large"); }
        Ok(())
    }
}
```

`SrtModuleConfig::validate` 调用之；变更 fec 主开关 → restart。

---

## 任务 4.2 — Core 纯函数 MVP（无 I/O）

### 文件

新建 `crates/protocols/srt/core/src/fec.rs`

### MVP 算法：按列/行 XOR（规格）

为可测试性，先实现与传输无关的组恢复：

```rust
/// 一组数据包：indices 0..n-1，某下标缺失为 None
pub fn xor_recover_one(packets: &[Option<Bytes>]) -> Option<Bytes>;
```

规则：

- 恰好缺 1 个且存在 XOR 校验包时恢复  
- 缺 0 个：不需要  
- 缺 ≥2：None  

单测：构造 4 个包 + 校验，删 1 个，恢复相等。

布局参数 `cols/rows` 的分组函数：

```rust
pub fn fec_group_id(seq: u32, cols: u32, rows: u32) -> u32;
```

（具体映射在实现时写清注释；保持确定性。）

---

## 任务 4.3 — Driver 集成（依赖 4.0）

### 仅当方案 A/B 可行

1. 握手设置 `PACKET_FILTER` flag / FILTER 扩展（参考规范 §5）。  
2. 发送：媒体包进入 FEC 编码器 → 输出媒体+校验 UDP 包经现有 SendPacket 路径。  
3. 接收：先 FEC 再交给 `SrtConnection` **或** 在库恢复后交付 payload — **以方案文档为准，禁止双份序号**。  
4. stats 字段递增 recovered/unrecovered。  

### 降级表（参考 §6）

| enabled | required | peer 支持 | 结果 |
|---------|----------|-----------|------|
| false | * | * | 不协商 |
| true | false | 否 | 连接成功 ARQ-only；metrics negotiated=0 |
| true | true | 否 | Close `reject:fec_required` |
| true | * | 是 | FEC on |

---

## 任务 4.4 — Metrics

```text
srt_fec_negotiated                 # gauge 0/1 per conn or counter successes
srt_fec_packets_recovered_total
srt_fec_packets_unrecovered_total
srt_fec_negotiate_fail_total
```

导出到 `/srt/metrics`。

---

## 任务 4.5 — 测试

| 测试 | 层 |
|------|-----|
| xor_recover 单元 | core |
| layout validate | core/module |
| required 失败关闭 | driver/module（若集成） |
| enabled 降级 | driver |
| fuzz 随机 cols/rows | fuzz 可选 |

```bash
cargo test -p cheetah-srt-core fec
cargo test -p cheetah-srt-driver-tokio
cargo test -p cheetah-srt-module
```

### 外部互操作说明

对端需支持 SRT FEC 的 libsrt/ffmpeg 构建；**不要求** 与无 FEC 的服务器实现对等。命令示例（工具可用时）：

```bash
# 视 libsrt 文档启用 packetfilter / fec 参数
```

---

## 验收命令

```bash
cargo fmt
cargo clippy -p cheetah-srt-core -p cheetah-srt-driver-tokio -p cheetah-srt-module
cargo test -p cheetah-srt-core
cargo test -p cheetah-srt-driver-tokio
cargo test -p cheetah-srt-module
```

## 本阶段不做

- Group Membership  
- 自适应 FEC 码率  
- 宣称完整 Haivision 全矩阵兼容（MVP 文档写清支持的 layout）  
