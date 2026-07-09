# 跨协议 GOP 秒开与时间戳统一：故障排查手册（Phase 04 任务 3.3）

## 1. 适用范围

- 目标链路：`RTSP TCP/UDP`、`RTMP`、`RTSP->RTMP`、`RTMP->RTSP`。
- 目标问题：起播慢、播放卡顿、冻结、`DTS out of order`、时间戳修正过多、队列堆积丢帧。
- 执行入口：`dev-scripts/cross_protocol_matrix_regression.sh`。
- 命令模板：`dev-scripts/cross_protocol_matrix_command_templates.sh`。

## 2. 排查输入条件（必须记录）

每次排查必须先固定以下输入，避免“不可复现”：

1. 场景：`rtsp-tcp-loopback` / `rtsp-udp-loopback` / `rtmp-loopback` / `bridge-*`。
2. 输入素材：`INPUT_PROFILE`（至少注明是否 `b_frames=yes/no`）。
3. 环境参数：`CHEETAH_HOST`、`RTSP_PORT`、`RTMP_PORT`、`APP_NAME`、`STREAM_NAME`。
4. 回归参数：`SCENARIO_DURATION_SECONDS`、`PUSH_STARTUP_GRACE_MS`、`STARTUP_POLL_INTERVAL_MS`。
5. 验收矩阵版本：`MATRIX_ACCEPTANCE_FILE`（默认 `cross_protocol_matrix_acceptance_matrix.tsv`）。
6. 运行编号：`run_id` 与报告目录。

建议先执行：

```bash
./dev-scripts/cross_protocol_matrix_command_templates.sh doctor
./dev-scripts/cross_protocol_matrix_regression.sh doctor
```

若任一 `doctor` 失败，先修复环境/输入后再继续，禁止直接进入推拉流回归。

## 3. 现象 -> 定位路径 -> 修复策略

### 3.1 起播超过阈值（`startup_latency > 3000ms`）

- 现象：
  - `summary.txt` 中 `result=FAIL` 且 `startup_latency failed`。
  - RTMP/RTSP 模块日志出现首帧等待超阈值告警。
- 定位路径：
  - 检查 `pull.log` 首次输出时间与 `summary.txt` 的 `startup_latency_ms`。
  - 搜索统一字段日志：`stream_key/track_id/codec/pts/dts`。
  - 确认是否伴随 `queue_drop_count` 告警或源流不可达告警。
- 修复策略：
  - 优先修复源流可达性/鉴权/路由问题，再看阈值。
  - 若仅在 B 帧素材触发，检查 GOP 首帧门控与关键帧到达路径，不直接放宽阈值掩盖问题。
  - 若跨协议桥接独有，优先核对桥接链路时间基换算（`AVFrame.timebase -> 目标协议时间轴`）。

### 3.2 出现 `DTS out of order`

- 现象：
  - `summary.txt` 中 `dts_out_of_order > 0`。
  - `push.log`/`pull.log` 含 `dts out of order`。
- 定位路径：
  - 用下列命令分别统计：
    ```bash
    rg -in "dts out of order" <push.log>
    rg -in "dts out of order" <pull.log>
    ```
  - 检查是否同时出现时间戳修正采样日志（`repair_count` 增长）。
  - 对照协议边界：入口归一化（ingest）是否执行，出口映射（egress）是否混用不同时基。
- 修复策略：
  - 入口统一先归一化为 `AVFrame + TrackInfo`，避免协议侧私有时间轴分叉。
  - 修复时优先落在通用时间戳换算/单调修正逻辑，禁止仅对单个场景加特判。
  - 为修复补回归：至少覆盖一个 `b_frames=yes` 与一个 `b_frames=no` 素材。

### 3.3 播放冻结/卡顿（`freeze_events > 0`）

- 现象：
  - `summary.txt` 中 `freeze_events failed`。
  - `pull.log` 中出现 `freeze|stutter`（由 `FREEZE_LOG_REGEX` 统计）。
- 定位路径：
  - 检查同时间窗是否出现 `DroppedByPolicy` 告警。
  - 检查是否存在慢订阅者堆积导致的连续丢包。
  - 对 RTSP 场景，确认 TCP/UDP 是否均复现，以区分传输层抖动与编码时间轴问题。
- 修复策略：
  - 优先处理背压/队列容量/慢订阅者隔离，不把冻结问题直接等价为“提高缓冲”。
  - 保持“慢订阅者不拖累其他订阅者”，必要时在 driver/module 层补有界队列策略。
  - 修复后必须复跑 `run-all`，确认未把问题转移为 `startup_latency` 或 `dts_out_of_order`。

### 3.4 时间戳修正告警频发（`repair_count` 达阈值）

- 现象：
  - 日志出现时间戳逆序修正阈值告警（RTMP ingest、RTSP publish/play）。
- 定位路径：
  - 过滤同一 `stream_key + track_id` 的修正日志，观察 `source_dts/raw_timestamp/repaired_timestamp` 演进。
  - 判断是输入源持续逆序，还是协议映射后引入逆序。
- 修复策略：
  - 输入源异常：在入口归一化收敛，避免把脏时间戳透传到桥接出口。
  - 映射异常：修复 timebase/rate 换算与 rounding 策略，统一到共享逻辑。
  - 不允许通过简单提高 `timestamp_repair_count` 阈值来“消警”。

### 3.5 队列堆积告警频发（`queue_drop_count` 达阈值）

- 现象：
  - 日志出现队列堆积告警，且可能伴随画面不连续。
- 定位路径：
  - 按 `stream_key` 聚合告警，确认发生在 ingest、bridge 还是 play 侧。
  - 检查告警是否在成功推送后重置；若未重置，优先排查计数生命周期。
- 修复策略：
  - 优先修复队列消费能力与限流策略，保持有界并发和背压闭环。
  - 避免无上界扩大缓冲，防止延迟持续放大。

## 4. 标准排查流程（推荐）

1. `doctor`：先确认依赖、模板、输入矩阵、验收矩阵完整。
2. `run <scenario>`：先单场景复现并收集 `summary/push.log/pull.log`。
3. 日志聚合：按 `stream_key/track_id/codec/pts/dts` 过滤关键链路。
4. 判定主故障：在 `startup_latency` / `freeze_events` / `dts_out_of_order` 中确定主失败项。
5. 通用修复：优先改共享时间轴/缓冲策略，不做场景特判。
6. 回归验证：`run-all` + Rust 模块 `fmt/clippy/test`，确认无回归。

## 5. 常用排查命令

```bash
# 1) 环境体检
./dev-scripts/cross_protocol_matrix_command_templates.sh doctor
./dev-scripts/cross_protocol_matrix_regression.sh doctor

# 2) 运行单场景并定位报告目录
./dev-scripts/cross_protocol_matrix_regression.sh run bridge-rtsp-udp-to-rtmp

# 3) 查看失败摘要
cat dev-scripts/reports/cross-protocol-matrix/<run_id>/<scenario>/summary.txt

# 4) 统计关键错误
rg -in "dts out of order|freeze|stutter|DroppedByPolicy|repair_count" \
  dev-scripts/reports/cross-protocol-matrix/<run_id>/<scenario>/*.log
```

## 6. 变更验收清单

修复完成后至少执行：

```bash
bash dev-scripts/tests/cross_protocol_matrix_command_templates_test.sh
bash dev-scripts/tests/cross_protocol_matrix_regression_test.sh
cargo fmt
cargo clippy -p cheetah-rtmp-module
cargo clippy -p cheetah-rtsp-module
cargo test -p cheetah-rtmp-module
cargo test -p cheetah-rtsp-module
```

若修复涉及 `cheetah-codec`、公共时间戳模型或桥接主路径，追加跨协议集成回归并在计划文档记录。
