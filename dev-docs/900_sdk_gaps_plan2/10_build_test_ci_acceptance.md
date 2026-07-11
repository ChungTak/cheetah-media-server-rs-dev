# 10 · 构建、测试、CI 与验收命令

> **Agent 用途**：把 connector-gaps §4 与 residual DoD 落成可复制命令。  
> **前置**：`cheetah-connector` 已在 workspace（plan1 完成）。

---

## 1. 本地前置

```bash
# 仓库根
test -d crates/sdk/cheetah-connector || {
  echo "MISSING cheetah-connector: complete 900_sdk_gaps_plan first"; exit 1;
}

export RUST_LOG=info
# 端口类测可选
export RUST_TEST_THREADS=1
```

**不需要**：外部媒体服务器、浏览器、硬件、native codec SDK。  
**允许**：`127.0.0.1` loopback TCP/UDP。

---

## 2. 包与 feature 速查

| 目的 | 命令 |
| --- | --- |
| 全量 connector | `cargo test -p cheetah-connector --features full --locked` |
| RTSP | `cargo test -p cheetah-connector --features rtsp,full --locked` |
| WebRTC | `cargo test -p cheetah-connector --features webrtc,full --locked` |
| clippy | `cargo clippy -p cheetah-connector --features full -- -D warnings` |
| fmt | `cargo fmt` |
| example | `cargo run -p cheetah-connector --example external_connector_loopback --features full` |
| codec（R7） | `cargo test -p cheetah-codec --locked`（包名以 Cargo.toml 为准） |

核对 package 名：

```bash
rg -n '^name = ' crates/sdk/cheetah-connector/Cargo.toml \
  crates/foundation/cheetah-codec/Cargo.toml
```

---

## 3. connector-gaps §4 六条 ↔ 命令

### 3.1 supports 与 open 一致

```bash
cargo test -p cheetah-connector --features full --locked capability
# T-CAP-*
```

### 3.2 RTSP pull streaming

```bash
cargo test -p cheetah-connector --features full --locked rtsp
# T-RTSP-* ：recv/cancel/close/queue
```

### 3.3 WebRTC push（signaling ≠ media）

```bash
cargo test -p cheetah-connector --features full --locked webrtc
# T-WR-* ；fixture 测与 open_push 测分文件/名
```

### 3.4 socket-free 或标注

```bash
cargo test -p cheetah-connector --features full --locked loopback
# T-LB-01 ProtocolFraming；T-LB-02 EngineOnly
```

### 3.5 metadata 契约

```bash
cargo test -p cheetah-connector --features full --locked metadata
# MUST + NOT_PRESERVED
```

### 3.6 wait_ready

```bash
cargo test -p cheetah-connector --features full --locked wait_ready
# 或 ready / T-RDY-*
```

### 3.7 error map（R8）

```bash
cargo test -p cheetah-connector --features full --locked error
```

### 3.8 options（R4）

```bash
cargo test -p cheetah-connector --features full --locked options
```

---

## 4. Example 增量规范

路径（既有）：

```text
crates/sdk/cheetah-connector/examples/external_connector_loopback.rs
```

**增量行为**：

1. 打印 `supports` 矩阵（与 R3 一致）。  
2. 演示 RTMP→HTTP-FLV loopback 并打印 `layer`。  
3. 若 feature 启用：尝试 RTSP pull / WebRTC push 最小路径或打印 “wired”。  
4. 演示 `wait_ready`（非 sleep）。  
5. 演示一次 typed 错误（错误方向或坏 URL）。  
6. 退出码 0。

```bash
cargo run -p cheetah-connector --example external_connector_loopback --features full --locked
```

---

## 5. 建议测试文件

```text
tests/capability_matrix.rs       # 修正 R3
tests/rtsp_pull.rs
tests/webrtc_push.rs
tests/webrtc_fixture_not_push.rs # 防混淆
tests/options_passthrough.rs
tests/wait_ready.rs
tests/loopback_layers.rs
tests/metadata_conformance.rs    # 扩展 R7
tests/error_conformance.rs       # 扩展 R8
```

---

## 6. CI 建议

Job 名：`connector-residual-900-2` 或并入既有 connector job。

```yaml
# 示意
steps:
  - cargo fmt --check
  - cargo clippy -p cheetah-connector --features full -- -D warnings
  - cargo test -p cheetah-connector --features full --locked
  - cargo run -p cheetah-connector --example external_connector_loopback --features full --locked
```

可选分离：

- job-fast：EngineOnly + unit  
- job-framing：ProtocolFraming localhost  
- job-webrtc：webrtc feature  

---

## 7. 提交前检查（AGENTS.md §12）

```bash
cargo fmt
cargo clippy -p cheetah-connector --features full
cargo test -p cheetah-connector --features full
```

若改 `cheetah-codec`：

```bash
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
```

---

## 8. 总门禁（R1–R8 完成后）

```bash
cargo fmt --check
cargo test -p cheetah-connector --features full --locked
cargo run -p cheetah-connector --example external_connector_loopback --features full --locked
```

### checklist

- [ ] R3 supports 诚实  
- [ ] R1 RTSP pull  
- [ ] R2 WebRTC push（非纯 SDP）  
- [ ] R4 options  
- [ ] R5 wait_ready  
- [ ] R6 layer 诚实 / socket-free 可选  
- [ ] R7 metadata 契约  
- [ ] R8 error map  
- [ ] 勿回退 HTTP-FLV/RTMP/loopback  
- [ ] example 退出 0  
