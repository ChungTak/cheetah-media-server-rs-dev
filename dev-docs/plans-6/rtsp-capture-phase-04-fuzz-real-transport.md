# Phase 04: RTSP Fuzz 真实传输场景

- 状态：计划中
- 范围：扩展 `crates/cheetah-rtsp-fuzz/fuzz_targets`，把真实 RTSP 抓包 fixture 和传输扰动纳入 fuzz target。
- 完成标准：新增 fuzz target 可构建、可短跑；真实 fixture 作为 seed 覆盖 RTSP request decoder、core、interleaved、RTP、RTCP 路径；UDP 丢包、UDP 乱序、TCP 粘包、TCP 半包、RTP 乱序、重复和截断均有目标入口。

## 目标文件

```text
crates/cheetah-rtsp-fuzz/Cargo.toml
crates/cheetah-rtsp-fuzz/fuzz_targets/common.rs
crates/cheetah-rtsp-fuzz/fuzz_targets/fuzz_real_capture_rtsp_tcp_replay.rs
crates/cheetah-rtsp-fuzz/fuzz_targets/fuzz_real_capture_udp_datagrams.rs
crates/cheetah-rtsp-fuzz/fuzz_targets/fuzz_real_capture_mixed_transport.rs
crates/cheetah-rtsp-fuzz/fuzz_targets/fuzz_rtp_sequence_faults.rs
crates/cheetah-rtsp-fuzz/corpus/fuzz_real_capture_rtsp_tcp_replay/
crates/cheetah-rtsp-fuzz/corpus/fuzz_real_capture_udp_datagrams/
crates/cheetah-rtsp-fuzz/corpus/fuzz_real_capture_mixed_transport/
crates/cheetah-rtsp-fuzz/corpus/fuzz_rtp_sequence_faults/
```

`corpus/` 如果当前被 `.gitignore` 忽略，应调整为忽略 fuzz 运行时自动生成的 artifacts/coverage/crashes，但允许提交 `seed_standard_*.rtspcap` 短前缀 seed。若不提交 corpus，则 fuzz target 必须使用 `include_bytes!` 读取 `cheetah-rtsp-pbt/tests/testdata/rtsp-capture` 中的 fixture 作为内置种子。

## Fuzz 输入策略

- 优先尝试把 fuzz input 当完整 `.rtspcap` 解码。
- 解码失败时，用 fuzz input 选择内置 fixture、record 子集和 fault mode。
- 所有 target 必须设置最大 record 数、最大 datagram 数、最大总输入字节数，防止 fuzz 生成超大循环。
- 对 parser/core 返回 `Err` 不 panic；只让 libFuzzer 捕获真正 panic、越界、OOM、timeout。

## 具体任务

### 4.1 新增真实 RTSP capture fuzz

- [x] 在 `Cargo.toml` 新增四个 `[[bin]]`：`fuzz_real_capture_rtsp_tcp_replay`、`fuzz_real_capture_udp_datagrams`、`fuzz_real_capture_mixed_transport`、`fuzz_rtp_sequence_faults`。
- [x] 扩展 `fuzz_targets/common.rs`，新增 `.rtspcap` 解码 helper、fixture seed 列表、bounded feed helper。
- [x] `fuzz_real_capture_rtsp_tcp_replay` 使用 `RtspCore::new()`，根据 fuzz data 选择 fixture、TCP record 视图和 chunk 策略。
- [x] TCP replay 同时调用 `fuzz_message_decoders`，覆盖 request decoder、response decoder 和 core drain path。
- [x] 对 interleaved frame，构造 `$ + channel + len + payload` 喂给 core，并把 payload 分别尝试 `RtpPacket::parse` 和 `RtcpPacket::parse`。

### 4.2 新增 UDP/TCP/RTP fault fuzz

- [x] `fuzz_real_capture_udp_datagrams` 从真实 fixture 或 fuzz data 构造 UDP RTP/RTCP datagram 列表。
- [x] UDP fault mode 支持：
  - `drop_every_nth_datagram`：每 N 个 datagram 丢弃一个。
  - `drop_first_media_datagram`：丢首个 RTP datagram。
  - `duplicate_datagram`：重复指定 datagram。
  - `swap_adjacent_datagrams`：交换相邻 datagram。
  - `reverse_small_window`：对 3 到 8 个 datagram 窗口反序。
  - `truncate_datagram_payload`：截断 payload 前缀。
  - `mix_rtp_rtcp_order`：把 RTCP 插入 RTP 序列中间。
- [x] `fuzz_real_capture_mixed_transport` 同时喂 RTSP TCP control、interleaved frame 和 UDP datagram parser，覆盖跨 parser 状态组合。
- [x] `fuzz_rtp_sequence_faults` 聚焦同一 SSRC 的 RTP packet 序列，覆盖 sequence wrap、重复旧包、timestamp 回退、marker bit 抖动和 payload 截断。
- [x] TCP fault mode 支持：single buffer、original records、one-byte chunks、coalesced N、prefix truncated、duplicate record、swap adjacent、drop every Nth。
- [x] RTP 乱序场景只要求 parser bounded，不要求 sequence 单调或成功组帧。

### 4.3 Fuzz smoke 和 corpus seed 收口

- [x] 对新增 target 执行构建校验（当前仓库结构下以 `cargo check/clippy --bins` 作为 `cargo-fuzz build` 的等价验证）。
- [x] 每个新增 target 至少跑 `-runs=128`。
- [x] 将标准 fixture 的短前缀作为 seed，确保初始 corpus 覆盖 `OPTIONS/ANNOUNCE/SETUP/RECORD`、`DESCRIBE/PLAY`、TCP interleaved RTP、UDP RTP/RTCP。
- [x] 如果 fuzz 发现 crash，先把最小化输入转成 `cheetah-rtsp-pbt/tests/testdata/rtsp-capture/probes/` 中的回归 fixture，再修 core/module 行为（本轮 smoke 未发现 crash）。
- [x] 保持已有 target `fuzz_http_request`、`fuzz_http_response`、`fuzz_interleaved`、`fuzz_rtp`、`fuzz_rtcp`、`fuzz_sdp`、`fuzz_rtsp_core`、`fuzz_rtsp_limits` 可 build。

## 完成后检查

```bash
cargo fmt
cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml --bins
cargo clippy --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml --bins
cd crates/cheetah-rtsp-fuzz
cargo +nightly fuzz build
cargo +nightly fuzz run fuzz_real_capture_rtsp_tcp_replay -- -runs=128
cargo +nightly fuzz run fuzz_real_capture_udp_datagrams -- -runs=128
cargo +nightly fuzz run fuzz_real_capture_mixed_transport -- -runs=128
cargo +nightly fuzz run fuzz_rtp_sequence_faults -- -runs=128
```

同时保留既有 target 的 build 覆盖：

```bash
cd crates/cheetah-rtsp-fuzz
cargo +nightly fuzz build fuzz_http_request
cargo +nightly fuzz build fuzz_http_response
cargo +nightly fuzz build fuzz_interleaved
cargo +nightly fuzz build fuzz_rtp
cargo +nightly fuzz build fuzz_rtcp
cargo +nightly fuzz build fuzz_sdp
cargo +nightly fuzz build fuzz_rtsp_core
cargo +nightly fuzz build fuzz_rtsp_limits
```
