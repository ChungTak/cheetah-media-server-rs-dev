# Phase 03: RTP / RTCP 原语迁移

- 状态：已完成（RTP 任务 1-8、RTCP 任务 1-8、module 私有 RTCP 回收已完成）
- 范围：迁移 RTP/RTCP build/parse、属性测试、fuzz 与现有 module 私有逻辑回收。
- 对应用例：`pbt_rtp.rs`、`pbt_rtcp.rs`、`src/rtp.rs`、`src/rtcp.rs`、`fuzz_rtp.rs`、`fuzz_rtcp.rs`。
- 完成标准：`cheetah-rtsp-core` 提供完整 RTP/RTCP 原语，module 不再维护私有低层拼包/解析实现。

## 来源用例

### `vendor-ref/rtsp-rs/pbt/tests/pbt_rtp.rs`

1. [x] `test_rtp_packet_roundtrip_basic`
2. [x] `test_rtp_packet_roundtrip_with_marker`
3. [x] `test_rtp_packet_roundtrip_with_csrc`
4. [x] `test_rtp_packet_roundtrip_with_extension`
5. [x] `test_rtp_packet_roundtrip_with_padding`
6. [x] `test_rtp_packet_roundtrip_full`
7. [x] `test_rtp_packet_size`
8. [x] `test_rtp_parse_invalid_data`

### `vendor-ref/rtsp-rs/pbt/tests/pbt_rtcp.rs`

1. [x] `test_rtcp_sender_report_roundtrip`
2. [x] `test_rtcp_receiver_report_roundtrip`
3. [x] `test_rtcp_sdes_roundtrip`
4. [x] `test_rtcp_bye_roundtrip`
5. [x] `test_rtcp_app_roundtrip`
6. [x] `test_rtcp_compound_packet_roundtrip`
7. [x] `test_rtcp_report_block_values`
8. [x] `test_rtcp_parse_invalid_data`

### `vendor-ref/rtsp-rs/src/rtp.rs`

1. `test_rtp_parse_and_build`
2. `test_rtp_with_marker`
3. `test_rtp_with_csrc`

### `vendor-ref/rtsp-rs/src/rtcp.rs`

1. `test_rtcp_sr_parse_and_build`
2. `test_rtcp_sdes_parse_and_build`
3. `test_rtcp_bye`

### Fuzz

- `fuzz_rtp.rs`
- `fuzz_rtcp.rs`

## 本地目标设计

### 1. `cheetah-rtsp-core` 新增统一 RTP 模块

- 提供 RTP header、extension、packet 的 parse/build/size 能力。
- 保证支持：
  - marker
  - CSRC 列表
  - extension
  - padding
  - payload type 边界
- parse 错误必须稳定返回显式错误，不允许 panic。

### 2. `cheetah-rtsp-core` 新增统一 RTCP 模块

- 提供以下 packet 族：
  - Sender Report
  - Receiver Report
  - SDES
  - BYE
  - APP
  - compound packet
- 所有 parse/build 行为必须是纯字节级原语，不含业务会话状态。

### 3. 回收 `cheetah-rtsp-module` 私有 RTCP 实现

- 现有 `media.rs` 中以下私有逻辑应逐步替换为 core 原语：
  - `parse_rtcp_packet_type`
  - `parse_rtcp_sender_ssrc`
  - `parse_rtcp_sender_report_lsr`
  - `build_rtcp_sender_report`
  - `build_rtcp_sdes_cname`
  - `build_rtcp_receiver_report`
  - `build_rtcp_bye`
- module 保留“何时发送何种 RTCP”的业务决策，不保留低层编码细节。

### 4. 回收 RTP 低层依赖

- module 中 `build_frame_from_rtp`、`packetize_frame_to_rtp` 应只做业务映射与 codec 适配。
- RTP 包结构体和编解码必须统一来自 core。

## 逐案迁移步骤

- 先迁 `src/rtp.rs` / `src/rtcp.rs` 的内联单测，建立最小可运行骨架。
- 然后迁 `pbt_rtp.rs` / `pbt_rtcp.rs`，把属性空间逐步放大。
- 最后迁 fuzz target，确认成功 parse 后可重新 encode 且不崩溃。
- 每完成一个 RTCP packet 家族后，同步替换 module 中相应私有实现并回归 keepalive 测试。

## 完成判定

- `prop_rtp.rs`、`prop_rtcp.rs` 中对应 vendor 用例全部迁完。
- core 提供统一 RTP/RTCP 原语。
- module 不再维护重复的 RTCP 编码器与解析辅助。
- 相关注释已全部翻译为中文。

## 最新进展

- 2026-04-19：已完成 Phase 03 模块收口任务（回收 `cheetah-rtsp-module` 私有 RTCP 低层实现）：`crates/cheetah-rtsp-module/src/media.rs` 的 `build_rtcp_*` 与发送端 `Sender Report` 解析统一改为复用 `cheetah-rtsp-core::RtcpPacket` 原语，`module/play.rs`、`module/publish.rs`、`module/cleanup.rs` 调整为显式错误处理（记录上下文并安全降级，不再静默截断/拼包）；新增 `parse_sender_report_rejects_invalid_rtcp_payload`、`parse_sender_report_ignores_non_sender_report_packets` 回归测试，覆盖异常输入与非 SR 包场景。并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-module --tests`、`cargo test -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-pbt`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml` 与工作区 `cargo test` 回归；下一步进入 Phase 07 任务 1（注释与文案统一）。
- 2026-04-18：已完成 `test_rtp_packet_roundtrip_basic` 迁移，`cheetah-rtsp-core` 新增统一 `rtp` 原语（`RtpHeader` / `RtpExtension` / `RtpPacket`）及显式 `RtpError` 错误处理，`cheetah-rtsp-pbt/tests/prop_rtp.rs` 替换占位测试并完成基础 roundtrip 属性验证；下一步进入任务 2（`test_rtp_packet_roundtrip_with_marker`）。
- 2026-04-19：已完成 `test_rtp_packet_roundtrip_with_marker` 迁移，`cheetah-rtsp-pbt/tests/prop_rtp.rs` 新增 marker 位 roundtrip 属性测试并覆盖核心字段断言；`cargo fmt`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-module` 全部通过；下一步进入任务 3（`test_rtp_packet_roundtrip_with_csrc`）。
- 2026-04-19：已完成 `test_rtp_packet_roundtrip_with_csrc` 迁移，`cheetah-rtsp-pbt/tests/prop_rtp.rs` 新增 CSRC 列表 roundtrip 属性测试并覆盖核心字段断言，`cheetah-rtsp-core/src/core/rtp.rs` 新增 CSRC 数量越界显式错误回归测试；`cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 全部通过；下一步进入任务 4（`test_rtp_packet_roundtrip_with_extension`）。
- 2026-04-19：已完成 `test_rtp_packet_roundtrip_with_extension` 迁移，`cheetah-rtsp-pbt/tests/prop_rtp.rs` 新增 extension roundtrip 属性测试并覆盖 profile/扩展数据断言，`cheetah-rtsp-core/src/core/rtp.rs` 补充 build 侧 RTP 版本显式校验与扩展负载截断错误回归测试；`cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 全部通过；下一步进入任务 5（`test_rtp_packet_roundtrip_with_padding`）。
- 2026-04-19：已完成 `test_rtp_packet_roundtrip_with_padding` 迁移，`cheetah-rtsp-pbt/tests/prop_rtp.rs` 新增 padding roundtrip 属性测试并断言 `padding_size/payload` 一致，`cheetah-rtsp-core/src/core/rtp.rs` 新增 parse/build 两条 padding 错误处理回归测试（非法 padding 长度、仅置位 padding 标志但未提供 padding 字节）；`cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 全部通过；下一步进入任务 6（`test_rtp_packet_roundtrip_full`）。
- 2026-04-19：已完成 `test_rtp_packet_roundtrip_full` 迁移，`cheetah-rtsp-pbt/tests/prop_rtp.rs` 新增全选项组合（`marker + csrc + extension + padding`）roundtrip 属性测试并补齐核心字段断言；`cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 全部通过；下一步进入任务 7（`test_rtp_packet_size`）。
- 2026-04-19：已完成 `test_rtp_packet_size` 迁移，`cheetah-rtsp-pbt/tests/prop_rtp.rs` 新增 RTP 包长度属性测试并断言 `encoded.len()` 与 `packet.size()` 都等于 `12 + csrc.len() * 4 + payload.len()`；`cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 全部通过；下一步进入任务 8（`test_rtp_parse_invalid_data`）。
- 2026-04-19：已完成 `test_rtp_parse_invalid_data` 迁移，`cheetah-rtsp-pbt/tests/prop_rtp.rs` 新增无效输入解析失败测试，覆盖短包与非法 RTP 版本并显式断言 `RtpError::InsufficientData` / `RtpError::UnsupportedVersion` 错误分支；`cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 全部通过；下一步进入 RTCP 任务 1（`test_rtcp_sender_report_roundtrip`）。
- 2026-04-19：已完成 `test_rtcp_sender_report_roundtrip` 迁移，`cheetah-rtsp-core` 新增 `core/rtcp.rs`（`RtcpPacket`、`RtcpSenderReport`、`RtcpReportBlock`）及显式 `RtcpError` 错误处理，`cheetah-rtsp-pbt/tests/prop_rtcp.rs` 替换占位测试并落地 Sender Report roundtrip 属性测试；`cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 全部通过；下一步进入 RTCP 任务 2（`test_rtcp_receiver_report_roundtrip`）。
- 2026-04-19：已完成 `test_rtcp_receiver_report_roundtrip` 迁移，`cheetah-rtsp-core` 为 `core/rtcp.rs` 增加 `RtcpReceiverReport` 与 `RtcpPacket::ReceiverReport` 的 parse/build 分支，并抽取报告块共享读写逻辑统一错误处理；`cheetah-rtsp-pbt/tests/prop_rtcp.rs` 新增 Receiver Report roundtrip 属性测试并覆盖 `ssrc/reports` 断言，同时在 core 单测增加 RR roundtrip 与截断输入显式错误回归测试；下一步进入 RTCP 任务 3（`test_rtcp_sdes_roundtrip`）。
- 2026-04-19：已完成 `test_rtcp_sdes_roundtrip` 迁移，`cheetah-rtsp-core` 在 `core/rtcp.rs` 增加 `RtcpSdes` / `RtcpSdesChunk` / `RtcpSdesItem` 与 `RtcpPacket::SourceDescription` 的 parse/build 分支，并补充 SDES 截断、PRIV 非法长度、item 长度溢出、chunk 数量溢出的显式错误处理与回归测试；`cheetah-rtsp-pbt/tests/prop_rtcp.rs` 新增 SDES roundtrip 属性测试并覆盖 `chunk/item` 核心断言，同时完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、工作区 `cargo test` 回归；下一步进入 RTCP 任务 4（`test_rtcp_bye_roundtrip`）。
- 2026-04-19：已完成 `test_rtcp_bye_roundtrip` 迁移，`cheetah-rtsp-core` 在 `core/rtcp.rs` 增加 `RtcpBye` 与 `RtcpPacket::Bye` 的 parse/build 分支，并补充 BYE 源列表截断、reason 截断、源数量越界、reason 长度越界等显式错误处理与回归测试；`cheetah-rtsp-pbt/tests/prop_rtcp.rs` 新增 BYE roundtrip 属性测试并校验 `ssrcs/reason` 字段一致性，同时完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、工作区 `cargo test` 回归；下一步进入 RTCP 任务 5（`test_rtcp_app_roundtrip`）。
- 2026-04-19：已完成 `test_rtcp_app_roundtrip` 迁移，`cheetah-rtsp-core` 在 `core/rtcp.rs` 增加 `RtcpApp` / `RTCP_PT_APP` 与 `RtcpPacket::App` 的 parse/build 分支，并补充 APP 截断输入、subtype 越界等显式错误处理与回归测试；`cheetah-rtsp-pbt/tests/prop_rtcp.rs` 新增 APP roundtrip 属性测试并校验 `subtype/ssrc/name/data` 字段一致性，同时完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、工作区 `cargo test` 回归；下一步进入 RTCP 任务 6（`test_rtcp_compound_packet_roundtrip`）。
- 2026-04-19：已完成 `test_rtcp_compound_packet_roundtrip` 迁移，`cheetah-rtsp-pbt/tests/prop_rtcp.rs` 新增 compound 包 roundtrip 属性测试（`SenderReport + SourceDescription` 组合），并复用 `cheetah-rtsp-core` 既有 `RtcpPacket::build/parse` 显式错误处理路径验证多包顺序与字段一致性；同时完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、工作区 `cargo test` 回归；下一步进入 RTCP 任务 7（`test_rtcp_report_block_values`）。
- 2026-04-19：已完成 `test_rtcp_report_block_values` 迁移，`cheetah-rtsp-pbt/tests/prop_rtcp.rs` 新增报告块字段值 roundtrip 属性测试，覆盖 `ssrc/fraction_lost/cumulative_lost/highest_seq/jitter/last_sr/delay_since_sr` 全字段断言，并复用 `cheetah-rtsp-core` 既有 `RtcpPacket::build/parse` 显式错误处理路径；同时完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、工作区 `cargo test` 回归；下一步进入 RTCP 任务 8（`test_rtcp_parse_invalid_data`）。
- 2026-04-19：已完成 `test_rtcp_parse_invalid_data` 迁移，在 `cheetah-rtsp-pbt/tests/prop_rtcp.rs` 新增无效输入解析测试，覆盖“短于 RTCP 公共头返回空结果”与“非法版本返回错误”两条 vendor 语义路径，并复用 `cheetah-rtsp-core` 现有显式错误处理；同时完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、工作区 `cargo test` 回归；下一步进入 Phase 04 任务 1（迁移 `pbt_rtsp.rs` 的 `test_transport_roundtrip`）。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-rtsp-core --tests`
- `cargo test -p cheetah-rtsp-core`
- `cargo test -p cheetah-rtsp-pbt`
- `cargo test -p cheetah-rtsp-module`
- `cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`
