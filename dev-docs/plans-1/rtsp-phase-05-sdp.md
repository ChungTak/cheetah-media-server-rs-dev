# Phase 05: SDP 通用能力迁移

- 状态：已完成（`src/sdp.rs` 任务 1-2、`pbt_sdp.rs` 任务 1-9、Fuzz 任务 1 已完成）
- 范围：迁移 SDP parse/build/builder、属性测试、fuzz，并回收 module 私有 SDP 通用逻辑。
- 对应用例：`pbt_sdp.rs`、`src/sdp.rs`、`fuzz_sdp.rs`。
- 完成标准：`cheetah-rtsp-core` 提供通用 SDP 原语，module 中 SDP 逻辑只保留业务映射。

## 来源用例

### `vendor-ref/rtsp-rs/pbt/tests/pbt_sdp.rs`

1. [x] `test_sdp_roundtrip_basic`
2. [x] `test_sdp_roundtrip_with_connection`
3. [x] `test_sdp_roundtrip_with_media`
4. [x] `test_sdp_roundtrip_with_attributes`
5. [x] `test_sdp_builder`
6. [x] `test_sdp_media_builder`
7. [x] `test_sdp_rtpmap_roundtrip`
8. [x] `test_sdp_bandwidth_roundtrip`
9. [x] `test_sdp_media_num_ports_roundtrip`

### `vendor-ref/rtsp-rs/src/sdp.rs`

1. [x] `test_parse_sdp`
2. [x] `test_build_sdp`

### Fuzz

- [x] `fuzz_sdp.rs`

## 本地目标设计

### 1. `cheetah-rtsp-core` 新增通用 SDP 模块

- 提供以下通用结构：
  - `Sdp`
  - `SdpOrigin`
  - `SdpConnection`
  - `SdpTiming`
  - `SdpMedia`
  - `SdpAttribute`
  - `SdpBuilder`
  - `SdpMediaBuilder`
- 支持 parse / to_string / builder 双向回归。

### 2. 业务映射与通用能力分离

- `cheetah-rtsp-module/src/sdp.rs` 只保留：
  - ANNOUNCE SDP 到本地轨道/能力模型的业务映射
  - DESCRIBE 响应的业务视图组装
  - 业务特定参数集与 codec 映射
- 通用 SDP 文法、builder、属性格式化全部迁到 core。

### 3. 与 codec / module 的边界

- SDP 原语只处理 SDP 文本，不直接依赖 `EngineContext`、`StreamManager`、Tokio。
- H264/H265/AAC 等媒体业务解释仍由 module 完成，但其输入应来自 core 解析后的结构体，不再是原始字符串行。

## 逐案迁移步骤

- 先迁 `src/sdp.rs` 的 `test_parse_sdp` / `test_build_sdp`，建立最小 parse/build 主线。
- 再迁 `pbt_sdp.rs` 的 basic / connection / media / attributes roundtrip。
- 最后迁 builder、bandwidth、num_ports 与 fuzz。
- 每迁完一批，就同步把 module 中的通用 SDP 解析逻辑替换为 core 结构化输入。

## 完成判定

- `prop_sdp.rs` 中对应 vendor 用例全部迁完。
- core 提供通用 SDP 原语。
- module SDP 文件只保留业务映射，不再承担通用 SDP 文法职责。
- 相关注释已全部翻译为中文。

## 最新进展

- 2026-04-19：已完成 `src/sdp.rs` 任务 1（迁移 `test_parse_sdp`），在 `cheetah-rtsp-core/src/core/sdp.rs` 新增通用 SDP 结构化原语 `Sdp` / `SdpOrigin` / `SdpConnection` / `SdpTiming` / `SdpMedia` / `SdpAttribute` 与显式错误 `SdpError`，完成 vendor 等价 `test_parse_sdp` 迁移并补充 `missing origin` 与 `invalid rtpmap` 错误回归测试；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `src/sdp.rs` 任务 2（迁移 `test_build_sdp`）。
- 2026-04-19：已完成 `src/sdp.rs` 任务 2（迁移 `test_build_sdp`），在 `cheetah-rtsp-core/src/core/sdp.rs` 补齐 `Sdp::builder`、`SdpBuilder`、`SdpMediaBuilder` 与 `Display/to_string` 通用构建能力，并新增 `builder_requires_required_fields` 错误回归测试以显式校验缺失 `o/s/t` 必填字段时返回 `SdpError::MissingRequiredField`；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `pbt_sdp.rs` 任务 1（迁移 `test_sdp_roundtrip_basic`）。
- 2026-04-19：已完成 `pbt_sdp.rs` 任务 1（迁移 `test_sdp_roundtrip_basic`），在 `cheetah-rtsp-pbt/tests/prop_sdp.rs` 替换占位测试并新增 vendor 等价属性用例与 `origin/session_name/timing` 生成器，覆盖 `Sdp::to_string` / `Sdp::parse` 基础 roundtrip 核心字段断言，复用 `cheetah-rtsp-core` 既有显式 `SdpError` 错误处理路径；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `pbt_sdp.rs` 任务 2（迁移 `test_sdp_roundtrip_with_connection`）。
- 2026-04-19：已完成 `pbt_sdp.rs` 任务 2（迁移 `test_sdp_roundtrip_with_connection`），在 `cheetah-rtsp-pbt/tests/prop_sdp.rs` 新增 `valid_connection` 生成器与 vendor 等价属性用例，覆盖 `Sdp` 会话级 `connection` 的 `to_string` / `parse` roundtrip 与 `net_type/addr_type/address` 全字段断言，复用 `cheetah-rtsp-core` 既有显式 `SdpError` 错误处理路径；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `pbt_sdp.rs` 任务 3（迁移 `test_sdp_roundtrip_with_media`）。
- 2026-04-19：已完成 `pbt_sdp.rs` 任务 3（迁移 `test_sdp_roundtrip_with_media`），在 `cheetah-rtsp-pbt/tests/prop_sdp.rs` 新增 `valid_media` 及媒体相关生成器，并落地 vendor 等价属性用例，覆盖 `Sdp` 中 media 列表的 `to_string` / `parse` roundtrip 与 `media_type/port/protocol/formats` 核心字段断言，复用 `cheetah-rtsp-core` 既有显式 `SdpError` 错误处理路径；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `pbt_sdp.rs` 任务 4（迁移 `test_sdp_roundtrip_with_attributes`）。
- 2026-04-19：已完成 `pbt_sdp.rs` 任务 4（迁移 `test_sdp_roundtrip_with_attributes`），在 `cheetah-rtsp-pbt/tests/prop_sdp.rs` 新增 `valid_session_attribute` 生成器与 vendor 等价属性用例，覆盖会话级 attributes 的 `to_string` / `parse` roundtrip 并断言属性数量一致，复用 `cheetah-rtsp-core` 既有显式 `SdpError` 错误处理路径；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `pbt_sdp.rs` 任务 5（迁移 `test_sdp_builder`）。
- 2026-04-19：已完成 `pbt_sdp.rs` 任务 5（迁移 `test_sdp_builder`），在 `cheetah-rtsp-pbt/tests/prop_sdp.rs` 新增 vendor 等价属性测试，覆盖 `Sdp::builder().origin_simple(...).session_name(...).timing(...).build()` 的核心构造语义，并断言 `version/session_id/address/session_name` 关键字段；复用 `cheetah-rtsp-core` 既有显式 `SdpError` 错误处理路径。并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `pbt_sdp.rs` 任务 6（迁移 `test_sdp_media_builder`）。
- 2026-04-19：已完成 `pbt_sdp.rs` 任务 6（迁移 `test_sdp_media_builder`），在 `cheetah-rtsp-pbt/tests/prop_sdp.rs` 新增 vendor 等价属性测试，覆盖 `SdpMediaBuilder::video(...).format(...).rtpmap(...).control(...).build()` 的核心构造语义，并断言 `media_type/port/protocol/formats/attributes` 关键字段；复用 `cheetah-rtsp-core` 既有显式 `SdpError` 错误处理路径。并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `pbt_sdp.rs` 任务 7（迁移 `test_sdp_rtpmap_roundtrip`）。
- 2026-04-19：已完成 `pbt_sdp.rs` 任务 7（迁移 `test_sdp_rtpmap_roundtrip`），在 `cheetah-rtsp-pbt/tests/prop_sdp.rs` 新增 vendor 等价测试，覆盖 `rtpmap` 属性 parse/to_string/再 parse roundtrip，并断言 `payload_type/encoding/clock_rate` 关键字段；复用 `cheetah-rtsp-core` 既有显式 `SdpError` 错误处理路径。并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `pbt_sdp.rs` 任务 8（迁移 `test_sdp_bandwidth_roundtrip`）。
- 2026-04-19：已完成 `pbt_sdp.rs` 任务 8（迁移 `test_sdp_bandwidth_roundtrip`），在 `cheetah-rtsp-pbt/tests/prop_sdp.rs` 新增 vendor 等价测试，覆盖会话级 `b=` 字段 parse/to_string/再 parse roundtrip，并断言 `bwtype/bandwidth` 关键字段；复用 `cheetah-rtsp-core` 既有显式 `SdpError::InvalidBandwidth` / `InvalidNumber` 错误处理路径。并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 `pbt_sdp.rs` 任务 9（迁移 `test_sdp_media_num_ports_roundtrip`）。
- 2026-04-19：已完成 `pbt_sdp.rs` 任务 9（迁移 `test_sdp_media_num_ports_roundtrip`），在 `cheetah-rtsp-pbt/tests/prop_sdp.rs` 新增 vendor 等价测试，覆盖 `m=<media> <port>/<num_ports> <proto> ...` 的 parse/to_string/再 parse roundtrip，并断言 `port/num_ports` 关键字段；同时在 `cheetah-rtsp-core/src/core/sdp.rs` 补充 `parse_rejects_invalid_media_num_ports` 回归测试，显式校验非法 `num_ports` 返回 `SdpError::InvalidNumber { field: "media.num_ports", ... }`。并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-pbt --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test` 回归；下一步进入 Phase 05 Fuzz 任务 1（迁移 `fuzz_sdp.rs`）。
- 2026-04-19：已完成 Phase 05 Fuzz 任务 1（迁移 `fuzz_sdp.rs`，将 `cheetah-rtsp-fuzz/fuzz_targets/fuzz_sdp.rs` 收敛为 vendor 等价语义：仅在 UTF-8 输入上执行 `Sdp::parse`，解析成功后执行 `to_string`，并保留“解析失败可接受但不得 panic”的错误处理策略）；并完成 `cargo fmt`、`cargo clippy -p cheetah-rtsp-fuzz`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-pbt`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo test` 回归；下一步进入 Phase 06 任务 1（`cheetah-rtsp-driver-tokio` 集成测试补齐）。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-rtsp-core --tests`
- `cargo test -p cheetah-rtsp-core`
- `cargo test -p cheetah-rtsp-pbt`
- `cargo test -p cheetah-rtsp-module`
- `cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`
