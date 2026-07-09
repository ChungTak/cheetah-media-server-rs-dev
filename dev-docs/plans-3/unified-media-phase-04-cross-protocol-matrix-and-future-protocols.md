# Phase 04: 全矩阵回归与未来协议扩展

- 状态：已完成
- 范围：RTSP/RTMP 全组合互操作矩阵、音视频 codec 矩阵、自动化回归、SRT/WebRTC 扩展契约。
- 完成标准：所有协议组合和 codec 组合都有明确验证路径，新增 SRT/WebRTC 时只需实现协议 adapter，不需要复制媒体时间、AU 或参数集逻辑。

## 具体任务

### 4.1 全协议全 codec 矩阵

- [x] RTMP publish -> RTMP play。
- [x] RTSP TCP publish -> RTSP TCP play。
- [x] RTSP UDP publish -> RTSP UDP play。
- [x] RTSP TCP publish -> RTMP play。
- [x] RTSP UDP publish -> RTMP play。
- [x] RTMP publish -> RTSP TCP play。
- [x] RTMP publish -> RTSP UDP play。
- [x] 覆盖 H264、H265、H266、AV1、VP8、VP9、AAC、Opus、G711A、G711U、MP3。

### 4.2 SRT/WebRTC 扩展契约

- [x] 定义未来协议 ingress adapter 必须输出的统一字段：track、codec、timebase、PTS/DTS、duration、random-access、discontinuity。
- [x] 定义未来协议 egress adapter 必须消费的统一导出视图：封装时间戳、分片边界、codec config、参数集补发。
- [x] SRT 作为传输协议时不允许绕过媒体归一化。
- [x] WebRTC RTP/RTCP 相关逻辑只保留协议状态和网络反馈，媒体 AU 与时间线仍由 `cheetah-codec` 提供。

### 4.3 自动化回归与报告

- [x] 为协议组合定义可重复执行的测试命令和输入媒体样例。
- [x] 自动扫描 ffplay/ffmpeg debug 日志中的 `Invalid timestamps`、`Non-increasing DTS`、`Negative cts`。
- [x] 输出每个组合的首帧时间、首 1 秒帧间隔、平均播放速率、异常日志摘要。
- [x] 将失败样例保存为回归输入，优先补到 codec 单元测试或 module 互操作测试。

### 4.4 排障手册和索引收口

- [x] 将真实问题的定位路径补入 `unified-media-troubleshooting-manual.md`。
- [x] 每完成一个任务，同步更新本目录 `index.md` 的任务状态。
- [x] 记录未覆盖组合和原因，不能用空泛“通过手工验证”代替矩阵结果。
- [x] 如果改变 `AVFrame` / `TrackInfo` / 时间戳模型，同步更新 `SystemArchitecture.md` 或相关说明文档。

## 未覆盖组合与原因（截至 2026-04-29）

| 组合 | 未覆盖原因 | 后续动作 |
| --- | --- | --- |
| SRT publish/play（同协议） | 仓库尚无 `cheetah-srt-core/driver/module` crate | 新建 SRT 三段式 crate 后接入矩阵脚本 |
| WebRTC publish/play（同协议） | 仓库尚无 `cheetah-webrtc-core/driver/module` crate | 完成 WebRTC 协议三段式与 adapter 接入后补矩阵 |
| SRT/WebRTC 与 RTSP/RTMP 跨协议桥接 | 上游协议模块尚未实现，当前无可执行桥接链路 | 新协议模块落地后复用 `bridge-*` 场景扩展回归 |

说明：以上为当前真实未覆盖项，未使用“手工验证通过”替代矩阵结果。

## 最新进展

- 2026-04-29：完成任务 4.4（排障手册和索引收口）。将 `unified-media-troubleshooting-manual.md` 从初版提纲升级为标准排查手册，新增“排查输入条件、现象->定位路径->通用修复、标准流程、命令清单、未覆盖组合与原因”；同步更新 `index.md` 与本文件状态为已完成。核对本次无 `AVFrame` / `TrackInfo` / 时间戳模型变更，无需修改 `SystemArchitecture.md`。执行并通过 `cargo fmt`、`bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/cross_protocol_matrix_regression.sh doctor`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成任务 4.3（自动化回归与报告）。复用并增强 `dev-scripts/cross_protocol_matrix_command_templates.sh` 与 `dev-scripts/cross_protocol_matrix_regression.sh`：场景模板统一输出 `ffmpeg/ffplay -v debug -debug_ts -stats` 可重复命令，固定输入媒体样例矩阵与验收矩阵；回归脚本新增 `Invalid timestamps`、`Non-increasing DTS`、`Negative cts` 三类异常日志自动扫描，并将 `dts_out_of_order`、上述三类异常按 `push/pull` 维度输出到 `anomaly_summary.txt`；新增每个组合 `summary.txt` 指标字段 `startup_latency_ms`、`first_second_avg_frame_interval_ms`、`average_playback_rate_x`、`media_span_seconds` 与异常摘要路径；失败场景新增 `failure_input.txt`（记录场景、矩阵文件、推拉命令）用于后续回归补测。同步扩展脚本测试 `dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`dev-scripts/tests/cross_protocol_matrix_regression_test.sh` 覆盖新验收项与失败路径。执行并通过 `bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh`、`bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh`、`bash dev-scripts/cross_protocol_matrix_regression.sh doctor`、`cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成任务 4.2（SRT/WebRTC 扩展契约）。`cheetah-codec` 新增 `adapter` 契约模块：`IngressAdapterFrame` 统一入口字段（`track/codec/timebase/pts/dts/duration/random_access/discontinuity`）并支持 `TimestampNormalizeOutput` 对齐校验；`EgressAdapterView` 统一出口导出视图（封装时间戳、AU 分片边界、`codec_config_view`、参数集补发视图）；新增 `AdapterContractError` 统一错误处理；新增 `enforce_future_protocol_ingress/egress` 对 SRT 与 WebRTC 扩展约束进行显式校验（SRT 禁止绕过归一化，WebRTC 视频要求 AU 边界标记）。补充 `cheetah-codec/tests/future_protocol_adapter_contract.rs` 回归覆盖契约字段与错误路径。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-codec --all-targets -- -D warnings`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：完成任务 4.1（全协议全 codec 矩阵）。新增 `cheetah-rtmp-module/tests/rtmp_publish_play_matrix.rs` 覆盖 RTMP publish -> RTMP play 时间戳单调与 100ms 步进回归；已有互操作测试 `bridge_rtsp_rtmp`、`bridge_rtmp_rtsp` 覆盖 RTSP TCP/UDP 与 RTMP 双向桥接；已有 RTSP 同协议 `play_pause`/`udp_forwarding` 回归覆盖 RTSP TCP/UDP publish->play；补齐编码矩阵缺口：`cheetah-rtmp-module` 的 RTSP->RTMP 时间归一化矩阵新增 H266 视频与 G711U 音频，`cheetah-rtsp-module` 的 raw RTP 时间戳保留矩阵新增 G711U。执行并通过 `cargo fmt`、`cargo clippy -p cheetah-rtmp-module --all-targets -- -D warnings`、`cargo clippy -p cheetah-rtsp-module --all-targets -- -D warnings`、`cargo test -p cheetah-rtmp-module`、`cargo test -p cheetah-rtsp-module`。
- 2026-04-29：计划已创建，任务未开始。

## 完成后检查

- `cargo fmt`
- `cargo clippy -p cheetah-codec`
- `cargo test -p cheetah-codec`
- `cargo test -p cheetah-rtmp-module`
- `cargo test -p cheetah-rtsp-module`
- 运行 RTSP/RTMP 全组合互操作矩阵。
- 汇总所有失败日志，确认无时间戳类异常。
