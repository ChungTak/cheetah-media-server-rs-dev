# Phase 04: RTMP Fuzz 真实传输场景

- 状态：已完成
- 范围：扩展 `crates/cheetah-rtmp-fuzz/fuzz_targets`，把真实抓包 fixture 和传输扰动纳入 fuzz target。
- 完成标准：新增 fuzz target 可构建、可短跑；真实 fixture 作为 seed 覆盖 server/client/core/chunk 路径；TCP 粘包、半包、截断、重复、乱序、datagram-like 丢片均有目标入口。

## 目标文件

```text
crates/cheetah-rtmp-fuzz/Cargo.toml
crates/cheetah-rtmp-fuzz/fuzz_targets/common.rs
crates/cheetah-rtmp-fuzz/fuzz_targets/fuzz_real_capture_server_replay.rs
crates/cheetah-rtmp-fuzz/fuzz_targets/fuzz_real_capture_client_post_handshake.rs
crates/cheetah-rtmp-fuzz/fuzz_targets/fuzz_transport_faults.rs
crates/cheetah-rtmp-fuzz/corpus/fuzz_real_capture_server_replay/
crates/cheetah-rtmp-fuzz/corpus/fuzz_real_capture_client_post_handshake/
crates/cheetah-rtmp-fuzz/corpus/fuzz_transport_faults/
```

`corpus/` 当前被 `crates/cheetah-rtmp-fuzz/.gitignore` 忽略。若需要提交 seed，应调整 `.gitignore` 为忽略 artifacts/coverage/target 但允许特定 seed 目录；若不提交 corpus，则 fuzz target 使用 `include_bytes!` 读取 `cheetah-rtmp-pbt/tests/testdata/rtmp-capture` 中的 fixture 作为内置种子。

## 具体任务

### 4.1 新增真实抓包 server/client fuzz

- [x] 在 `Cargo.toml` 新增三个 `[[bin]]`：`fuzz_real_capture_server_replay`、`fuzz_real_capture_client_post_handshake`、`fuzz_transport_faults`。
- [x] 扩展 `fuzz_targets/common.rs`，新增 `.rtmpflow` 解码 helper、fixture seed 列表和 bounded feed helper。
- [x] `fuzz_real_capture_server_replay` 使用 `RtmpCore::new()`，根据 fuzz data 选择 fixture、输入视图和 chunk 策略。
- [x] server replay 遇到 `PublishRequested` 时可注入 `AcceptPublish`，以覆盖 pending media 释放路径。
- [x] `fuzz_real_capture_client_post_handshake` 使用 `RtmpCore::new_client()`，只喂 server S2C post-handshake chunk/message fixture；client core 当前不接受 raw handshake bytes，不能把完整 S0/S1/S2 当 client raw input。

### 4.2 新增 transport faults fuzz

- [x] `fuzz_transport_faults` 从真实 fixture 或 fuzz data 构造 record 列表。
- [x] 支持以下模式：
  - `single_buffer`：所有 record 合并。
  - `original_records`：按真实 TCP payload 边界。
  - `one_byte_chunks`：逐字节喂入。
  - `coalesced_n`：每 N 个 record 合并，模拟 TCP 粘包。
  - `truncated_prefix`：只喂前缀。
  - `duplicate_record`：重复 record。
  - `swap_adjacent`：交换相邻 record。
  - `drop_every_nth`：datagram-like 丢片扰动。
- [x] 每轮 feed 设置最大输入字节数和最大 record 数，防止 fuzz 生成超大循环。
- [x] 对 `Err` 不 panic；只让 libFuzzer 捕获真正 panic、越界、OOM、timeout。

### 4.3 Fuzz smoke 和 corpus seed 收口

- [x] 对新增 target 执行 `cargo +nightly fuzz build`。
- [x] 每个新增 target 至少跑 `-runs=128`。
- [x] 将标准 fixture 的短前缀作为 seed，确保初始 corpus 覆盖 handshake/connect/publish/media。
- [x] 如果 fuzz 发现 crash，先把最小化输入转成 `cheetah-rtmp-pbt/tests/testdata/rtmp-capture/probes/` 中的回归 fixture，再修 core/module 行为。

## 最新进展

- 2026-05-05：完成 Phase 04 / 4.3 corpus seed 收口。新增 `crates/cheetah-rtmp-fuzz/corpus/` 下三组可提交 seed：`fuzz_real_capture_server_replay`、`fuzz_real_capture_client_post_handshake`、`fuzz_transport_faults` 各包含 `seed_standard_{h264,h265,audio}_prefix.rtmpflow`（每个 seed 取标准 fixture 前 40 条 record，单文件约 14 KiB～43 KiB）；`fuzz_targets/common.rs` 新增 `capture_records_from_data_or_seed`，优先把 fuzz 输入当作完整 CRF1 `.rtmpflow` 解码，失败时再回退到内置 fixture selector，使提交的 corpus seed 直接覆盖真实抓包重放路径；`fuzz_transport_faults` 同步改为新输入路径，三 target 行为一致。同时调整根 `.gitignore` 与 `crates/cheetah-rtmp-fuzz/.gitignore`：继续忽略 fuzz 运行时自动生成 corpus，仅放行 `seed_standard_*.rtmpflow`。本轮 fuzz smoke 未发现 crash，无需新增 probes 回归 fixture。验证已执行：`cargo fmt`、`cargo check --manifest-path crates/cheetah-rtmp-fuzz/Cargo.toml --bins`、`cargo clippy --manifest-path crates/cheetah-rtmp-fuzz/Cargo.toml --bins`、三个新增 target `cargo +nightly fuzz build --fuzz-dir crates/cheetah-rtmp-fuzz ...`、三个新增 target 各 `-runs=128`、`cargo test --workspace`。
- 2026-05-03：完成 Phase 04 / 4.1 和 4.2。`Cargo.toml` 新增 `fuzz_real_capture_server_replay`、`fuzz_real_capture_client_post_handshake`、`fuzz_transport_faults` 三个 target；`common.rs` 内置 8 个真实 `.rtmpflow` fixture seed，新增 CRF1 解码、bounded record/byte feed、server replay 自动 `AcceptPublish`、post-handshake server write 派生和 transport view 构造。server target 使用 `RtmpCore::new()` 重放真实 C2S fixture，client target 使用 `RtmpCore::new_client()` 且只喂由真实 replay 产生的 post-handshake S2C chunk，transport target 覆盖 single buffer、原始 record、逐字节、coalesced N、prefix 截断、重复 record、相邻乱序和 datagram-like 每 N 片丢弃；所有 `Err` 作为可接受输入终止，不 panic。已完成 4.3 的 build/smoke 子项，但 corpus seed 收口仍未完成。验证已执行：`cargo fmt`、`cargo check --manifest-path crates/cheetah-rtmp-fuzz/Cargo.toml --bins`、`cargo clippy --manifest-path crates/cheetah-rtmp-fuzz/Cargo.toml --bins`、`cargo +nightly fuzz build --fuzz-dir crates/cheetah-rtmp-fuzz fuzz_real_capture_server_replay`、`cargo +nightly fuzz build --fuzz-dir crates/cheetah-rtmp-fuzz fuzz_real_capture_client_post_handshake`、`cargo +nightly fuzz build --fuzz-dir crates/cheetah-rtmp-fuzz fuzz_transport_faults`、三个新增 target 各 `-runs=128`、`cargo test --workspace`。
- 2026-05-03：计划已创建，任务未开始。现有 fuzz target 主要覆盖 AMF、FLV、handshake、chunk、message、command、user control 和单次 server/client raw bytes；本阶段补齐真实抓包、多次输入、传输异常和内置 seed。

## 完成后检查

```bash
cargo fmt
cd crates/cheetah-rtmp-fuzz
cargo +nightly fuzz build
cargo +nightly fuzz run fuzz_real_capture_server_replay -- -runs=128
cargo +nightly fuzz run fuzz_real_capture_client_post_handshake -- -runs=128
cargo +nightly fuzz run fuzz_transport_faults -- -runs=128
```

同时保留既有 target 的 build 覆盖：

```bash
cd crates/cheetah-rtmp-fuzz
cargo +nightly fuzz build fuzz_amf0
cargo +nightly fuzz build fuzz_amf3
cargo +nightly fuzz build fuzz_flv
cargo +nightly fuzz build fuzz_handshake
cargo +nightly fuzz build fuzz_rtmp_chunk
cargo +nightly fuzz build fuzz_rtmp_command
cargo +nightly fuzz build fuzz_rtmp_message
cargo +nightly fuzz build fuzz_user_control
```
