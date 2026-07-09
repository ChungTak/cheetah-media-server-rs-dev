# Phase 02: RTMP Core 与 PBT 抓包回归

- 状态：已完成
- 范围：在 `crates/cheetah-rtmp-core/src/core/tests` 和 `crates/cheetah-rtmp-pbt/tests` 中消费真实抓包 fixture，覆盖标准协议重放和传输扰动属性测试。
- 完成标准：Sans-I/O core 可稳定处理真实 publish byte stream；PBT 覆盖 TCP 分片、粘包、截断、重复、乱序、datagram-like 丢片，且不引入 runtime/socket 依赖。

## 目标文件

```text
crates/cheetah-rtmp-core/src/core/tests/capture.rs
crates/cheetah-rtmp-core/src/core/tests.rs
crates/cheetah-rtmp-pbt/tests/prop_rtmp_capture_transport.rs
crates/cheetah-rtmp-pbt/tests/support/capture_fixture.rs
```

`cheetah-rtmp-core` 是 `no_std` crate，核心测试文件中不能依赖 pcap parser 或外部工具。测试只通过 `include_bytes!` 引入 `.rtmpflow`，并使用本地小 helper 解码 record。

## 具体任务

### 2.1 Core server replay 回归

- [x] 在 `core/tests.rs` 中挂载 `mod capture;`。
- [x] 新增 `capture.rs`，提供 `decode_rtmpflow(bytes: &[u8]) -> Vec<&[u8]>` 测试 helper。
- [x] 对标准 publish fixture 执行三种输入视图：
  - 原始 TCP payload record 边界。
  - 合并为单个大 buffer。
  - 每个 record 拆成 1 字节 chunks。
- [x] 重放到 `RtmpCore::new()`，收集 `CoreOutput::Event`。
- [x] 当遇到 `PublishRequested` 后立即注入 `RtmpCoreCommand::AcceptPublish { stream_id }`，释放 pending media。
- [x] 标准样例断言：
  - 至少一个 `Connected`。
  - 至少一个 `PublishRequested`。
  - 至少 `expect_media_min` 个 `MediaData`。
  - 同一 media type 的 timestamp 单调非递减。

### 2.2 Core 分片/粘包/截断鲁棒性回归

- [x] 对所有 standard/probe fixture 新增 robustness test，不要求事件成功。
- [x] 输入视图覆盖：
  - `coalesced_pairs`：每两个 record 合并，模拟 TCP 粘包。
  - `prefix_truncated`：只喂入前 1/2、前 3/4。
  - `suffix_truncated_record`：最后一个 record 截断到一半。
  - `duplicated_record`：重复第一个 post-handshake record。
  - `reordered_adjacent`：交换两个相邻 post-handshake record。
  - `dropped_every_nth`：丢弃每 5 个 record 中的一个。
- [x] 每个视图断言 `handle_input` 不 panic，循环有明确 record 上限，返回 `Err` 可接受。
- [x] 对 `RtmpCoreError` 不做文本匹配，避免把鲁棒性测试绑定到错误消息。

### 2.3 PBT 传输扰动属性测试

- [x] 新增 `prop_rtmp_capture_transport.rs`，用 proptest 在 fixture 集合中随机选 case、输入视图、chunk size、截断点、重复/丢弃步长。
- [x] 对 standard fixture 的未扰动视图保留强断言；对扰动视图只断言 bounded robustness。
- [x] 属性测试必须限制 case 数和输入大小，默认 `ProptestConfig::with_cases(64)`，避免 CI 过慢。
- [x] 所有 helper 放在 `tests/support/capture_fixture.rs`，避免在每个测试文件复制 `.rtmpflow` 解析。

## 最新进展

- 2026-05-03：完成 2.3。新增 `prop_rtmp_capture_transport.rs`，使用 `ProptestConfig::with_cases(64)` 在 8 个 committed fixture 中随机选择 case、transport view、chunk size、截断点、重复次数和丢弃步长；新增 deterministic standard pristine 测试保证 4 个标准样例始终执行 Connected/PublishRequested/MediaData/timestamp 强断言。共享 helper 现在集中在 `tests/support/capture_fixture.rs`，负责加载 manifest fixture 和构造 pristine、chunked、粘包、截断、重复、乱序、丢包视图。
- 2026-05-03：完成 2.2。所有 standard/probe fixture 均新增 core robustness replay，覆盖 TCP 粘包、prefix 截断、suffix 半 record 截断、post-handshake record 重复、相邻乱序和每 5 个 post-handshake record 丢弃。测试只要求 bounded processing：输入视图有明确上限，`handle_input` panic 会失败，返回 `Err` 可接受且不匹配错误文本。
- 2026-05-03：完成 2.1。新增 core capture replay 测试，直接 `include_bytes!` 读取 4 个 standard `.rtmpflow`，本地 helper 解码 `CRF1` record。每个样例覆盖原始 record、合并单 buffer、逐字节输入三种视图；测试在 `PublishRequested` 后注入 `AcceptPublish`，断言 Connected、PublishRequested、MediaData 和按媒体类型 timestamp 单调非递减，保持 Sans-I/O 且不引入 runtime/socket/pcap parser。
- 2026-05-03：计划已创建，任务未开始。现有 core 测试已覆盖 handshake、command、media 的人工构造样例；本阶段补齐真实抓包 byte stream 和传输边界变化。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-rtmp-core
cargo test -p cheetah-rtmp-core capture
cargo clippy -p cheetah-rtmp-pbt
cargo test -p cheetah-rtmp-pbt --test prop_rtmp_capture_transport
cargo test -p cheetah-rtmp-pbt --test capture_fixture_manifest
```
