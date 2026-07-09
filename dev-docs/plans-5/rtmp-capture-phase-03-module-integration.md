# Phase 03: RTMP Module 真实抓包集成回归

- 状态：已完成
- 范围：在 `crates/cheetah-rtmp-module/tests` 中用真实抓包 fixture 驱动 raw TCP publish，验证 module、driver、engine、RTMP play 的集成行为和非标准输入健康度。
- 完成标准：标准 fixture 能通过 module 进入 engine 并被 RTMP play client 拉到媒体；非标准/扰动 fixture 不会导致 rtmp module 崩溃或 engine health 失效。

## 目标文件

```text
crates/cheetah-rtmp-module/tests/rtmp_capture_replay.rs
crates/cheetah-rtmp-module/tests/support/capture_fixture.rs
crates/cheetah-rtmp-module/tests/support/rtmp_test_harness.rs
```

如果现有测试目录不使用 `support/`，实现时仍应把 fixture 读取、engine 启动、raw TCP 写入拆成小模块，避免继续膨胀单个测试文件。

## 具体任务

### 3.1 Module raw TCP publish replay

- [x] 新增测试 harness：动态保留 `127.0.0.1:0` 地址，启动 `EngineBuilder`，注册 `RtmpModuleFactory`。
- [x] 用 `tokio::net::TcpStream` 连接 rtmp listen 地址，按 `.rtmpflow` record 顺序写入 publish C2S payload。
- [x] 写入策略先按原始 record 边界发送；标准样例通过后再新增 coalesced 发送模式。
- [x] raw replay 不解析 pcap，不使用 tokio 类型暴露到 module 公共接口，只存在测试代码。
- [x] 测试结束必须关闭 TCP stream、停止 engine，并等待 client handle shutdown。

### 3.2 RTMP play 验收与 timestamp 断言

- [x] 对标准 H264/AAC、H265/AAC、audio-only fixture，在 raw publish 开始后启动现有 `start_client(..., RtmpClientMode::Play, ...)` 拉流。
- [x] 复用 `rtmp_publish_play_matrix.rs` 中的等待 state 和接收 media 事件思路，抽成 test helper。
- [x] 视频样例断言至少收到一个 video `MediaData`；audio-only 样例断言至少收到一个 audio `MediaData`。
- [x] 收集前几个 media timestamp，断言单调非递减。
- [x] 对 H264/H265 标准样例，若收到 sequence header/config 和 coded frame，断言 coded frame 不早于 play state。

### 3.3 非标准样例 module 健康度回归

- [x] 对 AV1/VP8/VP9/H266/enhanced/fallback probe fixture，只要求 raw replay 后 rtmp module 仍为 `ModuleState::Running`。
- [x] 对截断、丢片、乱序视图，只要求 engine health `is_live()` 和 `is_ready()` 仍为 true。
- [x] 如果连接被 module 主动关闭，测试接受关闭事件，但必须确认 module 可停止，且 stop 后状态为 `Stopped`。
- [x] 不在 module 测试里复制媒体 payload 修复逻辑；遇到 codec 解析不足，只把 fixture 纳入 probe/fuzz，生产逻辑修复另开任务回到 `cheetah-codec` 或 ingest/egress 明确边界。

## 最新进展

- 2026-05-03：完成 3.3。新增 probe fixture 集合和 module health fault view：AV1/VP8/VP9/H266 probe raw replay 后只验证 rtmp module 仍 `Running` 且 engine live/ready；标准与 probe fixture 共同覆盖 prefix 截断、每 N record 丢弃、相邻乱序三类输入扰动。fault replay 写入失败被视为对端关闭的可接受终止，随后仍必须确认 module 可停止且 stop 后状态为 `Stopped`。本阶段不在 module 中复制任何 codec payload 修复逻辑，兼容缺口继续保留在 probe/fuzz 覆盖层。Phase 03 已全部完成。验证已执行：`cargo fmt`、`cargo clippy -p cheetah-rtmp-module --tests`、`cargo test -p cheetah-rtmp-module --test rtmp_capture_replay`、`cargo test -p cheetah-rtmp-module --test rtmp_publish_play_matrix`、`cargo test -p cheetah-rtmp-module --test rtmp_module_push_job_resilience`、`cargo test --workspace`。
- 2026-05-03：完成 3.2。新增 raw publish prefix/session 模式：测试先写入抓包前缀让 module 产生 `StreamSnapshot` 和 ready tracks，随后用 snapshot 的 `StreamKey` 构建 RTMP play URL，等待 `RtmpClientState::Playing` 后继续写完剩余 raw publish records。`rtmp_capture_replay.rs` 新增 play 验收测试覆盖 H264/AAC、H265/AAC、audio-only；视频样例要求收到 video `MediaData`，audio-only 要求收到 audio `MediaData`，收集到的 audio/video timestamp 均断言单调非递减。play helper 在 `Playing` 前收到 media 会直接失败，因此 coded frame 不会早于 play state。验证已执行：`cargo fmt`、`cargo clippy -p cheetah-rtmp-module --tests`、`cargo test -p cheetah-rtmp-module --test rtmp_capture_replay`、`cargo test -p cheetah-rtmp-module --test rtmp_publish_play_matrix`、`cargo test -p cheetah-rtmp-module --test rtmp_module_push_job_resilience`、`cargo test --workspace`。
- 2026-05-03：完成 3.1。新增 `rtmp_capture_replay.rs` 覆盖 4 个标准 publish fixture 的 raw TCP module replay；`support/capture_fixture.rs` 只解码已提交 `.rtmpflow` 的 `CRF1 + big-endian record_count + big-endian length-prefixed payload records`，不触碰原始 pcap。`support/rtmp_test_harness.rs` 负责启动/停止 engine、等待 rtmp module 状态、raw TCP 连接、服务端响应 drain、record 边界写入和 post-control 相邻 payload 粘包写入。测试断言 engine 能观察到 active publisher 和非空 tracks，停止路径关闭 TCP 写半连接、等待读任务、停止 engine，并确认 module `Stopped` 与 health 下线。验证已执行：`cargo fmt`、`cargo clippy -p cheetah-rtmp-module --tests`、`cargo test -p cheetah-rtmp-module --test rtmp_capture_replay`、`cargo test -p cheetah-rtmp-module --test rtmp_publish_play_matrix`、`cargo test -p cheetah-rtmp-module --test rtmp_module_push_job_resilience`、`cargo test --workspace`。
- 2026-05-03：计划已创建，任务未开始。现有 module 测试已有 engine + rtmp client publish/play harness，本阶段补充 raw TCP 真实抓包 publish replay。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-rtmp-module
cargo test -p cheetah-rtmp-module --test rtmp_capture_replay
cargo test -p cheetah-rtmp-module --test rtmp_publish_play_matrix
cargo test -p cheetah-rtmp-module --test rtmp_module_push_job_resilience
```

如果新增测试在低性能环境偶发超时，优先收紧 fixture 前缀和事件等待条件，不放宽 module 生命周期断言。
