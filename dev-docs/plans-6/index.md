# RTSP 真实抓包测试数据与鲁棒性计划总索引

- 状态：计划中
- 目标：基于 `test_media_files/dump_rtsp_sms_gst` 中 SimpleMediaServer + GStreamer RTSP 推拉流抓包，补齐 `cheetah-rtsp-core`、`cheetah-rtsp-module`、`cheetah-rtsp-pbt` 和 `cheetah-rtsp-fuzz` 的真实协议样例、传输扰动和鲁棒性覆盖。
- 方法：不直接在常规测试中读取被 `.gitignore` 忽略的原始 `test_media_files`；从非空 pcap 抽取小型、可提交的 RTSP 控制面、TCP interleaved、UDP RTP/RTCP fixture。标准样例做强行为断言，probe 和损坏样例做 bounded robustness 断言。
- 完成判定：H264/H265/audio-only 标准样例能在 core/pbt/module 中稳定重放；AV1/VP8/VP9/H266/4K/high-bitrate 样例至少覆盖解析和健壮性路径；新增 fuzz target 覆盖真实 RTSP 抓包、UDP 丢包、UDP 乱序、TCP 粘包、TCP 半包、RTP 乱序、RTP 重复和截断，且 fuzz smoke 无 crash、无 OOM、无超时。

## 当前抓包事实

本地 `test_media_files/dump_rtsp_sms_gst` 的关键事实：

- `summary.tsv` 中基础短名矩阵如 `h264_aac__push_tcp__pull_tcp.pcap` 当前多为 0 字节，不作为首批 fixture 来源。
- `summary_from_files.tsv` 对应的 `from_file_*` 系列存在 69 个非空 pcap，总量约 344 MiB，可作为首批真实来源。
- 非空 pcap 为 pcap v2.4 little-endian，linktype 为 Linux cooked v2，抓包长度 262144。
- TCP 控制面固定围绕 `127.0.0.1:8554`；TCP interleaved 推流样例中 publisher C2S flow 包含 `OPTIONS -> ANNOUNCE -> SETUP -> RECORD` 后的 `$` RTP/RTCP interleaved bytes。
- UDP 推流样例中控制面仍走 TCP，RTP/RTCP 走 UDP datagram；同一 pcap 会同时包含 publish 侧和 pull 侧 UDP flow，需要按 RTSP SETUP 响应里的端口和 flow 方向归类。

## 总体约束

- RTSP core 保持 Sans-I/O；pcap 解析、fixture 生成、端口归类和重放 harness 只能出现在测试/工具代码中。
- 测试不能依赖 CI 中存在 `test_media_files`；原始 pcap 只作为本地生成来源。
- fixture 必须有上界，单个可提交样例默认不超过 512 KiB；超限只能按完整 record/datagram 前缀截取，不能截断标准 fixture 的 record payload。
- 0 字节 pcap、无 8554 RTSP 控制面、无法解析 linktype、无法归类 RTP/RTCP flow 的输入只记录 skipped，不生成 fixture。
- 标准 RTSP 样例必须断言 OPTIONS、ANNOUNCE、SETUP、RECORD、DESCRIBE、PLAY 等具体控制面行为；标准 RTP 样例必须至少解析到 RTP header，并校验同一 SSRC sequence/timestamp 基本单调。
- probe、截断、乱序、丢包、重复样例不得要求成功播放，只要求不 panic、不越界、不无限循环、module 不异常退出。
- 兼容处理应优先沉淀到 `cheetah-codec` 或明确的 RTSP media 兼容层；不要在 module 热路径临时复制媒体时间戳、NALU、参数集或 RTP depacketize 修复逻辑。

## 计划文件清单

| 文件 | 状态 | 范围 |
| --- | --- | --- |
| `rtsp-capture-fixture-architecture.md` | 计划中 | 真实抓包 fixture 格式、生成工具、manifest、样例选择和断言分层 |
| `rtsp-capture-phase-01-testdata.md` | 计划中 | `cheetah-rtsp-pbt/tests/testdata` 数据落地、抽取工具和校验测试 |
| `rtsp-capture-phase-02-core-and-pbt-tests.md` | 计划中 | `cheetah-rtsp-core/src/core/tests` 与 `cheetah-rtsp-pbt/tests` 的真实抓包回归和属性测试 |
| `rtsp-capture-phase-03-module-integration.md` | 计划中 | `cheetah-rtsp-module/tests` 的真实 publish/play replay、UDP/TCP 集成和健康度回归 |
| `rtsp-capture-phase-04-fuzz-real-transport.md` | 计划中 | `cheetah-rtsp-fuzz/fuzz_targets` 的真实抓包和传输异常 fuzz target |

## 任务状态总表

| 阶段 | 任务 | 状态 | 计划文件 |
| --- | --- | --- | --- |
| Architecture | A.1 固定 RTSP capture fixture 格式 | 已完成 | `rtsp-capture-fixture-architecture.md` |
| Architecture | A.2 固定 pcap 样例集和跳过规则 | 已完成 | `rtsp-capture-fixture-architecture.md` |
| Architecture | A.3 固定标准/probe/fault 断言分层 | 已完成 | `rtsp-capture-fixture-architecture.md` |
| Phase 01 | 1.1 新增 RTSP capture fixture 目录 | 已完成 | `rtsp-capture-phase-01-testdata.md` |
| Phase 01 | 1.2 新增 pcap 抽取工具和 manifest 校验 | 已完成 | `rtsp-capture-phase-01-testdata.md` |
| Phase 01 | 1.3 生成首批标准与 probe fixture | 已完成 | `rtsp-capture-phase-01-testdata.md` |
| Phase 02 | 2.1 core RTSP 控制面 replay 回归 | 已完成 | `rtsp-capture-phase-02-core-and-pbt-tests.md` |
| Phase 02 | 2.2 core RTP/RTCP/interleaved 鲁棒性回归 | 已完成 | `rtsp-capture-phase-02-core-and-pbt-tests.md` |
| Phase 02 | 2.3 pbt 传输扰动属性测试 | 已完成 | `rtsp-capture-phase-02-core-and-pbt-tests.md` |
| Phase 03 | 3.1 module TCP interleaved publish replay | 已完成 | `rtsp-capture-phase-03-module-integration.md` |
| Phase 03 | 3.2 module UDP publish/play replay | 已完成 | `rtsp-capture-phase-03-module-integration.md` |
| Phase 03 | 3.3 probe/fault module 健康度回归 | 已完成 | `rtsp-capture-phase-03-module-integration.md` |
| Phase 04 | 4.1 新增真实 RTSP capture fuzz | 已完成 | `rtsp-capture-phase-04-fuzz-real-transport.md` |
| Phase 04 | 4.2 新增 UDP/TCP/RTP fault fuzz | 已完成 | `rtsp-capture-phase-04-fuzz-real-transport.md` |
| Phase 04 | 4.3 fuzz smoke 和 corpus seed 收口 | 已完成 | `rtsp-capture-phase-04-fuzz-real-transport.md` |

## 渐进式执行顺序

1. 先完成 Architecture，固定 fixture 格式、样例选择、断言分层和 CI 边界。
2. 再完成 Phase 01，把可提交测试数据、manifest、生成工具和校验测试落地。
3. 再完成 Phase 02，在 Sans-I/O core 与 PBT 层锁定真实控制面、RTP/RTCP 和传输扰动鲁棒性。
4. 再完成 Phase 03，在 module 层验证真实 RTSP publish/play replay 到 engine 的集成路径。
5. 最后完成 Phase 04，把真实样例作为 fuzz seed，扩展 UDP/TCP/RTP 异常 target 并跑 smoke。

## 阶段完成后的统一检查

```bash
cargo fmt
cargo clippy -p cheetah-rtsp-core
cargo test -p cheetah-rtsp-core capture
cargo clippy -p cheetah-rtsp-pbt
cargo test -p cheetah-rtsp-pbt
cargo clippy -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-module
cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml --bins
cargo clippy --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml --bins
```

新增 fuzz target 后还必须执行短跑 smoke：

```bash
cd crates/cheetah-rtsp-fuzz
cargo +nightly fuzz build
cargo +nightly fuzz run fuzz_real_capture_rtsp_tcp_replay -- -runs=128
cargo +nightly fuzz run fuzz_real_capture_udp_datagrams -- -runs=128
cargo +nightly fuzz run fuzz_real_capture_mixed_transport -- -runs=128
cargo +nightly fuzz run fuzz_rtp_sequence_faults -- -runs=128
```
