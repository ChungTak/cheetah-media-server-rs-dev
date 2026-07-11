# Phase 05 — 互操作、Fuzz、运维收口（可执行）

- **状态**: 部分完成（自动化测试壳、互操作记录、指标清单已落地；P1/L1 等需手工环境复跑）  
- **依赖**: Phase 01–03 必选；Phase 04 若延期须在矩阵中标注 FEC 用例跳过  
- **兼容规范**: [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md) §2.5  
- **基线 ops**: [../plans-28-srt/srt-ops-interop.md](../plans-28-srt/srt-ops-interop.md)  

## 完成标准（DoD）

- [ ] 互操作矩阵至少 **P1 + L1** 实测通过并记录  
- [ ] VLC/OBS 步骤写入 ops（可手工一次）  
- [ ] fuzz 短跑 smoke 不崩溃  
- [ ] `/srt/metrics` 字段与本文清单一致  
- [ ] index 状态更新为「可交付/部分交付」  

---

## 任务 5.1 — 互操作矩阵（执行表）

在仓库内维护结果（二选一）：

- 更新 `dev-docs/plans-28-srt/srt-ops-interop.md` 增加「ZLM 兼容」节，**或**  
- 本目录新建 `interop-results.md`（推荐，避免改历史计划语义）

### 推流

| ID | 客户端 | 命令/步骤 | 期望 | 结果 |
|----|--------|-----------|------|------|
| P1 | FFmpeg | 见下 | 流 `live/test` Ready | |
| P2 | OBS | `srt://HOST:9000?streamid=#!::r=live/test,m=publish` | 同上 | |
| P3 | 加密 | passphrase 双边一致 | 成功 | |
| P4 | 错误口令 | 不一致 | 失败 | |

```bash
ffmpeg -re -stream_loop -1 -i test.ts -c:v copy -c:a copy -f mpegts \
  "srt://127.0.0.1:9000?streamid=#!::r=live/test,m=publish"
```

### 拉流

| ID | 客户端 | 命令/步骤 | 期望 | 结果 |
|----|--------|-----------|------|------|
| L1 | ffplay | 见下 | 可播 | |
| L2 | VLC | URL 仅 `srt://HOST:9000`；偏好设置 streamid=`#!::r=live/test` | 可播 | |
| L3 | FFmpeg 录像 | `-i srt://...?streamid=#!::r=live/test -c copy out.ts` | ffprobe OK | |

```bash
ffplay -i "srt://127.0.0.1:9000?streamid=#!::r=live/test"
```

### 语义负例

| ID | 场景 | 期望 |
|----|------|------|
| N1 | 仅 `#!::r=live/test`（无 m） | **不** 建立 publish 租约；走 play |
| N2 | 双 publish 同 key | 第二路 reject |
| N3 | auth 开 + 错 token | reject:auth |
| N4 | `#!::r=live` | reject invalid stream id |
| N5 | FEC required 无对端 FEC | reject（若 Phase 04 完成） |

### 跨协议

| ID | 路径 | 期望 |
|----|------|------|
| X1 | SRT→RTMP/HLS | 可播或 playlist 有分片 |
| X2 | RTMP→SRT play | ffplay OK |

### 弱网

复跑 Phase 02 netem；记录 metrics 快照。

### 结果行格式

```text
| 日期 | ID | 环境 | pass/fail | 备注 | metrics 摘要 |
```

---

## 任务 5.2 — `#[ignore]` 自动化壳

### 文件建议

`crates/protocols/srt/module/tests/zlm_compat_interop.rs`

```rust
// 环境变量 CHEETAH_SRT_INTEROP=1 才运行
// 启动方式：外部已起 cheetah-server 或测试内嵌 runtime
#[tokio::test]
#[ignore]
async fn p1_ffmpeg_publish_l1_ffplay_play() { ... }
```

无 GUI 的 P2/L2 保持手工。

---

## 任务 5.3 — Fuzz smoke

```bash
cd crates/protocols/srt/fuzz
cargo fuzz run fuzz_stream_id -- -runs=1000
cargo fuzz run fuzz_srt_url -- -runs=1000
cargo fuzz run fuzz_driver_packet -- -runs=1000
```

若 `cargo fuzz` 布局问题（历史 plans-28-srt 提过），修复 `fuzz/Cargo.toml` 或文档改为：

```bash
cargo test -p cheetah-srt-core --test parser
# 并保证 fuzz target 至少能编译
cargo build -p cheetah-srt-fuzz  # 若 package 名如此
```

Property：

```bash
cargo test -p cheetah-srt-property-tests
```

---

## 任务 5.4 — 运维指标清单核对

打开 `module/src/http.rs` / `metrics.rs`，逐项勾选：

```text
[ ] srt 连接计数（总/publish/play）
[ ] bytes/packets in/out
[ ] retransmit
[ ] lost / duplicate
[ ] rtt / jitter（或可导出）
[ ] send_queue_full
[ ] key_refresh
[ ] disconnect
[ ] handshake/auth reject
[ ] fec_*（Phase 04）
```

补缺字段；JSON 与 Prometheus 一致。

日志字段抽样：一次 publish 日志含 `app`/`stream`/`mode`（Phase 01 后）。

---

## 任务 5.5 — 文档收口

1. 更新 [index.md](index.md) 顶部状态。  
2. 各 phase 顶部状态改为完成/部分完成。  
3. 确认全文 **无** `vendor-ref` 字符串：  

```bash
rg -n "vendor-ref" dev-docs/plans-28-srt-zlm || echo "OK none"
```

4. 链接仅指向本目录、`dev-docs/plans-28-srt`、`crates/**`、公开 URL。  

---

## 总验收命令

```bash
cargo fmt
cargo clippy -p cheetah-srt-core
cargo clippy -p cheetah-srt-driver-tokio
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-core
cargo test -p cheetah-srt-driver-tokio
cargo test -p cheetah-srt-module
cargo test -p cheetah-srt-property-tests
rg -n "vendor-ref" dev-docs/plans-28-srt-zlm && exit 1 || true
```

手工：

```bash
ffmpeg -re -stream_loop -1 -i test.ts -c copy -f mpegts \
  "srt://127.0.0.1:9000?streamid=#!::r=live/test,m=publish"
ffplay -i "srt://127.0.0.1:9000?streamid=#!::r=live/test"
curl -sS "http://127.0.0.1:<control-port>/srt/metrics" | head
```

---

## 整计划完成定义

1. 参考规范 §2 语义有测试锁定（默认拉流、两段 r、auth 含 m）。  
2. P1/L1 互操作 pass。  
3. ARQ 指标可见；弱网至少手工或 ignore 记录一次。  
4. FEC：完成或 index 明确延期原因。  
5. 智能体仅依赖本目录 + crates 即可复现实现。  
