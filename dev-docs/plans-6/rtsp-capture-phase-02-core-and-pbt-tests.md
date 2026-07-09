# Phase 02: RTSP Core 与 PBT 抓包回归

- 状态：计划中
- 范围：在 `crates/cheetah-rtsp-core/src/core/tests` 和 `crates/cheetah-rtsp-pbt/tests` 中消费真实抓包 fixture，覆盖标准控制面重放、RTP/RTCP 解析和传输扰动属性测试。
- 完成标准：Sans-I/O core 可稳定处理真实 RTSP request byte stream 和 TCP interleaved frame；PBT 覆盖 TCP 分片/粘包/截断/重复/乱序、UDP 丢包/乱序、RTP 乱序/重复/截断，且不引入 runtime/socket 依赖。

## 目标文件

```text
crates/cheetah-rtsp-core/src/core/tests/capture.rs
crates/cheetah-rtsp-core/src/core/mod.rs
crates/cheetah-rtsp-pbt/tests/prop_rtsp_capture_transport.rs
crates/cheetah-rtsp-pbt/tests/support/rtsp_capture_fixture.rs
```

当前 `cheetah-rtsp-core/src/core/mod.rs` 已有 inline `#[cfg(test)] mod tests`。实现时不要把所有真实抓包测试继续塞进该文件；应新增 `src/core/tests/capture.rs`，并在 `mod.rs` 中用 `#[cfg(test)] #[path = "tests/capture.rs"] mod capture_tests;` 挂载。

## Core 测试边界

- core 只喂 `CoreInput::Bytes`、`CoreInput::Command`、`CoreInput::PeerClosed`。
- core 测试不能打开 pcap、socket 或 tokio runtime。
- core 测试直接 `include_bytes!` 引入 `cheetah-rtsp-pbt/tests/testdata/rtsp-capture/**/*.rtspcap`。
- RTSP response 不是当前 core server request parser 的输出目标；对 S2C response record 应进入 decoder/PBT 或 fuzz，不要求 server core 产生 request event。
- RTP/RTCP payload 通过 `RtpPacket::parse` 和 `RtcpPacket::parse` 独立断言，不把 UDP datagram 喂给 RTSP byte stream parser。

## 具体任务

### 2.1 Core RTSP 控制面 replay 回归

- [x] 在 `core/mod.rs` 挂载 `capture_tests`。
- [x] 新增 `capture.rs`，提供最小 `.rtspcap` 解码 helper。
- [x] 对 standard C2S RTSP TCP record 执行三种输入视图：
  - 原始 TCP payload record 边界。
  - 合并为单个大 buffer。
  - 每个 record 拆成 1 字节 chunks。
- [x] 重放到 `RtspCore::new()`，收集 `CoreOutput::Event(RtspEvent::Request)`。
- [x] publish 标准样例断言按顺序至少包含 `OPTIONS`、`ANNOUNCE`、`SETUP`、`RECORD`。
- [x] play 标准样例断言按顺序至少包含 `OPTIONS`、`DESCRIBE`、`SETUP`、`PLAY`。
- [x] 对 request 断言 `CSeq` 可解析，`ANNOUNCE` body 非空且可被 `Sdp::parse` 解析，`SETUP` Transport 可被 `RtspTransport::parse_multiple` 解析。

### 2.2 Core RTP/RTCP/interleaved 鲁棒性回归

- [x] 对 TCP interleaved record 构造 `$ + channel + len + payload`，喂给 core 后断言产生 `RtspEvent::InterleavedFrame`。
- [x] 对 interleaved RTP payload 调用 `RtpPacket::parse`，断言 version=2、payload 非空、sequence/timestamp 可读。
- [x] 对 interleaved RTCP payload 调用 `RtcpPacket::parse`，允许 compound 包，断言不 panic。
- [x] 对 UDP RTP/RTCP datagram 直接调用 `RtpPacket::parse` / `RtcpPacket::parse`。
- [x] 对所有 standard/probe fixture 新增 robustness test，不要求事件成功，只要求 bounded processing。
- [x] 输入视图覆盖：
  - `coalesced_pairs`：每两个 TCP record 合并，模拟 TCP 粘包。
  - `prefix_truncated`：只喂入前 1/2、前 3/4。
  - `suffix_truncated_record`：最后一个 record 截断到一半。
  - `duplicated_record`：重复第一个 post-control record。
  - `reordered_adjacent`：交换两个相邻 post-control record。
  - `dropped_every_nth`：丢弃每 5 个 record 中的一个。
- [x] 每个视图断言 `handle_input` 不 panic，循环有明确 record 上限，返回 `Err` 可接受。

### 2.3 PBT 传输扰动属性测试

- [x] 新增 `prop_rtsp_capture_transport.rs`，用 proptest 在 fixture 集合中随机选择 case、输入视图、chunk size、截断点、重复/丢弃步长。
- [x] 对 standard fixture 的未扰动视图保留强断言；对扰动视图只断言 bounded robustness。
- [x] 属性测试默认 `ProptestConfig::with_cases(64)`，避免 CI 过慢。
- [x] TCP 属性覆盖：single buffer、original records、one-byte chunks、coalesced N、prefix truncated、suffix truncated、duplicate、swap adjacent、drop every Nth。
- [x] UDP 属性覆盖：drop datagram、duplicate datagram、swap adjacent datagrams、reverse small window、truncate payload。
- [x] RTP sequence 属性覆盖：同一 SSRC 原始顺序基本单调；乱序/重复视图不要求单调，只要求 parser 和 helper bounded。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-rtsp-core
cargo test -p cheetah-rtsp-core capture
cargo clippy -p cheetah-rtsp-pbt
cargo test -p cheetah-rtsp-pbt --test prop_rtsp_capture_transport
cargo test -p cheetah-rtsp-pbt --test rtsp_capture_fixture_manifest
```
