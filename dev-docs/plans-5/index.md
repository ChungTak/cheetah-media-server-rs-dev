# RTMP 真实抓包测试数据与鲁棒性计划总索引

- 状态：已完成
- 目标：基于 `test_media_files/dump_rtmp_sms_gst` 中可解析的 RTMP 抓包，补齐 `cheetah-rtmp-core`、`cheetah-rtmp-module`、`cheetah-rtmp-pbt` 和 `cheetah-rtmp-fuzz` 的真实协议样例、非标准输入鲁棒性和传输扰动覆盖。
- 方法：不直接依赖被 `.gitignore` 忽略的原始 `test_media_files`，而是从非空 pcap 抽取小型、可提交的 RTMP TCP payload fixture；标准样例做强断言，非标准和损坏样例做 bounded robustness 断言。
- 完成判定：标准 H264/AAC、H265/AAC、audio-only 抓包样例可在 core/module/pbt 中稳定重放；AV1/VP8/VP9/H266/enhanced/fallback 样例至少覆盖解析和健壮性路径；新增 fuzz target 覆盖真实抓包、TCP 粘包、半包、截断、重复、乱序和 datagram-like 丢片，且 fuzz smoke 无 crash、无 OOM、无超时。

## 总体约束

- RTMP core 仍保持 Sans-I/O；pcap 解析和 fixture 生成只存在测试/工具代码，不进入 `cheetah-rtmp-core` 公共 API。
- 测试不能直接依赖 `test_media_files` 在 CI 中存在；原始 pcap 只作为本地生成来源。
- fixture 必须有上界，单个可提交样例默认不超过 256 KiB，避免把大媒体流塞进仓库。
- 0 字节或 tcpdump 无法解析的 pcap 只记录在 manifest 备注，不作为重放输入。
- 标准协议样例必须断言 `Connected`、`PublishRequested`、`MediaData`、timestamp 单调等具体行为。
- 非标准、截断、乱序、丢片样例不得要求成功播放，只要求不 panic、不越界、不无限循环、module 不异常退出。
- “udp 丢包”在 RTMP fuzz 中按输入扰动建模：把真实 RTMP byte stream 切成 datagram-like 片段后做丢片、重复、乱序、截断，不新增真实 UDP 协议栈。
- 兼容处理应优先沉淀到 `cheetah-codec` 或明确测试 compat 层；不要在 module 热路径临时复制媒体时间戳、NALU 或参数集逻辑。

## 计划文件清单

| 文件 | 状态 | 范围 |
| --- | --- | --- |
| `rtmp-capture-fixture-architecture.md` | 已完成 | 真实抓包 fixture 格式、生成工具、manifest 和样例选择策略 |
| `rtmp-capture-phase-01-testdata.md` | 已完成 | `cheetah-rtmp-pbt/tests/testdata` 数据落地与生成/校验测试 |
| `rtmp-capture-phase-02-core-and-pbt-tests.md` | 已完成 | `cheetah-rtmp-core/src/core/tests` 与 `cheetah-rtmp-pbt/tests` 的抓包重放和传输扰动属性测试 |
| `rtmp-capture-phase-03-module-integration.md` | 已完成 | `cheetah-rtmp-module/tests` 的真实 publish replay、play 验收和 module 健康度回归 |
| `rtmp-capture-phase-04-fuzz-real-transport.md` | 已完成 | `cheetah-rtmp-fuzz/fuzz_targets` 的真实抓包和传输异常 fuzz target |

## 任务完成状态总表

| 阶段 | 任务 | 状态 | 计划文件 |
| --- | --- | --- | --- |
| Architecture | A.1 明确 fixture 格式与边界 | 已完成 | `rtmp-capture-fixture-architecture.md` |
| Architecture | A.2 明确可解析 pcap 样例集 | 已完成 | `rtmp-capture-fixture-architecture.md` |
| Architecture | A.3 明确标准/非标准断言分层 | 已完成 | `rtmp-capture-fixture-architecture.md` |
| Phase 01 | 1.1 新增 RTMP capture fixture 目录 | 已完成 | `rtmp-capture-phase-01-testdata.md` |
| Phase 01 | 1.2 新增 pcap 抽取工具与 manifest 校验 | 已完成 | `rtmp-capture-phase-01-testdata.md` |
| Phase 01 | 1.3 生成首批标准与非标准样例 | 已完成 | `rtmp-capture-phase-01-testdata.md` |
| Phase 02 | 2.1 core server replay 回归 | 已完成 | `rtmp-capture-phase-02-core-and-pbt-tests.md` |
| Phase 02 | 2.2 core 分片/粘包/截断鲁棒性回归 | 已完成 | `rtmp-capture-phase-02-core-and-pbt-tests.md` |
| Phase 02 | 2.3 pbt 传输扰动属性测试 | 已完成 | `rtmp-capture-phase-02-core-and-pbt-tests.md` |
| Phase 03 | 3.1 module raw TCP publish replay | 已完成 | `rtmp-capture-phase-03-module-integration.md` |
| Phase 03 | 3.2 RTMP play 验收与 timestamp 断言 | 已完成 | `rtmp-capture-phase-03-module-integration.md` |
| Phase 03 | 3.3 非标准样例 module 健康度回归 | 已完成 | `rtmp-capture-phase-03-module-integration.md` |
| Phase 04 | 4.1 新增真实抓包 server/client fuzz | 已完成 | `rtmp-capture-phase-04-fuzz-real-transport.md` |
| Phase 04 | 4.2 新增 transport faults fuzz | 已完成 | `rtmp-capture-phase-04-fuzz-real-transport.md` |
| Phase 04 | 4.3 fuzz smoke 和 corpus seed 收口 | 已完成 | `rtmp-capture-phase-04-fuzz-real-transport.md` |

## 最新进展

- 2026-05-05：完成 Phase 04 / 4.3。`cheetah-rtmp-fuzz/corpus/` 新增可提交标准抓包短前缀 seed：三个 target 各三份 `seed_standard_{h264,h265,audio}_prefix.rtmpflow`（从标准 fixture 提取前 40 records，保留 CRF1 格式），覆盖 handshake/connect/publish/media 的真实输入路径；`common.rs` 新增 `capture_records_from_data_or_seed`，优先把 fuzz 输入按 `.rtmpflow` 解码，失败时回退内置 fixture selector；`fuzz_transport_faults` 同步到相同输入策略。根 `.gitignore` 与 `crates/cheetah-rtmp-fuzz/.gitignore` 已改为只放行这些 seed，继续忽略 fuzz 运行时自动生成 corpus。验证已执行：`cargo fmt`、`cargo check --manifest-path crates/cheetah-rtmp-fuzz/Cargo.toml --bins`、`cargo clippy --manifest-path crates/cheetah-rtmp-fuzz/Cargo.toml --bins`、三个新增 target 的 `cargo +nightly fuzz build --fuzz-dir crates/cheetah-rtmp-fuzz ...`、三个新增 target 各 `-runs=128`、`cargo test --workspace`，未发现 crash 与回归。
- 2026-05-03：完成 Phase 04 / 4.1 和 4.2。`cheetah-rtmp-fuzz` 新增三个真实传输 fuzz target：server replay 使用真实 C2S `.rtmpflow` fixture 并在 `PublishRequested` 后注入 `AcceptPublish`；client post-handshake target 使用 `RtmpCore::new_client()`，只喂由真实 server replay 派生出的 post-handshake S2C chunk；transport faults target 覆盖 single buffer、原始 TCP record、逐字节、coalesced N、prefix 截断、重复、相邻乱序和 datagram-like 每 N 片丢弃。`common.rs` 内置 8 个 capture seed、CRF1 解码和 bounded feed helper；4.3 的 build/smoke 子项已完成，但标准 fixture 短前缀 corpus seed 仍待收口。验证已执行：`cargo fmt`、`cargo check --manifest-path crates/cheetah-rtmp-fuzz/Cargo.toml --bins`、`cargo clippy --manifest-path crates/cheetah-rtmp-fuzz/Cargo.toml --bins`、三个新增 target 的 `cargo +nightly fuzz build --fuzz-dir crates/cheetah-rtmp-fuzz ...`、三个新增 target 各 `-runs=128`、`cargo test --workspace`。
- 2026-05-03：完成 Phase 03 / 3.3。扩展 module capture fixture helper，新增 `probe_publish_cases()` 覆盖 `av1_probe`、`vp8_probe`、`vp9_probe`、`h266_probe`；新增 `CaptureFaultKind` 和 fault chunk 构造，覆盖 prefix 截断、每 5 个 post-handshake record 丢弃、相邻 record 乱序。`rtmp_capture_replay.rs` 新增 `probe_capture_raw_replay_keeps_module_running` 和 `capture_transport_faults_keep_module_and_engine_healthy`：probe raw replay 只要求 rtmp module 仍为 `Running`，扰动视图只要求 engine `is_live()/is_ready()` 仍为 true，连接被关闭或写入失败作为可接受输入终止，不在 module 热路径增加 codec 修复逻辑。Phase 03 已全部完成。验证已执行：`cargo fmt`、`cargo clippy -p cheetah-rtmp-module --tests`、`cargo test -p cheetah-rtmp-module --test rtmp_capture_replay`、`cargo test -p cheetah-rtmp-module --test rtmp_publish_play_matrix`、`cargo test -p cheetah-rtmp-module --test rtmp_module_push_job_resilience`、`cargo test --workspace`。
- 2026-05-03：完成 Phase 03 / 3.2。扩展 `rtmp_capture_replay.rs`，新增 `standard_capture_raw_publish_can_be_played_with_monotonic_timestamps`：对 `h264_aac_publish`、`h265_aac_publish` 和 `audio_only_publish`，先用 raw TCP 写入真实 publish 前缀使 tracks ready，再用 engine `StreamSnapshot.key` 构建真实 RTMP play URL，等待 play client 进入 `Playing` 后继续写完剩余抓包 payload。新增 `RawPublishSession` 和 play helper，覆盖 H264/H265 至少一个 video media、audio-only 至少一个 audio media，收集音视频 timestamp 并断言单调；等待 `Playing` 前如果收到 media 会失败，保证 coded frame 不早于 play state。验证已执行：`cargo fmt`、`cargo clippy -p cheetah-rtmp-module --tests`、`cargo test -p cheetah-rtmp-module --test rtmp_capture_replay`、`cargo test -p cheetah-rtmp-module --test rtmp_publish_play_matrix`、`cargo test -p cheetah-rtmp-module --test rtmp_module_push_job_resilience`、`cargo test --workspace`。
- 2026-05-03：完成 Phase 03 / 3.1。新增 `crates/cheetah-rtmp-module/tests/rtmp_capture_replay.rs`、`tests/support/capture_fixture.rs` 和 `tests/support/rtmp_test_harness.rs`，module 集成测试直接 `include_bytes!` 读取已提交标准 `.rtmpflow`，不解析 pcap，不改 module 公共接口。测试 harness 动态保留 `127.0.0.1:0`，启动 `EngineBuilder` 并注册 `RtmpModuleFactory`，使用 raw `tokio::net::TcpStream` 回放真实 publish C2S payload；同时启动读半连接 drain 服务端响应，测试结束关闭 TCP 写半连接、等待读任务、停止 engine，并确认 rtmp module 进入 `Stopped` 且 health 下线。4 个标准样例覆盖原始 record 边界发送，另覆盖保留握手/控制面边界并合并相邻 post-control payload 的 TCP 粘包模式；断言 engine `StreamSnapshot` 出现 active publisher 且 tracks 非空。
- 2026-05-03：完成 Phase 02 / 2.3。新增 `crates/cheetah-rtmp-pbt/tests/prop_rtmp_capture_transport.rs`，默认 `ProptestConfig::with_cases(64)`，从 committed manifest fixture 集合中随机选择 case、视图、chunk size、截断点、重复次数和丢包步长。`tests/support/capture_fixture.rs` 现在提供 fixture 加载、`.rtmpflow` record 解码复用、transport view 构造：pristine records、按字节 chunk、coalesced pairs、prefix 截断、suffix 半 record 截断、重复 record、相邻乱序、每 N 个 record 丢弃。standard pristine 视图保留强断言，扰动/probe 视图只验证 bounded robustness；成功 replay 仍要求已发出的同类媒体 timestamp 单调。
- 2026-05-03：完成 Phase 02 / 2.2。扩展 core capture 测试，所有 standard/probe `.rtmpflow` 均覆盖 bounded robustness：每两个 record 合并、前 1/2 输入、前 3/4 输入、最后 record 半截断、重复第一个 post-handshake record、交换相邻 post-handshake records、每 5 个 post-handshake record 丢弃一个。鲁棒性测试不要求成功事件，不匹配 `RtmpCoreError` 文本；`handle_input` 返回 `Err` 作为可接受终止，panic 或无界输入视图才失败。
- 2026-05-03：完成 Phase 02 / 2.1。新增 `crates/cheetah-rtmp-core/src/core/tests/capture.rs` 并在 `core/tests.rs` 挂载；core 测试直接 `include_bytes!` 消费已提交 `.rtmpflow`，不依赖 pcap parser、runtime 或 socket。4 个标准 publish fixture 均覆盖原始 TCP record 边界、合并单 buffer、逐字节输入三种视图；replay 到 `RtmpCore::new()` 后收集事件，遇到 `PublishRequested` 立即注入 `RtmpCoreCommand::AcceptPublish`，断言 `Connected`、`PublishRequested`、至少一个 `MediaData`，并校验同一 media type timestamp 单调非递减。
- 2026-05-03：完成 Phase 01 / 1.3。已生成首批 8 个 `.rtmpflow` fixture 并写入 manifest：`standard/h264_aac_publish.rtmpflow`、`standard/h265_aac_publish.rtmpflow`、`standard/h265_large_publish.rtmpflow`、`standard/audio_only_publish.rtmpflow` 作为标准 publish 样例，`probes/av1_probe.rtmpflow`、`probes/vp8_probe.rtmpflow`、`probes/vp9_probe.rtmpflow`、`probes/h266_probe.rtmpflow` 作为 enhanced/fallback/compat probe。标准样例设置 `expect_connected=1`、`expect_publish=1`、`expect_media_min=1`；probe 设置 `expect_media_min=0` 且 notes 明确为 probe。基础短名 `av1_aac.pcap`、`vp8_aac.pcap`、`vp9_aac.pcap`、`h266_aac.pcap` 当前为空，probe fixture 改用同源非空 `from_file_*` 抓包生成，README 已记录该 CI 边界。
- 2026-05-03：完成 Phase 01 / 1.2。新增 `dev-scripts/rtmp_extract_capture_fixtures.py`，使用 Python 标准库解析 pcap global header、packet header、Linux cooked v2/Ethernet、IPv4 和 TCP payload，按 TCP flow 聚合并优先选择 `dport == 1935` 的最大 C2S publish flow；同时提供 play C2S 与 server S2C flow 选择函数供后续 fuzz 使用。新增 `capture_fixture_manifest.rs` 与 `tests/support/capture_fixture.rs`，校验 manifest 表头/字段、role/flag/number、路径安全、fixture 存在性、大小上限、`CRF1` magic、record 长度、截断和尾随字节；`RTMP_CAPTURE_FIXTURE_DIR=/tmp/cheetah-rtmp-capture-fixtures cargo test -p cheetah-rtmp-pbt --test capture_fixture_manifest` 可验证本地生成输出。
- 2026-05-03：完成 Phase 01 / 1.1。新增 `crates/cheetah-rtmp-pbt/tests/testdata/rtmp-capture/` 骨架：`README.md` 说明 fixture 来源、`.rtmpflow` 格式、manifest 字段、断言分层和再生成命令；`manifest.tsv` 固定表头；`standard/` 与 `probes/` 子目录已创建，用于后续区分标准样例和兼容/鲁棒性 probe。
- 2026-05-03：完成 Architecture / A.3。断言分层已固定：标准 fixture 在 core/module/pbt 中做强行为断言，probe fixture 在 core/pbt/fuzz 中做鲁棒性断言，扰动视图统一由 helper 构造并只验证 bounded processing；module probe 只要求 rtmp module 与 engine health 保持正常，不把 codec 兼容缺口临时补在 module 热路径。
- 2026-05-03：完成 Architecture / A.2。已固定可解析 pcap 样例集：当前非空候选均为 Linux cooked v2（linktype 276），按 `sport/dport == 1935` 聚合 TCP payload flow；标准样例优先选 H264/AAC、H265/AAC、H265 大 payload、audio-only，AV1/VP8/VP9/H266/VVC 作为 probe；生成工具必须跳过 0 字节 pcap、拒绝非法 global header/未知 linktype/无 1935 payload flow 的输入，并在 manifest 中记录来源、stream name、媒体签名、角色和期望事件。
- 2026-05-03：完成 Architecture / A.1。固定 `.rtmpflow` 为 `CRF1 + record_count + length-prefixed TCP payload records`，manifest 字段和 `role` 枚举已明确；单 fixture 默认上限 256 KiB，超限时只能按完整 record 前缀截取并保留 handshake、connect、publish/play、metadata/config 和早期 media；fixture 明确只保存 TCP payload，不保存 IP/TCP 头和抓包时间戳。
- 2026-05-03：创建 `plans-5` 计划文档结构。已确认 `test_media_files/dump_rtmp_sms_gst` 中存在大量 0 字节 pcap，不能直接纳入测试；也确认可解析 pcap 为 Linux cooked v2，包含推流连接与拉流连接，可从 TCP payload 中抽取 C2S/S2C byte stream 作为小型 fixture。首选策略为提交抽取后的 compact fixture，不提交完整 pcap。

## 渐进式执行顺序

1. 先完成 Architecture，固定 fixture 格式、样例选择、断言分层和 CI 边界。
2. 再完成 Phase 01，把可提交测试数据、manifest 和抽取工具落地。
3. 再完成 Phase 02，先在 Sans-I/O core 与 pbt 层锁定协议重放和输入扰动鲁棒性。
4. 再完成 Phase 03，在 module 层验证真实 publish replay 到 engine/play 的集成路径。
5. 最后完成 Phase 04，把真实样例作为 fuzz seed，扩展传输异常 target 并跑 smoke。

## 阶段完成后的统一检查

```bash
cargo fmt
cargo clippy -p cheetah-rtmp-core
cargo test -p cheetah-rtmp-core
cargo clippy -p cheetah-rtmp-pbt
cargo test -p cheetah-rtmp-pbt
cargo clippy -p cheetah-rtmp-module
cargo test -p cheetah-rtmp-module
cd crates/cheetah-rtmp-fuzz && cargo +nightly fuzz build
```

新增 fuzz target 后还必须执行短跑 smoke：

```bash
cd crates/cheetah-rtmp-fuzz
cargo +nightly fuzz run fuzz_real_capture_server_replay -- -runs=128
cargo +nightly fuzz run fuzz_real_capture_client_post_handshake -- -runs=128
cargo +nightly fuzz run fuzz_transport_faults -- -runs=128
```
