# Phase 04: RTSP 工具原语迁移

- 状态：已完成（`pbt_rtsp.rs` 任务 1-6、`src/rtsp_range.rs` 任务 1-6、`src/rtsp_rtp_info.rs` 任务 1-5、`src/rtsp_connection.rs` 任务 1-8、Fuzz 任务 1 已完成）
- 范围：迁移 Method、Session、Transport、Range、RTP-Info、interleaved 工具与对应测试。
- 对应用例：`pbt_rtsp.rs`、`src/rtsp_range.rs`、`src/rtsp_rtp_info.rs`、`src/rtsp_connection.rs` 中工具语义、`fuzz_interleaved.rs`。
- 完成标准：module 内 transport/range/session 相关私有解析逻辑收敛到 core 工具原语。

## 来源用例

### `vendor-ref/rtsp-rs/pbt/tests/pbt_rtsp.rs`

1. [x] `test_transport_roundtrip`
2. [x] `test_transport_parse_multiple`
3. [x] `test_smpte_type_roundtrip`
4. [x] `test_npt_roundtrip`
5. [x] `test_extension_method_roundtrip`
6. [x] `test_standard_method_case_sensitive`

### `vendor-ref/rtsp-rs/src/rtsp_range.rs`

1. [x] `test_npt_parse`
2. [x] `test_smpte_parse`
3. [x] `test_clock_parse`
4. [x] `test_npt_reverse_range`
5. [x] `test_smpte_type_preserved`
6. [x] `test_display`

### `vendor-ref/rtsp-rs/src/rtsp_rtp_info.rs`

1. [x] `test_parse_single_stream`
2. [x] `test_parse_multiple_streams`
3. [x] `test_parse_without_optional`
4. [x] `test_display`
5. [x] `test_find_by_url`

### `vendor-ref/rtsp-rs/src/rtsp_connection.rs`

优先迁移以下能力对应的测试与实现：

1. [x] `RtspConnectionLimits`
2. [x] `RtspConnectionState`
3. [x] `RtspSession::parse` / `to_header`
4. [x] `RtspTransport::parse` / `parse_multiple` / `to_header`
5. [x] `parse_interleaved_frame`
6. [x] `encode_interleaved_frame`
7. [x] `supported_methods`
8. [x] `public_header_value`

### Fuzz

- [x] `fuzz_interleaved.rs`

## 本地目标设计

### 1. 方法与状态

- 将 `RtspMethod` 从当前最小集合扩展为完整方法集合。
- 保持标准方法大小写敏感，未知方法归入扩展方法或未知方法分支。
- 连接状态若需要对外暴露，只保留协议层状态，不混入业务会话对象。

### 2. Session 与 Transport

- `Session` 头解析下沉到 core，覆盖：
  - `session-id`
  - `timeout`
  - roundtrip header 输出
- `Transport` 头解析下沉到 core，覆盖：
  - 单值与多值 header
  - `interleaved`
  - `client_port` / `server_port` / `port`
  - `ssrc`
  - `mode`
  - `destination` / `source` / `ttl`
  - `append`
- `cheetah-rtsp-module/src/media.rs` 中现有 transport 解析逻辑逐步改为复用 core transport 原语。

### 3. Range 与 RTP-Info

- `Range` 支持 NPT / SMPTE / clock 解析与格式化。
- `RTP-Info` 支持多流条目解析、检索与输出。
- module 中现有 `parse_play_range_header`、`parse_request_range_scale_headers` 需要建立在 core `Range` 原语上，而不是继续手写字符串逻辑。

### 4. Interleaved 工具

- core 暴露独立的 interleaved 解析/编码工具。
- 现有 `RtspCore` 与 driver 统一复用这套逻辑。
- 限制检查仍与 Phase 02 的 `max_interleaved_frame_size` 打通。

## 逐案迁移步骤

- 先迁 `Range` 与 `Method`，因为 module 中已有直接使用点。
- 再迁 `Session` / `Transport`，同步替换 `media.rs` 与 `module.rs` 的私有解析。
- 再迁 `RTP-Info`，为 PLAY 响应构造提供统一原语。
- 最后补 interleaved fuzz 和 utility 回归。

## 完成判定

- `prop_rtsp.rs` 中对应 vendor 用例全部迁完。
- `Range`、`Session`、`Transport`、`RTP-Info` 全部由 core 统一提供。
- module 私有字符串解析逻辑明显收缩，只保留业务映射。
- 相关注释已全部翻译为中文。

## 最新进展

- 2026-04-19：已完成 `test_transport_roundtrip` 迁移，`cheetah-rtsp-core` 新增 `core/transport.rs` 并提供 `RtspTransport` / `RtspTransportError` 与 `parse` / `parse_multiple` / `to_header` 统一原语，`cheetah-rtsp-pbt/tests/prop_rtsp.rs` 替换占位测试并落地 Transport roundtrip 属性测试，同时补充已知参数显式错误处理回归测试；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入任务 2（`test_transport_parse_multiple`）。
- 2026-04-19：已完成 `test_transport_parse_multiple` 迁移，在 `cheetah-rtsp-pbt/tests/prop_rtsp.rs` 新增多值 Transport 头解析属性测试并对齐 vendor 语义，同时在 `cheetah-rtsp-core/src/core/transport.rs` 增加多值解析路径的错误传播回归测试（非法参数应返回 `RtspTransportError::InvalidParameter`）；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入任务 3（`test_smpte_type_roundtrip`）。
- 2026-04-19：已完成 `test_smpte_type_roundtrip` 迁移，`cheetah-rtsp-core` 新增 `core/range.rs` 并提供 `RtspRange` / `SmpteRange` / `SmpteType` / `SmpteTime` 等统一 Range 原语与显式 `RtspRangeError`，在 `cheetah-rtsp-pbt/tests/prop_rtsp.rs` 新增 SMPTE 类型保持 roundtrip 属性测试并对齐 vendor 语义；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入任务 4（`test_npt_roundtrip`）。
- 2026-04-19：已完成 `test_npt_roundtrip` 迁移，在 `cheetah-rtsp-pbt/tests/prop_rtsp.rs` 新增 NPT range parse/display roundtrip 属性测试并对齐 vendor 语义，覆盖 `NptTime::Seconds` 的数值误差容忍断言；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入任务 5（`test_extension_method_roundtrip`）。
- 2026-04-19：已完成 `test_extension_method_roundtrip` 迁移，在 `cheetah-rtsp-pbt/tests/prop_rtsp.rs` 新增扩展方法 roundtrip 属性测试并补充扩展方法生成器，`cheetah-rtsp-core/src/core/method.rs` 将未知方法语义统一为 `Extension(String)` 并补齐 `as_str` / `Display` / `FromStr`，同时在 `cheetah-rtsp-module/src/module/request_dispatch.rs` 对 `Redirect` 和扩展方法统一返回 405 错误处理；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo clippy -p cheetah-rtsp-module --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入任务 6（`test_standard_method_case_sensitive`）。
- 2026-04-19：已完成 `test_standard_method_case_sensitive` 迁移，在 `cheetah-rtsp-pbt/tests/prop_rtsp.rs` 新增标准方法大小写敏感属性测试，显式断言标准大写方法不会退化为 `Extension` 且对应小写输入会保持为 `Extension`；同时在 `cheetah-rtsp-core/src/core/method.rs` 补充大小写敏感回归单测以覆盖核心错误/边界语义；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_range.rs` 任务 1（`test_npt_parse`）。
- 2026-04-19：已完成 `test_smpte_parse` 迁移，在 `cheetah-rtsp-core/src/core/range.rs` 新增 SMPTE 起止时间解析单测并补齐时分秒断言，同时补充 SMPTE 非法分钟值的显式错误回归测试（断言返回 `RtspRangeError::InvalidSmpteTime`）；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_range.rs` 任务 3（`test_clock_parse`）。
- 2026-04-19：已完成 `test_clock_parse` 迁移，在 `cheetah-rtsp-core/src/core/range.rs` 新增 clock 区间解析单测并补齐起始 UTC 时间与 `end=None` 断言，复用现有 `RtspRangeError::InvalidClockRange` 显式错误处理路径；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_range.rs` 任务 4（`test_npt_reverse_range`）。
- 2026-04-19：已完成 `test_npt_reverse_range` 迁移，在 `cheetah-rtsp-core/src/core/range.rs` 新增合法反向 NPT 区间（`npt=-30.5`）解析单测并补齐 `start=0/end=30.5` 断言，同时覆盖 `npt=-` 非法输入显式错误分支（`RtspRangeError::InvalidNptRange`）；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_range.rs` 任务 5（`test_smpte_type_preserved`）。
- 2026-04-19：已完成 `test_smpte_type_preserved` 迁移，在 `cheetah-rtsp-core/src/core/range.rs` 将 SMPTE 类型保留与前缀格式化断言合并为 vendor 等价测试 `test_smpte_type_preserved`，覆盖 `smpte` / `smpte-30-drop` / `smpte-25` 的 parse + to_string 一致性；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_range.rs` 任务 6（`test_display`）。
- 2026-04-19：已完成 `test_display` 迁移，在 `cheetah-rtsp-core/src/core/range.rs` 为 `NptRange` 补齐 `new/from_start/all/from_now` 通用构造函数并新增 `test_display` 回归单测，覆盖 `npt=10.5-` 与 `npt=0-` 显式格式化输出；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_rtp_info.rs` 任务 1（`test_parse_single_stream`）。
- 2026-04-19：已完成 `test_parse_single_stream` 迁移，`cheetah-rtsp-core` 新增 `core/rtp_info.rs` 并提供 `RtspRtpInfo` / `RtspRtpInfoStream` / `RtspRtpInfoError` 的统一 RTP-Info 解析与格式化原语，落地 vendor 等价测试 `test_parse_single_stream`，同时补充 `seq` 非法值与缺失 `url` 的显式错误回归测试；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_rtp_info.rs` 任务 2（`test_parse_multiple_streams`）。
- 2026-04-19：已完成 `test_parse_multiple_streams` 迁移，在 `cheetah-rtsp-core/src/core/rtp_info.rs` 新增 vendor 等价多流解析单测并补齐 `url/seq/rtptime` 断言，同时增强 `split_rtp_info_streams` 的通用分流逻辑：当前一条目 `url` 未携带可选参数时，也可正确识别后续 `url=` 条目分隔；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_rtp_info.rs` 任务 3（`test_parse_without_optional`）。
- 2026-04-19：已完成 `test_parse_without_optional` 迁移，在 `cheetah-rtsp-core/src/core/rtp_info.rs` 新增 vendor 等价单测并显式断言“仅 `url` 时 `seq/rtptime` 都为 `None`”语义，复用现有 `RtspRtpInfoStream::parse` 显式错误处理路径；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_rtp_info.rs` 任务 4（`test_display`）。
- 2026-04-19：已完成 `test_display` 迁移，在 `cheetah-rtsp-core/src/core/rtp_info.rs` 新增 vendor 等价单测 `test_display`，覆盖多流 `to_string` 结果中的 `url/seq/rtptime` 字段输出；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_rtp_info.rs` 任务 5（`test_find_by_url`）。
- 2026-04-19：已完成 `test_find_by_url` 迁移，在 `cheetah-rtsp-core/src/core/rtp_info.rs` 新增 vendor 等价单测 `test_find_by_url`，覆盖按 `url` 检索命中与未命中语义，并复用现有 `RtspRtpInfo::find_by_url` 与 `RtspRtpInfoStream::parse` 的显式错误处理路径；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_connection.rs` 任务 1（`RtspConnectionLimits`）。
- 2026-04-19：已完成 `src/rtsp_connection.rs` 任务 1（迁移 `RtspConnectionLimits`，在 `cheetah-rtsp-core/src/core/connection.rs` 新增连接限制模型并提供与 `RtspMessageLimits` 的双向转换，在 `core/mod.rs` 导出 `RtspConnectionLimits` 并新增 `RtspCore::with_connection_limits`，同时将入站 interleaved 解析接入 `max_interleaved_frame_size` 校验并返回显式 `InterleavedFrameSizeLimitExceeded` 错误；`cheetah-rtsp-pbt/tests/prop_limits.rs` 补齐 interleaved 限制命中/放行属性测试），并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_connection.rs` 任务 2（`RtspConnectionState`）。
- 2026-04-19：已完成 `src/rtsp_connection.rs` 任务 2（迁移 `RtspConnectionState`，在 `cheetah-rtsp-core/src/core/connection.rs` 新增协议层连接状态枚举 `Init/Ready/Playing/Recording/Disconnected` 并补齐默认值与语义回归单测，在 `core/mod.rs` 与 `src/lib.rs` 导出该公共原语），并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_connection.rs` 任务 3（`RtspSession::parse` / `to_header`）。
- 2026-04-19：已完成 `src/rtsp_connection.rs` 任务 3（迁移 `RtspSession::parse` / `to_header`，在 `cheetah-rtsp-core/src/core/connection.rs` 新增 `RtspSession` 与显式 `RtspSessionError`，覆盖 `Session` 头 `session-id/timeout` 解析与 `to_header` roundtrip，并补充空会话 ID、非法 timeout、非法 session-id 的错误回归测试；在 `core/mod.rs` 与 `src/lib.rs` 导出该公共原语），并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_connection.rs` 任务 4（`RtspTransport::parse` / `parse_multiple` / `to_header`）。
- 2026-04-19：已完成 `src/rtsp_connection.rs` 任务 4（迁移 `RtspTransport::parse` / `parse_multiple` / `to_header`，在 `cheetah-rtsp-core/src/core/transport.rs` 补齐 vendor 等价构造能力 `new` / `rtp_avp_tcp_interleaved` / `rtp_avp_udp`，并在 `parse` 与 `parse_multiple` 路径新增控制字符与非法协议校验以统一错误处理（显式返回 `RtspTransportError::InvalidHeaderValue` / `InvalidProtocol`）；同时补充单值端口/通道自动配对与构造器语义回归测试），并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_connection.rs` 任务 5（`parse_interleaved_frame`）。
- 2026-04-19：已完成 `src/rtsp_connection.rs` 任务 5（迁移 `parse_interleaved_frame`，在 `cheetah-rtsp-core/src/core/connection.rs` 新增公共 interleaved 帧头解析原语 `parse_interleaved_frame` 与 `RtspInterleavedFrameHeader`，覆盖“非 `$` 输入/短帧头/部分负载”边界语义并保证无 panic；同时在 `cheetah-rtsp-core/src/core/interleaved.rs` 复用该原语统一核心热路径解析行为，避免重复实现并保持限制校验路径不变），并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_connection.rs` 任务 6（`encode_interleaved_frame`）。
- 2026-04-19：已完成 `src/rtsp_connection.rs` 任务 6（迁移 `encode_interleaved_frame`，在 `cheetah-rtsp-core/src/core/connection.rs` 新增公共编码原语 `encode_interleaved_frame` 与显式错误 `RtspInterleavedEncodeError::PayloadTooLarge`，并在 `cheetah-rtsp-core/src/core/interleaved.rs` 复用该原语统一 core 命令路径编码逻辑，避免重复实现并保持既有 `RtspCoreError::InterleavedPayloadTooLarge` 错误语义不变），并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_connection.rs` 任务 7（`supported_methods`）。
- 2026-04-19：已完成 `src/rtsp_connection.rs` 任务 7（迁移 `supported_methods`，在 `cheetah-rtsp-core/src/core/connection.rs` 新增标准 RTSP 方法集合原语 `supported_methods` 并补齐 vendor 顺序回归测试，在 `core/mod.rs` 与 `src/lib.rs` 导出该能力以供上层统一复用；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `vendor-ref/rtsp-rs/src/rtsp_connection.rs` 任务 8（`public_header_value`）。
- 2026-04-19：已完成 `src/rtsp_connection.rs` 任务 8（迁移 `public_header_value`，在 `cheetah-rtsp-core/src/core/connection.rs` 新增 `public_header_value` 原语并复用 `supported_methods` 保持输出顺序一致，补齐 vendor 等价回归测试 `public_header_value_matches_vendor_semantics`，并在 `core/mod.rs` 与 `src/lib.rs` 导出该公共能力）；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 Phase 04 Fuzz 任务 1（迁移 `fuzz_interleaved.rs`）。
- 2026-04-19：已完成 Phase 04 Fuzz 任务 1（迁移 `fuzz_interleaved.rs`，将 `cheetah-rtsp-fuzz/fuzz_targets/fuzz_interleaved.rs` 从“构造合法帧并驱动 core”收敛为 vendor 等价语义：直接对 `parse_interleaved_frame` 执行稳健性 fuzz，保留“解析失败可接受但不得 panic”的错误处理策略）；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-fuzz`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo test` 回归；下一步进入 Phase 05 `src/sdp.rs` 任务 1（迁移 `test_parse_sdp`）。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-rtsp-core --tests`
- `cargo test -p cheetah-rtsp-core`
- `cargo test -p cheetah-rtsp-pbt`
- `cargo test -p cheetah-rtsp-module`
- `cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`
