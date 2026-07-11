# 09 · 构建、测试、CI 与验收命令

> **Agent 用途**：把 `cheetah-media-server-rs-gaps.md` §4 落成可复制命令与 example 规范。  
> **约束**：不依赖外部媒体服务器、浏览器、硬件、native codec SDK；允许本机 loopback TCP/UDP。

---

## 1. 本地开发前置

```bash
# 仓库根
cargo fetch --locked   # 若仓库使用 lock；否则 cargo fetch

# 常用环境
export RUST_LOG=info
# 可选：限制并行 flaky 端口测
export RUST_TEST_THREADS=1
```

**不需要**：FFmpeg 系统库、浏览器、Pion、ZLM、Janus、GPU、厂商 SDK。

**允许**：本机 `127.0.0.1` 回环端口、纯 Rust 依赖、已在 `Cargo.lock` 的 crates.io 包。

---

## 2. 包与 feature 速查

| 目的 | 命令骨架 |
| --- | --- |
| connector 单测 | `cargo test -p cheetah-connector --features full --locked` |
| connector 最小检查 | `cargo check -p cheetah-connector --locked` |
| HTTP-FLV module | `cargo test -p cheetah-http-flv-module --locked` |
| RTMP module | `cargo test -p cheetah-rtmp-module --locked` |
| WebRTC module | `cargo test -p cheetah-webrtc-module --locked` |
| engine | `cargo test -p cheetah-engine --locked` |
| sdk | `cargo test -p cheetah-sdk --locked` |
| example | `cargo run -p cheetah-connector --example external_connector_loopback --features full --locked` |
| clippy | `cargo clippy -p cheetah-connector --features full -- -D warnings`（按仓库既有 clippy 策略调整） |
| fmt | `cargo fmt` |

> package name 以各 crate `Cargo.toml` 的 `name =` 为准（module 通常是 `cheetah-http-flv-module` 等）。

核对：

```bash
rg -n '^name = ' crates/protocols/http-flv/module/Cargo.toml \
  crates/protocols/rtmp/module/Cargo.toml \
  crates/protocols/webrtc/module/Cargo.toml \
  crates/sdk/cheetah-sdk/Cargo.toml
```

---

## 3. gaps.md §4 八条验收 ↔ 命令

### 3.1 明确 features；无 native artifact 强制

```bash
cargo check -p cheetah-connector --features full --locked
cargo tree -p cheetah-connector --features full --locked 2>&1 | tee /tmp/cheetah-connector-tree.txt
# 人工/脚本确认无意外 system library 链接意图（以项目现有依赖为准）
```

**通过标准**：仅用 workspace 声明依赖可构建。

### 3.2 安装 connector；capability matrix

```bash
cargo test -p cheetah-connector --features full --locked capability_matrix
# 覆盖 T-C-01…05（见 03）
```

### 3.3 in-memory / in-process loopback

```bash
cargo test -p cheetah-connector --features full --locked loopback
# T-L1-* 必须 BYPASS_WIRE=false
cargo test -p cheetah-connector --features full --locked engine_smoke
# L0 单独；命名含 bypass
```

### 3.4 HTTP-FLV streaming

```bash
cargo test -p cheetah-http-flv-module --locked streaming
cargo test -p cheetah-connector --features http-flv,loopback --locked http_flv
# recv / cancel / close / queue / reconnect
```

### 3.5 WebRTC signaling vs media

```bash
cargo test -p cheetah-webrtc-module --locked signaling
cargo test -p cheetah-webrtc-module --locked media_fixture
# 禁止仅 SDP 测试命名为 media_roundtrip
cargo test -p cheetah-connector --features webrtc,full --locked webrtc
```

### 3.6 metadata conformance

```bash
cargo test -p cheetah-connector --features full --locked metadata
```

### 3.7 error conformance

```bash
cargo test -p cheetah-connector --features full --locked error
```

### 3.8 engine smoke 标注绕过

```bash
cargo test -p cheetah-connector --features full --locked engine_smoke_bypass_wire
# 或 engine 既有测 + connector 包装测
```

---

## 4. Example 规范

### 4.1 路径

```text
crates/sdk/cheetah-connector/examples/external_connector_loopback.rs
```

### 4.2 行为（对齐 gaps §4）

1. 仅启用明确 features 构建。  
2. `ConnectorBuilder::new(...).with_default_modules().build().await`。  
3. 打印/断言 capability matrix（RTSP/HTTP-FLV pull、RTMP/WebRTC push）。  
4. 调用 `open_in_memory_loopback`（L1）推送若干帧并 `recv`。  
5. 若 WebRTC media 仅 fixture：打印 `layer=…`。  
6. 演示至少一次 typed 错误（如错误方向）。  
7. 优雅 `shutdown`；退出码 0。  

### 4.3 运行

```bash
cargo run -p cheetah-connector --example external_connector_loopback --features full --locked
```

---

## 5. 建议测试目录布局

```text
crates/sdk/cheetah-connector/
  tests/
    capability_matrix.rs
    engine_smoke_bypass_wire.rs
    loopback_rtmp_http_flv.rs
    http_flv_streaming.rs
    error_conformance.rs
    metadata_conformance.rs
    webrtc_layers.rs
  examples/
    external_connector_loopback.rs
```

---

## 6. CI 建议（阶段 7 落地）

### 6.1 Job 名

`connector-external-sdk-ci` 或 `sdk-gaps-900`

### 6.2 步骤草案

```yaml
# 示意，非最终 yaml
steps:
  - cargo fmt --check
  - cargo clippy -p cheetah-connector --features full -- -D warnings
  - cargo test -p cheetah-sdk --locked
  - cargo test -p cheetah-http-flv-module --locked
  - cargo test -p cheetah-connector --features full --locked
  - cargo run -p cheetah-connector --example external_connector_loopback --features full --locked
  # optional:
  # - cargo test -p cheetah-webrtc-module --locked media_fixture
```

### 6.3 flaky 防护

- 端口：绑定 `127.0.0.1:0` 再读实际端口。  
- 时间：避免短 sleep 竞态；用 `wait_ready` / 条件变量 / 超时 `recv`。  
- WebRTC W3 UDP：默认 `#[ignore]` 或独立 job。

---

## 7. 提交前最低检查（对齐 AGENTS.md §12）

改动 connector：

```bash
cargo fmt
cargo clippy -p cheetah-connector --features full
cargo test -p cheetah-connector --features full
```

改动 http-flv module：

```bash
cargo fmt
cargo clippy -p cheetah-http-flv-module
cargo test -p cheetah-http-flv-module
```

改动 webrtc：

```bash
cargo fmt
cargo clippy -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-module
```

若动到 sdk/engine/codec：追加对应 crate 测试。

---

## 8. 总门禁（全部阶段完成后）

```bash
cargo fmt --check
cargo test -p cheetah-sdk --locked
cargo test -p cheetah-http-flv-module --locked
cargo test -p cheetah-connector --features full --locked
cargo run -p cheetah-connector --example external_connector_loopback --features full --locked
# WebRTC media fixture
cargo test -p cheetah-webrtc-module --locked media_fixture
```

### 验收 checklist

- [ ] feature 安装面清晰  
- [ ] capability matrix 测过  
- [ ] L1 protocol loopback 绿  
- [ ] L0 bypass 存在且分离  
- [ ] HTTP-FLV streaming 行为测过  
- [ ] WebRTC W1/W2 分离  
- [ ] metadata 字段断言  
- [ ] typed errors + retryable  
- [ ] example 退出 0  
- [ ] 分层约束未破  

---

## 9. 与既有协议测试的关系

- **保留** 各 `*-module/tests` 与 property-tests。  
- connector 测试是 **外部 integrator 视角** 的薄集成，不替代协议深度测。  
- 可抽取共享 test harness 到 `module/tests/support`，避免复制。  
