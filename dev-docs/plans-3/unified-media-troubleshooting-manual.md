# 统一媒体内核与跨协议起播：故障排查手册（plans-3）

- 状态：已完成
- 范围：RTSP/RTMP 推拉流、双向转协议、RTSP TCP/UDP 差异、后续 SRT/WebRTC 接入。
- 目标：为 `Invalid timestamps`、`Non-increasing DTS`、`Negative cts`、首帧不能秒开、启动快放等问题提供统一定位路径。
- 执行入口：`dev-scripts/cross_protocol_matrix_regression.sh`。
- 命令模板：`dev-scripts/cross_protocol_matrix_command_templates.sh`。

## 1. 排查输入条件（必须记录）

每次排查先固定输入，避免“偶发但不可复现”：

1. 场景：`rtsp-tcp-loopback` / `rtsp-udp-loopback` / `rtmp-loopback` / `bridge-*`。
2. 输入素材：`INPUT_PROFILE`，必须注明 `b_frames=yes/no`。
3. 环境参数：`CHEETAH_HOST`、`RTSP_PORT`、`RTMP_PORT`、`APP_NAME`、`STREAM_NAME`。
4. 回归参数：`SCENARIO_DURATION_SECONDS`、`PUSH_STARTUP_GRACE_MS`、`STARTUP_POLL_INTERVAL_MS`。
5. 验收矩阵：`MATRIX_ACCEPTANCE_FILE`（默认 `dev-scripts/cross_protocol_matrix_acceptance_matrix.tsv`）。
6. 运行编号：`run_id` 与报告目录。

推荐先体检：

```bash
./dev-scripts/cross_protocol_matrix_command_templates.sh doctor
./dev-scripts/cross_protocol_matrix_regression.sh doctor
```

任一 `doctor` 失败时，先修复依赖/素材/矩阵，再进入推拉流回归。

## 2. 现象 -> 定位路径 -> 通用修复

### 2.1 首段快放（首 1 秒明显加速）

- 现象：播放刚开始帧被快速冲出，约 1 秒后恢复。
- 定位路径：
  - 检查 `summary.txt` 中 `first_second_avg_frame_interval_ms` 与 `average_playback_rate_x`。
  - 对照日志确认是否在 bootstrap 段一次性发送过多历史帧。
  - 检查 `FrameFlags::DISCONTINUITY` 后是否重建了 pacing 基准。
- 通用修复：
  - 使用 `SubscriberOptions::bootstrap_policy` 限制 `max_bootstrap_age_ms` 与 `max_bootstrap_frames`。
  - 保持首个媒体帧立即发送，后续媒体帧按统一媒体毫秒时间线 pacing。
  - 不要在 RTSP/RTMP module 内做私有“sleep 特判”。

### 2.2 `Invalid timestamps` / `Non-increasing DTS` / `Negative cts`

- 现象：`push.log` / `pull.log` 出现三类关键异常。
- 定位路径：
  - `rg -in "invalid timestamps|non-increasing dts|negative cts" <push.log> <pull.log>`。
  - 确认 ingress 是否先经过 `cheetah-codec::TimestampNormalizer`。
  - 确认 egress 是否通过 `cheetah-codec::egress` 导出时间戳，而非复用协议私有时间字段。
- 通用修复：
  - 时间修复统一落在 `cheetah-codec`（归一化/导出/告警策略）。
  - 禁止在 module 中加场景化 timestamp 补丁掩盖问题。
  - 修复后补 codec 单测与跨协议回归，不只修单一协议。

### 2.3 首帧不能秒开（无可解码关键帧）

- 现象：拉流后先见到非随机访问帧，或缺失 codec config / 参数集。
- 定位路径：
  - 检查 bootstrap 窗口是否落在最近随机访问点。
  - 检查参数集补发是否与首个可解码帧绑定。
- 通用修复：
  - 随机访问点选择与断流边界裁剪由 engine bootstrap 统一负责。
  - 参数集缓存/补发由 `cheetah-codec` 统一负责。
  - module 只做协议封装，不重写 AU/参数集逻辑。

### 2.4 RTSP TCP/UDP 表现不一致

- 现象：同一源流在 TCP/UDP 下起播时延或时间轴表现差异明显。
- 定位路径：
  - 先区分 transport 抖动（丢包/乱序）与媒体时间语义问题。
  - 校验进入引擎后的 `AVFrame + TrackInfo` 是否保持一致。
- 通用修复：
  - TCP framing / UDP 收发 / backpressure 放在 driver。
  - 媒体时间线、AU 拼装、参数集处理放在 `cheetah-codec`。
  - 禁止在 RTSP transport 分支改写媒体时间模型。

### 2.5 RTMP -> RTSP 桥接时间戳漂移

- 现象：RTSP RTP 时间戳抖动，或音视频逐步失步。
- 定位路径：
  - 确认 RTMP DTS/CTS 先归一化到 `AVFrame`。
  - 确认 RTSP egress 基于 track clock 导出 RTP 时间戳。
- 通用修复：
  - 统一走 codec egress 视图导出。
  - 不直接透传 RTMP 原始 timestamp 字段给 RTSP RTP。

## 3. 标准排查流程

1. 执行 `doctor`，确保模板、输入矩阵、验收矩阵完整。
2. 单场景执行 `run <scenario>`，拿到 `summary.txt`、`push.log`、`pull.log`。
3. 按 `stream_key/track_id/codec/pts/dts` 聚合日志，确定主故障归类。
4. 修复共享逻辑（codec/engine/sdk），避免协议分支特判。
5. 复跑单场景和 `run-all`，确认异常项清零。
6. 最后执行 Rust `fmt/clippy/test`，确认无回归。

## 4. 常用命令

```bash
# 环境体检
./dev-scripts/cross_protocol_matrix_command_templates.sh doctor
./dev-scripts/cross_protocol_matrix_regression.sh doctor

# 运行单场景
./dev-scripts/cross_protocol_matrix_regression.sh run bridge-rtsp-udp-to-rtmp

# 查看摘要
cat dev-scripts/reports/cross-protocol-matrix/<run_id>/<scenario>/summary.txt

# 统计关键异常
rg -in "invalid timestamps|non-increasing dts|negative cts|dts out of order|freeze|stutter" \
  dev-scripts/reports/cross-protocol-matrix/<run_id>/<scenario>/*.log
```

## 5. 已知未覆盖组合与原因（截至 2026-04-29）

| 组合 | 未覆盖原因 | 后续动作 |
| --- | --- | --- |
| SRT publish/play（同协议） | 本仓库当前无 `cheetah-srt-core/driver/module` crate，尚未进入实现阶段 | 新建 SRT 三段式 crate 后，接入现有矩阵脚本并补 `run-all` 场景 |
| WebRTC publish/play（同协议） | 本仓库当前无 `cheetah-webrtc-core/driver/module` crate，且缺少 RTCP 反馈联动回归脚手架 | 按 4.2 adapter 契约补齐 crate 后，引入统一 ingress/egress 契约测试与矩阵场景 |
| SRT/WebRTC 与 RTSP/RTMP 跨协议桥接 | 上游协议模块尚未落地，当前不存在可执行桥接链路 | 协议模块完成后复用现有 `bridge-*` 命名规范扩展回归矩阵 |

说明：上表为“真实未覆盖项”，不是“手工验证通过”的替代描述。

## 6. 变更验收清单

```bash
bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh
bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh
cargo fmt
cargo clippy -p cheetah-codec --all-targets -- -D warnings
cargo test -p cheetah-codec
cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings
cargo test -p cheetah-rtmp-module
cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings
cargo test -p cheetah-rtsp-module
```

若修复涉及 `AVFrame` / `TrackInfo` / 时间戳模型，必须同步更新 `SystemArchitecture.md` 与相关计划文档。
