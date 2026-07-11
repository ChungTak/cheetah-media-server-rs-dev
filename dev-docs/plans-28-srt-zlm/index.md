# SRT 协议完善 — 智能体可执行总计划（ZLM 兼容）

- **状态**: 草案 / 待执行  
- **读者**: 编程智能体（可直接按阶段改代码、写测试、跑验收）  
- **目标**: 在已有 `cheetah-srt-*` 上对齐 [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md) 的业务语义，并补齐版本策略、NACK 观测、FEC。  
- **基线**: [../plans-28-srt/index.md](../plans-28-srt/index.md) 已完成 crate 脚手架、Listener/Caller、TS 推拉主路径；本计划是 **兼容增强**，禁止重写三段式骨架。  
- **约束**: 严格遵守仓库根目录 `AGENTS.md`（core Sans-I/O、module 不暴露 tokio 公共类型、媒体走 `cheetah-codec`、单发布者租约等）。

---

## 0. 智能体工作方式（必读）

### 0.1 允许阅读的路径

| 路径 | 用途 |
|------|------|
| `dev-docs/plans-28-srt-zlm/**` | 本计划全部知识（含兼容行为规范） |
| `dev-docs/plans-28-srt/**` | 已落地基线设计与 ops 记录 |
| `crates/protocols/srt/**` | 唯一实现改动区（主） |
| `crates/foundation/cheetah-codec/**` | 仅当 TS 行为缺陷必须修时 |
| `crates/sdk/cheetah-sdk/**` | 仅当需要扩展公共契约时（尽量避免） |
| `AGENTS.md`、`SystemArchitecture.md` | 分层硬约束 |
| `Cargo.toml`（workspace） | 依赖版本 |

### 0.2 禁止

- **禁止** 引用或假设存在任何 `vendor-ref/**` 路径（执行环境可能没有）。兼容语义只以 [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md) 为准。  
- 禁止在 `cheetah-srt-core` 使用 Tokio / socket / 系统时间 / engine。  
- 禁止 module 公共接口暴露 `tokio::*`。  
- 禁止绕过 `PublishLease` 多发布者写同流。  
- 禁止在 SRT module 复制时间戳/NALU 私有逻辑。  
- 不要例行 `--all-features`。

### 0.3 每个 Phase 的固定交付循环

```text
1. 阅读该 phase 文档全文 + reference-behavior-zlm-compat.md 相关章节
2. 阅读文中列出的本地源文件（当前实现）
3. 按任务编号顺序改代码
4. 补/改测试（文档给出用例表）
5. 运行验收命令
6. 更新该 phase 文档顶部「状态」为完成，并在本 index 任务表勾选
```

### 0.4 全局验收命令

```bash
cargo fmt
cargo clippy -p cheetah-srt-core
cargo clippy -p cheetah-srt-driver-tokio
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-core
cargo test -p cheetah-srt-driver-tokio
cargo test -p cheetah-srt-module
cargo test -p cheetah-srt-property-tests
```

---

## 1. V1 范围

必须完成：

1. Stream ID：`#!::h,r,m,...` 语义（见参考规范）  
2. 默认无 `m` → **拉流**（配置可回退 publish）  
3. `r=app/stream` → `StreamKey`；`h` → vhost meta  
4. 鉴权参数 = 除 `h`/`r` 外全部 key（含 `m`）  
5. Listener 推 TS / 拉 TS only  
6. NACK/ARQ 指标 + latency/buffer 配置 + 弱网验收  
7. peer 版本 `<1.3.0` 可拒绝  
8. FEC（可降级）  
9. OBS/ffmpeg/ffplay/VLC 互操作矩阵  

明确不做：FileCC、Rendezvous、Group、非 TS payload、自研完整 SRT 替代 `shiguredo_srt`（除非 Phase 04 评估结论要求有限 patch）。

---

## 2. 本地代码地图（执行前先打开）

```text
crates/protocols/srt/
  core/
    src/lib.rs
    src/stream_id.rs      # Phase 01 主战场
    src/config.rs         # 模式/加密/会话选项
    src/session.rs        # core 事件类型
    src/error.rs
    tests/parser.rs
  driver-tokio/
    src/config.rs
    src/driver.rs         # Phase 02 主战场
    tests/driver_smoke.rs
  module/
    src/lib.rs
    src/config.rs         # 默认 mode 等
    src/module.rs         # ~1400 行，Phase 03 拆分
    src/metrics.rs
    src/http.rs
  testing/property-tests/
  fuzz/
```

依赖：workspace `shiguredo_srt = "=2026.1.0-canary.1"`（Sans-I/O；内含 ACK/NAK/TSBPD；**无 FEC**）。

---

## 3. 缺口总表（改什么）

| ID | 缺口 | 当前行为 | 目标行为 | Phase |
|----|------|----------|----------|-------|
| G1 | 默认 mode | `default_mode="publish"` | 默认 `"request"`（拉流） | 01 |
| G2 | `r` 结构 | 整串 key，允许单段 | 严格两段 app/stream | 01 |
| G3 | `h` | `host` 字段闲置 | vhost + meta/metrics | 01 |
| G4 | auth_params | 主要 extras token | 除 h/r 外全量含 m | 01 |
| G5 | bare streamid | 接受无 `#!::` | 严格拒绝；开关兼容 | 01 |
| G6 | 版本拒绝 | 无 | min 1.3.0 | 01+02 |
| G7 | NACK 观测/弱网 | 部分 stats | 完整指标+矩阵 | 02 |
| G8 | latency/buf 配置 | latency_ms 有 | 对齐 pkt_buf 等 | 02 |
| G9 | 业务失败关闭 | 部分 | 对齐规范状态机 | 03 |
| G10 | module 体积 | module.rs 过大 | 拆分文件 | 03 |
| G11 | FEC | 无 | 可协商可降级 | 04 |
| G12 | 互操作矩阵 | 部分记录 | 正式矩阵 | 05 |

---

## 4. 文档清单

| 文件 | 用途 |
|------|------|
| [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md) | **兼容行为唯一权威**（自包含） |
| [srt-zlm-architecture.md](srt-zlm-architecture.md) | 目标架构、类型、配置、数据流 |
| [srt-zlm-gap-analysis.md](srt-zlm-gap-analysis.md) | 逐文件差距与风险 |
| [phase-01-streamid-version-auth.md](phase-01-streamid-version-auth.md) | 可执行任务：streamid/auth/version |
| [phase-02-nack-arq-latency-stats.md](phase-02-nack-arq-latency-stats.md) | 可执行任务：ARQ 观测/弱网 |
| [phase-03-module-publish-play-ts-bridge.md](phase-03-module-publish-play-ts-bridge.md) | 可执行任务：推拉业务/拆分 |
| [phase-04-fec-packet-filter.md](phase-04-fec-packet-filter.md) | 可执行任务：FEC |
| [phase-05-interop-ops-fuzz.md](phase-05-interop-ops-fuzz.md) | 可执行任务：互操作/fuzz/ops |

---

## 5. 执行顺序

```text
Phase 01 → Phase 02 → Phase 03 → Phase 04 → Phase 05
```

- 01 不依赖 driver 大改即可单测通过。  
- 02 可与 01 尾部并行，但版本拒绝依赖 01 的版本类型。  
- 03 依赖 01 的 parse/classify API。  
- 04 独立风险最高，可在 03 后；若阻塞可标记延期但 index 必须写明。  
- 05 收口。

---

## 6. 风险与迁移（实现时必须处理）

1. **默认 mode 变更** 会破坏「无 m 却推流」的旧客户端 → 保留 `ingress.default_mode`，默认改 `request`，注释写明兼容设 `publish`。  
2. **严格 `r` 两段** 可能拒绝旧 bare key → `stream_id.allow_bare_key` / `strict_resource`。  
3. **FEC** 依赖库能力，Phase 04 先做调研决策再写代码。  

---

## 7. 外部互操作速查

```bash
# 推
ffmpeg -re -stream_loop -1 -i test.ts -c copy -f mpegts \
  "srt://127.0.0.1:9000?streamid=#!::r=live/test,m=publish"

# 拉
ffplay -i "srt://127.0.0.1:9000?streamid=#!::r=live/test"
```

完整矩阵见 Phase 05；语义见 [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md) §2.5。
