# Phase 02: RTSP 消息编解码与限制模型

- 状态：已完成（任务 1-5、限制任务 1-9 与 Fuzz 任务 1-3 已完成）
- 范围：迁移 RTSP 请求/响应消息 roundtrip、增量解码、限制模型与相关 fuzz。
- 对应用例：`pbt_http.rs`、`pbt_limits.rs` 中 HTTP/limit 用例、`fuzz_http_request.rs`、`fuzz_http_response.rs`、`fuzz_rtsp_limits.rs` 的消息部分。
- 完成标准：本地具备独立 RTSP request/response codec 与 limits 模型，所有对应 PBT/fuzz 测试可承载并通过。

## 来源用例

### `vendor-ref/rtsp-rs/pbt/tests/pbt_http.rs`

按源码顺序逐个迁移：

1. [x] `test_http_request_roundtrip_no_body`
2. [x] `test_http_request_roundtrip_with_body`
3. [x] `test_http_response_roundtrip_no_body`
4. [x] `test_http_response_roundtrip_with_body`
5. [x] `test_http_request_chunked_feed`

### `vendor-ref/rtsp-rs/pbt/tests/pbt_limits.rs`

先迁移消息限制相关用例：

1. [x] `test_buffer_size_limit_exceeded`
2. [x] `test_buffer_size_within_limit`
3. [x] `test_header_count_limit_exceeded`
4. [x] `test_header_count_within_limit`
5. [x] `test_body_size_limit_exceeded`
6. [x] `test_body_size_within_limit`
7. [x] `test_header_line_size_limit_exceeded`
8. [x] `test_request_roundtrip_with_limits`
9. [x] `test_response_roundtrip_with_limits`

### Fuzz

1. [x] `fuzz_http_request.rs`
2. [x] `fuzz_http_response.rs`
3. [x] `fuzz_rtsp_limits.rs` 中消息限制与解码部分

## 本地目标设计

### 1. `cheetah-rtsp-core` 新增通用消息层

- 新增 `RtspRequestMessage`、`RtspResponseMessage` 或等价命名的纯协议对象。
- 将当前只支持 request 的解析逻辑扩展为 request/response 双向支持。
- 明确区分：
  - 起始行解析
  - 头字段解析与保留顺序
  - `Content-Length` 语义
  - 增量解码缓冲状态
- 不引入任何 Tokio、socket、时间调用。

### 2. 引入显式限制模型

- 设计 `RtspMessageLimits` 或等价结构，至少包含：
  - `max_buffer_size`
  - `max_headers_count`
  - `max_header_line_size`
  - `max_body_size`
  - `max_interleaved_frame_size`
  - 可选 `validate_version`
- `RtspCore` 与独立 decoder 共用该限制配置，避免 driver/module 各自维护私有限制。

### 3. 现有 `RtspCore` 的改造范围

- 支持 request/response 双向消息对象，以承接 vendor response 用例。
- 对输入进行增量解析时，正确处理：
  - 头未收全
  - body 未收全
  - request/response 完整后一次或多次吐出事件
  - interleaved 与 RTSP 消息边界共存
- 保留现有 `RtspCommand::SendResponse`，但其底层应复用通用 response encoder。

### 4. driver 配合项

- driver 只负责把字节流交给 core，不自行复制 body/头限制逻辑。
- 若因限制命中产生错误，driver 负责把错误转成连接关闭原因并结束连接。

## 逐案迁移步骤

- 先在 `cheetah-rtsp-pbt/tests/prop_message.rs` 中迁入 `pbt_http.rs` 的第 1 个测试。
- 单测失败后，再把 request codec 缺口补到 core。
- request 系列稳定后，再迁 response roundtrip。
- chunked feed 最后迁，确保增量 decoder 状态模型已经稳定。
- `limits` 用例按“buffer -> header count -> body -> header line -> roundtrip”顺序推进。
- Fuzz target 放在 PBT 稳定后补入，避免把未定型接口提前固化。

## 完成判定

- `prop_message.rs` 与 `prop_limits.rs` 中对应 vendor 用例全部迁完。
- core 能独立编码/解码 RTSP request/response。
- `max_*` 限制全部在 core 统一生效。
- 相关注释已全部翻译为中文。

## 最新进展

- 2026-04-18：已完成 `test_http_request_roundtrip_no_body` 迁移，新增 `RtspRequestMessage`、`encode_rtsp_request`、`RtspRequestDecoder`，并补齐请求解码遇到 response 起始行与非法头行的错误处理；下一步进入任务 2（`test_http_request_roundtrip_with_body`）。
- 2026-04-18：已完成 `test_http_request_roundtrip_with_body` 迁移，`prop_message.rs` 新增带 body 的 request roundtrip 属性测试并校验 `Content-Length` 自动补齐；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入任务 3（`test_http_response_roundtrip_no_body`）。
- 2026-04-18：已完成 `test_http_response_roundtrip_no_body` 迁移，新增 `RtspResponseMessage`、`encode_rtsp_response`、`RtspResponseDecoder`，并补齐 response/request 解码类型不匹配错误处理；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入任务 4（`test_http_response_roundtrip_with_body`）。
- 2026-04-18：已完成 `test_http_response_roundtrip_with_body` 迁移，`prop_message.rs` 新增带 body 的 response roundtrip 属性测试并校验 body/状态行与 `Content-Length` 自动补齐；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入任务 5（`test_http_request_chunked_feed`）。
- 2026-04-18：已完成 `test_http_request_chunked_feed` 迁移，`prop_message.rs` 新增逐字节分块 `feed` 的请求增量解码属性测试，并在每次 `decode` 路径显式校验“未完成返回 `None`、末字节完成返回请求对象”；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 `pbt_limits.rs` 任务 1（`test_buffer_size_limit_exceeded`）。
- 2026-04-18：已完成 `test_buffer_size_limit_exceeded` 迁移，新增 `RtspMessageLimits` 并在 `RtspRequestDecoder` / `RtspResponseDecoder` / `RtspCore` 的 `feed` 路径统一执行 `max_buffer_size` 校验；`prop_limits.rs` 新增超限随机字节输入用例并覆盖错误返回路径，下一步进入 `pbt_limits.rs` 任务 2（`test_buffer_size_within_limit`）。
- 2026-04-18：已完成 `test_buffer_size_within_limit` 迁移，`prop_limits.rs` 新增 buffer 限制内随机字节输入用例并显式校验 `feed` 返回 `Ok`，用于覆盖限制边界内的正常路径；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 `pbt_limits.rs` 任务 3（`test_header_count_limit_exceeded`）。
- 2026-04-18：已完成 `test_header_count_limit_exceeded` 迁移，`prop_limits.rs` 新增超限 header 数量请求用例，并在 `cheetah-rtsp-core` 解码路径统一新增 `max_headers_count` 校验和 `HeaderCountLimitExceeded` 错误；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 `pbt_limits.rs` 任务 4（`test_header_count_within_limit`）。
- 2026-04-18：已完成 `test_header_count_within_limit` 迁移，`prop_limits.rs` 新增限制内 header 数量请求可成功解码的属性测试（覆盖到上界 `max_headers_count = 5`）；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 `pbt_limits.rs` 任务 5（`test_body_size_limit_exceeded`）。
- 2026-04-18：已完成 `test_body_size_limit_exceeded` 迁移，`prop_limits.rs` 新增超限 `Content-Length` 请求用例并显式断言 `BodySizeLimitExceeded { max: 256, actual }` 错误，确保 body 限制命中语义稳定；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 `pbt_limits.rs` 任务 6（`test_body_size_within_limit`）。
- 2026-04-18：已完成 `test_body_size_within_limit` 迁移，`prop_limits.rs` 新增 body 分段 `feed` 后的限制内成功解码属性测试，并显式断言请求 `body.len()` 与 `Content-Length` 一致；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 `pbt_limits.rs` 任务 7（`test_header_line_size_limit_exceeded`）。
- 2026-04-18：已完成 `test_header_line_size_limit_exceeded` 迁移，`prop_limits.rs` 新增超限 header 单行长度请求的属性测试，并显式断言 `HeaderLineSizeLimitExceeded { max: 128, actual }` 错误，确保 `max_header_line_size` 限制命中语义稳定；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 `pbt_limits.rs` 任务 8（`test_request_roundtrip_with_limits`）。
- 2026-04-18：已完成 `test_request_roundtrip_with_limits` 迁移，`prop_limits.rs` 新增请求在限制范围内的 encode/decode roundtrip 属性测试并对齐 vendor 语义；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 `pbt_limits.rs` 任务 9（`test_response_roundtrip_with_limits`）。
- 2026-04-18：已完成 `test_response_roundtrip_with_limits` 迁移，`prop_limits.rs` 新增响应在限制范围内的 encode/decode roundtrip 属性测试并断言版本、状态码、原因短语与 body 长度；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 Phase 02 Fuzz 任务 1（迁移 `fuzz_http_request.rs`）。
- 2026-04-18：已完成 `fuzz_http_request.rs` 迁移：将本地占位 target 重命名为 `fuzz_http_request`，按 vendor 语义直接对 `RtspRequestDecoder` 执行 `feed + decode`，并显式保留“解析失败可接受，但不得 panic”的错误处理策略；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-fuzz`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 Phase 02 Fuzz 任务 2（迁移 `fuzz_http_response.rs`）。
- 2026-04-18：已完成 `fuzz_http_response.rs` 迁移：将本地占位 target `fuzz_rtsp_response` 重命名为 `fuzz_http_response`，按 vendor 语义直接对 `RtspResponseDecoder` 执行 `feed + decode`，并显式保留“解析失败可接受，但不得 panic”的错误处理策略；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-fuzz`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 Phase 02 Fuzz 任务 3（迁移 `fuzz_rtsp_limits.rs` 中消息限制与解码部分）。
- 2026-04-18：已完成 `fuzz_rtsp_limits.rs` 迁移（消息限制与解码部分）：按 vendor 小限制语义为 request/response decoder 注入 `RtspMessageLimits`，并新增结构化 RTSP 请求/响应输入与分块 `feed + decode` 循环覆盖，显式保留“解析错误可接受但不得 panic”的错误处理策略；回归通过 `cargo fmt`、`cargo clippy -p cheetah-rtsp-fuzz`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，下一步进入 Phase 03 任务 1（迁移 `pbt_rtp.rs` 的 `test_rtp_packet_roundtrip_basic`）。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-rtsp-core --tests`
- `cargo test -p cheetah-rtsp-core`
- `cargo test -p cheetah-rtsp-pbt`
- `cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`
