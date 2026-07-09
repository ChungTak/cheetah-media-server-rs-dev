# Phase 03: 引擎 bootstrap 与播放启动 pacing

- 状态：已完成
- 范围：订阅者启动策略、RingBuffer 起播选择、RTMP/RTSP 播放端启动 pacing、慢订阅者隔离。
- 完成标准：新播放器接入时能基于最新可用随机访问点秒开，首段历史帧不会被无节制快速吐出，播放开始后 1 秒内画面速率稳定。

## 具体任务

### 3.1 `SubscriberOptions` / `BootstrapPolicy` 设计

- [x] 定义协议无关的订阅启动策略，例如 live-tail、full-gop、none。
- [x] live 场景默认选择最新可用随机访问点，并限制可回放历史帧的最大时长或最大帧数。
- [x] 策略中显式表达 `max_bootstrap_age_ms`、`max_bootstrap_frames`、是否等待下一个随机访问点。
- [x] RTSP/RTMP module 只能选择策略，不直接扫描和改写媒体缓存。

### 3.2 RingBuffer live-tail bootstrap

- [x] RingBuffer 支持查找不超过最大年龄的最新随机访问点。
- [x] 如果没有可接受随机访问点，live 播放应等待下一个随机访问点，而不是推送过旧 GOP。
- [x] bootstrap 帧集合必须有上界，避免新订阅者拖累发布者或其他订阅者。
- [x] 断流或 track reset 时，bootstrap 选择必须能识别 discontinuity。

### 3.3 RTMP play 启动 pacing

- [x] metadata、codec config、首个可解码媒体帧可以立即发送。
- [x] 后续 bootstrap 媒体帧按归一化媒体时间 pacing，不允许把积压 GOP 一次性冲出。
- [x] 处理音视频交织，保证音频不会触发视频启动快放。
- [x] discontinuity、timestamp reset、publisher restart 后重新建立 pacing 基准。

### 3.4 RTSP play 启动 pacing

- [x] RTSP play 的 RTP packet 输出按归一化媒体时间 pacing。
- [x] TCP interleaved 和 UDP 发送路径共享同一套 pacing 决策。
- [x] SDP、SETUP、PLAY 的协议状态不携带媒体时间修正逻辑。
- [x] 处理多 track 同步，避免音频或视频单 track 积压导致整体启动快放。

### 3.5 慢订阅者与积压回归

- [x] 慢订阅者队列有明确上界。
- [x] 慢订阅者不拖累发布者和其他订阅者。
- [x] 新订阅者不会因历史积压获得超出策略限制的帧。
- [x] 测试覆盖高帧率视频、大 GOP、音视频混合、低码率音频、断流重连。

## 最新进展

- 2026-04-29：完成任务 3.5（慢订阅者与积压回归）。`cheetah-engine` 的 `SubscriberApi::subscribe` 新增订阅参数校验：`queue_capacity` 必须大于 0 且不得小于 `bootstrap_policy.max_bootstrap_frames`，避免 bootstrap 窗口配置与队列上界冲突导致静默截断；补充回归测试覆盖“慢订阅者不拖累快订阅者与发布者分发结果”“高帧率视频 + 低码率音频 + 大 GOP + 断流重连场景下 bootstrap 仅回放断流后窗口并遵守 `max_bootstrap_frames` 上界”“非法订阅窗口参数返回 `InvalidArgument`”。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-engine --all-targets -- -D warnings`、`cargo clippy -p cheetah-sdk --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-sdk`、`cargo test -p cheetah-engine`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成任务 3.4（RTSP play 启动 pacing）。`cheetah-rtsp-module` 在 `handle_play` 发送循环新增 runtime-neutral `PlayStartPacingState` 与统一等待逻辑：首个媒体帧立即发送，后续帧按统一媒体毫秒时间线延迟发送；`FrameFlags::DISCONTINUITY`、大幅时间戳回退与异常前跳时重建 pacing 锚点；同一 pacing 状态跨音视频 track 共享，避免单 track 积压触发整体启动快放；`TCP interleaved` 与 `UDP unicast` 发送路径复用同一 pacing 决策。新增单测覆盖“首帧立即+后续按 delta 延迟”“discontinuity/回退重建基准”“音视频交织共享单时间线”“媒体时间戳优先级与 timebase 转换”。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-sdk --all-targets -- -D warnings`、`cargo clippy -p cheetah-engine --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-sdk`、`cargo test -p cheetah-engine`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成任务 3.2（RingBuffer live-tail bootstrap）。`cheetah-engine` 在 RingBuffer bootstrap 起点裁剪中新增 `discontinuity` 边界识别：先按 `max_bootstrap_frames` / `max_bootstrap_age_ms` 收敛窗口，再将起点提升到窗口内最近 `FrameFlags::DISCONTINUITY`，最后执行随机访问点选择，从而避免 track reset 后把旧 GOP 注入新订阅者。新增 `bootstrap_waits_for_next_keyframe_after_discontinuity` 与 `bootstrap_fallback_does_not_cross_discontinuity_boundary` 回归测试，分别验证“等待 keyframe”与“关闭等待时 fallback 不跨 discontinuity”语义。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-sdk --all-targets -- -D warnings`、`cargo clippy -p cheetah-engine --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-sdk`、`cargo test -p cheetah-engine`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成 Phase 03 任务 3.1（`SubscriberOptions` / `BootstrapPolicy` 设计）。`cheetah-sdk` 新增协议无关订阅启动策略模型：`BootstrapMode`（`None`/`LiveTail`/`FullGop`）与 `BootstrapPolicy`（显式 `max_bootstrap_age_ms`、`max_bootstrap_frames`、`wait_for_next_random_access_point`），`SubscriberOptions` 改为持有策略对象；`cheetah-engine` 的 RingBuffer bootstrap 入口切换为策略驱动，并补充最大年龄窗口与随机访问点等待/回退语义测试；RTMP/RTSP module 仅构造并传递策略，不再通过订阅选项暴露缓存扫描细节。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-sdk --all-targets -- -D warnings`、`cargo clippy -p cheetah-engine --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-sdk`、`cargo test -p cheetah-engine`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：计划已创建，任务未开始。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-sdk`
- `cargo test -p cheetah-sdk`
- `cargo clippy -p cheetah-engine`
- `cargo test -p cheetah-engine`
- `cargo test -p cheetah-rtmp-module`
- `cargo test -p cheetah-rtsp-module`
- 使用 ffplay 拉流验证首 1 秒无明显快放。
