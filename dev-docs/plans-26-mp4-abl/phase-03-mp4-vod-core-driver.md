# Phase 03: MP4 VOD `core + driver + module`

- **状态**: 已完成
- **目标**: 建立本地 MP4 文件点播主路径，支持 seek、pause、resume、speed、loop 和多文件回放
- **完成标准**: `cheetah-mp4-core`、`cheetah-mp4-driver-tokio`、`cheetah-mp4-module` 的职责边界与控制模型明确

## 实现概览（ABL 增量）

- `cheetah-mp4-core` 在原有 `Start/Seek/Pause/Scale/Stop/Tick` 状态机基础上新增 `VodOutput::Diagnostic(VodDiagnostic)`：
  - `VodDiagnostic::SeekOutOfRange { requested_us, duration_us }` 用于 ABL "seek 越界返回明确错误" 要求；session 在 `Seek` 命令位置非法时直接返回 diagnostic 且不修改播放位置。
  - `Scale` 上界放宽到 `0.05..=32.0`，覆盖 ABL 要求的 16x 高倍速；具体限速由 driver 层 keyframe-only 策略保护网络。
  - 其他 `InvalidState` 字段为协议层传递审计/错误响应预留。
- `cheetah-mp4-driver-tokio::VodDriverConfig` 新增：
  - `read_count: i32`：`1`（默认） / `n>1` / `-1` 无限循环 / `0` 拒绝启动。`run_multi_driver` 在 playlist 维度循环。
  - `keyframe_only_above_speed: f32`：默认 8.0；当 `Scale >= threshold` 时 `drive_outputs_filtered` 丢弃非关键帧 video sample，对齐 ABL 8x/16x 的关键帧回放策略；audio 始终透传以保持时间锚点。
  - `Diagnostic` 事件类型转发 core 诊断。
- `cheetah-mp4-module::api::StartVodRequest::loop_count`：把 `None / Some(n) / Some(u32::MAX)` 映射成驱动层 `read_count`，并对 `Some(0)` 返回 `InvalidRequest`。
- `VodSessionRecord` 新增 `reader_count / remote_ip / remote_port / network_type / params` 字段，预留 ABL `on_rtsp_replay` 风格审计 hook 的承接点。
- 测试覆盖：
  - `cheetah-mp4-core`（8 用例）含 `seek_negative_position_emits_diagnostic` / `seek_past_duration_emits_diagnostic` / `seek_within_duration_succeeds` / `scale_clamp_allows_high_speed_playback`。
  - `cheetah-mp4-driver-tokio`（6 用例）含 `read_count_repeats_playback` / `read_count_zero_refuses_start`。

## `cheetah-mp4-core`

负责纯状态机：

1. `Open` 后暴露轨道、duration、初始 timeline
2. `ReadNext` 输出下一帧或下一次调度需求
3. `SeekTo` 将逻辑位置映射到关键 sample 或可恢复点
4. `Pause`、`Resume`、`SetSpeed`、`Stop`
5. `read_count` 与 EOF loop 计数

核心约束：

1. 不持有文件句柄或 Tokio 对象
2. 输入输出使用显式 command / event / timer
3. seek 后要求触发 codec config 补发
4. 高倍速模式允许只输出关键帧

## `cheetah-mp4-driver-tokio`

负责运行时：

1. 打开文件和多文件清单
2. 驱动 sample 读取与定时发送
3. 在 EOF 时根据 `read_count` 重置到文件头或关闭任务
4. 管理 pause、resume、seek、speed 指令的串行执行
5. 空闲超时关闭，避免无读者任务泄漏

## `cheetah-mp4-module`

负责系统接入：

1. 将点播任务发布为 engine 可消费的 media source
2. 暴露 `VodControlApi`
3. 管理 `stream_key -> vod session` 映射
4. 维护回放会话审计字段，为 `on_rtsp_replay` 类事件预留数据

## ABL 对齐要求

1. `read_count = 1` 默认播放一次，`-1` 无限循环
2. seek 越界返回显式错误
3. 8x、16x 回放启用关键帧模式
4. 多文件回放按时间顺序组成单一 timeline
5. 支持直接加载绝对路径文件和 catalog 反查文件

## 测试要求

1. core 单元测试覆盖 open、read、pause、resume、seek、speed、stop、loop
2. seek 测试覆盖关键帧 seek、无 `stss` 降级 seek 和越界错误
3. driver 集成测试覆盖单文件、多文件、无限循环和空闲关闭
4. module 测试覆盖 session 生命周期和 stream key 冲突
