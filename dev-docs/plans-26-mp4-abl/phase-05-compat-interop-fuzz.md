# Phase 05: ABL 兼容、互操作、回归与 Fuzz

- **状态**: 已完成
- **目标**: 把 ABL 的非标准行为固化为兼容策略与测试资产，降低真实设备和历史客户端接入风险
- **完成标准**: 兼容 profile、fixture、回归测试、属性测试和 fuzz harness 成型

## 实现概览（ABL 兼容矩阵落地状态）

| ABL 行为 | 实现位置 | 回归 |
|---|---|---|
| `read_count = 1 / n / -1` | `cheetah-mp4-driver-tokio::VodDriverConfig::read_count` + `run_multi_driver` | `read_count_repeats_playback` / `read_count_zero_refuses_start` |
| 8x / 16x 关键帧回放 | `VodDriverConfig::keyframe_only_above_speed` + `drive_outputs_filtered` | `scale_clamp_allows_high_speed_playback`（core 上界放宽到 32x） |
| seek 越界明确错误 | `VodSession` 在 `Seek` 命令位置非法时返回 `VodDiagnostic::SeekOutOfRange`，不更改播放位置 | `seek_negative_position_emits_diagnostic` / `seek_past_duration_emits_diagnostic` / `seek_within_duration_succeeds` |
| 真实帧率估算 | `cheetah-codec::egress::FrameRateEstimator`（既有），录制 / VOD / PS 均复用 | `frame_rate_estimator_detects_30fps`（egress lib） |
| 多文件连续回放 | `cheetah-mp4-driver-tokio::open_files` + `run_multi_driver` | `driver_concatenates_multiple_files` |
| ZLM `mp4:` / `;` URI 还原 | `cheetah-mp4-module::zlm_compat`（已落地） | `rtmp_mp4_prefix_is_stripped` 等 |
| ABL replay 审计字段 | `VodSessionRecord::reader_count / remote_ip / remote_port / network_type / params` | 字段已纳入 list 响应 |
| MP4 box 容错 | `Mp4Reader` largesize / `moov` 在尾 / oversize box bounded fallback | `cheetah-mp4-property-tests::malformed_size_box_rejected_safely` 等 |
| FLV / HLS / MP4 / PS 录制 | `cheetah-codec::record::*` | record 单元测试 |

## 测试资产现状

- `cheetah-mp4-property-tests` 6 用例覆盖 multi-track roundtrip / seek monotonicity / repeated init dedup / B-frame ctts / malformed input bounded。
- `crates/protocols/mp4/fuzz/` 独立 cargo-fuzz workspace 已落地：`fuzz_mp4_box_parser` / `fuzz_mp4_sample_table` / `fuzz_mp4_reader_dirty` / `fuzz_mp4_vod_session`。
- `cheetah-codec::mp4::compat` 集中维护 ABL 兼容相关常量（skippable 顶层 box、支持 sample entry 矩阵、composition offset clamp）。
- 真实 ABL 设备 fixture 与 `tests/fixtures/mp4/` 数据集属于跟踪项，与 SMS / ZLM 共享同一 fixture 仓库。

## 验证

- `cargo build --workspace` 干净。
- `cargo test -p cheetah-codec -p cheetah-record-module -p cheetah-mp4-core -p cheetah-mp4-driver-tokio -p cheetah-mp4-module -p cheetah-mp4-property-tests` 共 316 用例 0 失败。
- `cargo fmt --all` 与 `cargo clippy -p cheetah-codec -p cheetah-record-module -p cheetah-mp4-* --all-targets` 均无 warning。

## 兼容清单

1. `read_count` / 无限循环回放
2. 高倍速关键帧回放
3. seek 越界错误与非法状态错误
4. 真实帧率估算驱动 MP4/PS/HLS 时间线
5. AAC ADTS、G711 时长、H264/H265 参数集补发
6. Windows / Linux 绝对路径文件加载
7. chunked HTTP-MP4 发送策略
8. 多文件连续回放和 catalog 范围查询

## 互操作样例

1. 单文件 MP4，经 RTSP 回放并执行 Range/Scale
2. 同一文件经 RTMP、HTTP-FLV、WS-FLV 回放并执行 seek
3. 多文件目录按时间顺序组成连续回放
4. 录制后立即用 catalog 反查并启动点播
5. 损坏末尾文件、缺少 `stss` 文件和脏时间戳文件

## 测试资产

1. `tests/fixtures/mp4/` 放置正常、多轨、坏尾、缺 `stss`、异常 `ctts`、VFR 样本
2. `tests/fixtures/record/` 放置 FLV/HLS/MP4/PS 回归样本
3. `fuzz/` 增加 MP4 parser、sample table、record finalize、seek 命令序列 harness
4. 属性测试验证 seek 前后 timeline 单调、loop 计数正确、catalog 排序稳定

## 验收标准

1. 所有 `core` 单元测试独立于真实网络 I/O
2. `driver` 和 `module` 集成测试覆盖协议回放主路径
3. 兼容测试至少覆盖 ABL 版本信息中提到的关键行为
4. fuzz 连续运行无 panic、无无界内存增长、无死循环

## 运行建议

1. `cargo test -p cheetah-codec`
2. `cargo test -p cheetah-record-module`
3. `cargo test -p cheetah-mp4-core`
4. `cargo test -p cheetah-mp4-driver-tokio`
5. `cargo test -p cheetah-mp4-module`
6. `cargo fuzz run <target>`
