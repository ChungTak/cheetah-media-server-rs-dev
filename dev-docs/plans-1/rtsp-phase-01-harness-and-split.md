# Phase 01: 测试承载与文件拆分

- 状态：已完成（任务 1-4 已完成）
- 范围：建立 RTSP 专用 `pbt` / `fuzz` crate，先把本地超大 RTSP 文件拆到可持续维护的粒度。
- 对应用例：无直接 vendor 用例；本阶段为后续迁移提供基础设施。
- 完成标准：`cheetah-rtsp-pbt`、`cheetah-rtsp-fuzz` 可被工作区识别，RTSP 关键大文件完成第一轮拆分。

## 最新进展

- 2026-04-18：任务 1 已完成（新建 `cheetah-rtsp-pbt`、`cheetah-rtsp-fuzz`，并接入工作区）。
- 2026-04-18：任务 2 已完成（`cheetah-rtsp-core` 从 `src/core.rs` 拆分为 `core/{message,limits,interleaved,method}.rs` + `core/mod.rs`，并保留显式 `Input / Output / Event` 模型）。
- 2026-04-18：任务 3 已完成（`cheetah-rtsp-driver-tokio` 的 `server.rs` 拆分为 `server/{mod,listener,connection,command,tests}.rs`，并保持对外 API 不变）。
- 2026-04-18：任务 4 已完成（`cheetah-rtsp-module/src/module.rs` 拆分为 `module/{request_dispatch,publish,play,session_guard,response,cleanup}.rs`，`tests/keepalive.rs` 拆分为 `tests/{keepalive_basic,publish_record,play_pause,udp_forwarding,multitrack}.rs` + `tests/common/mod.rs`）。
- 下一步：进入 Phase 02，迁移 `pbt_http.rs` 的第一个用例 `test_http_request_roundtrip_no_body`。

## 目标

- 参照现有 `cheetah-rtmp-pbt` 与 `cheetah-rtmp-fuzz` 的承载方式，为 RTSP 建立独立测试入口。
- 解决当前 `crates/cheetah-rtsp-module/src/module.rs` 与 `crates/cheetah-rtsp-module/tests/keepalive.rs` 过大的问题，为逐案迁移创造可维护边界。
- 约束后续新增测试文件命名和归档位置，避免测试继续散落在单一巨型文件中。

## 具体任务

### 1. 新建测试承载 crate（已完成）

- 新建 `crates/cheetah-rtsp-pbt`：
  - `Cargo.toml` 只依赖 RTSP 相关 crate 与 `proptest`。
  - 初始测试文件固定命名：
    - `tests/prop_message.rs`
    - `tests/prop_limits.rs`
    - `tests/prop_rtp.rs`
    - `tests/prop_rtcp.rs`
    - `tests/prop_rtsp.rs`
    - `tests/prop_sdp.rs`
- 新建 `crates/cheetah-rtsp-fuzz`：
  - 采用与 `cheetah-rtmp-fuzz` 一致的 `cargo-fuzz` 结构。
  - 初始目标固定命名：
    - `fuzz_targets/fuzz_http_request.rs`
    - `fuzz_targets/fuzz_http_response.rs`
    - `fuzz_targets/fuzz_interleaved.rs`
    - `fuzz_targets/fuzz_rtp.rs`
    - `fuzz_targets/fuzz_rtcp.rs`
    - `fuzz_targets/fuzz_sdp.rs`
    - `fuzz_targets/fuzz_rtsp_core.rs`
    - `fuzz_targets/fuzz_rtsp_limits.rs`
- 更新工作区 `Cargo.toml`，加入上述两个 crate。

### 2. 拆分 `cheetah-rtsp-core`（已完成）

- 将当前 `src/core.rs` 拆为最小可维护子模块：
  - `message.rs`：RTSP request/response 起始行、头、body、增量解码。
  - `limits.rs`：消息、头数、头行、body、interleaved 大小限制。
  - `interleaved.rs`：`$` frame 解析与编码。
  - `method.rs`：方法枚举与解析。
  - 后续阶段再追加 `rtp.rs`、`rtcp.rs`、`range.rs`、`transport.rs`、`rtp_info.rs`、`sdp.rs`。
- 保持 `RtspCore` 对外仍然是显式 `Input / Output / Event` 模型，不引入 runtime 依赖。

### 3. 拆分 `cheetah-rtsp-driver-tokio`（已完成）

- 将 `src/server.rs` 至少拆成：
  - `server/listener.rs`
  - `server/connection.rs`
  - `server/command.rs`
  - `server/tests.rs`
- 明确未来与 vendor 客户端连接语义的映射落点：
  - 分段收包、close、缓冲处理、写队列溢出在 driver。
  - 解析语义在 core。

### 4. 拆分 `cheetah-rtsp-module`（已完成）

- 将 `src/module.rs` 至少拆成：
  - `request_dispatch.rs`
  - `publish.rs`
  - `play.rs`
  - `session_guard.rs`
  - `response.rs`
  - `cleanup.rs`
- 将 `tests/keepalive.rs` 按场景拆分为：
  - `tests/keepalive_basic.rs`
  - `tests/publish_record.rs`
  - `tests/play_pause.rs`
  - `tests/udp_forwarding.rs`
  - `tests/multitrack.rs`
  - `tests/common/mod.rs`

## 产出要求

- 每个新 crate 至少有一个占位测试 / fuzz target，保证后续可以逐案填充。
- 所有新测试文件头部统一标注来源 vendor 文件。
- 所有注释统一使用中文。

## 完成后检查

- `cargo fmt`
- `cargo check -p cheetah-rtsp-core`
- `cargo check -p cheetah-rtsp-driver-tokio`
- `cargo check -p cheetah-rtsp-module`
- `cargo check -p cheetah-rtsp-pbt`
- `cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`
