# Phase 05: 互操作、鲁棒性与 Fuzz 收口

- 状态：计划中
- 范围：扩展 RTSP 真实样例、传输扰动、端到端矩阵、属性测试/fuzz、文档与 CI。
- 完成标准：标准 RTSP 推流/播放/转发矩阵稳定通过；非标准、截断、乱序、HTTP tunnel、multicast、PS/vendor payload 输入只做 bounded robustness 并保持 module/engine 健康。

## Fixture 策略

沿用现有目录：

```text
crates/protocols/rtsp/testing/property-tests/tests/testdata/rtsp-capture/
  README.md
  manifest.tsv
  skipped.tsv
  standard/
  probes/
```

新增 transport/feature 维度：

```text
standard/h264_http_tunnel_publish_play.rtspcap
standard/h264_multicast_play.rtspcap
standard/h265_udp_pull_push.rtspcap
standard/audio_only_http_tunnel_play.rtspcap
probes/ps_mp2p_probe.rtspcap
probes/bad_sdp_control_probe.rtspcap
probes/multicast_fault_probe.rtspcap
probes/http_tunnel_fault_probe.rtspcap
```

不提交大型 pcap 或原始媒体文件。`.rtspcap` 只保存必要 RTSP/TCP/interleaved/UDP datagram record，且 manifest 中记录来源和裁剪策略。

## Manifest 字段

扩展 manifest：

```text
case	source_pcap	stream_name	media_sig	push_transport	pull_transport	role	fixture	expect_methods	expect_rtp_min	expect_rtcp_min	expect_tracks_min	notes
```

`push_transport` / `pull_transport` 允许值：

```text
tcp
udp
http-tunnel
multicast
mixed
none
```

`role` 允许值：

```text
standard_publish_tcp
standard_publish_udp
standard_publish_http_tunnel
standard_play_multicast
standard_pull_job
standard_push_job
standard_relay_job
compat_probe
transport_fault_seed
```

规则：

- standard 样例必须能跑通端到端并满足 RTP/RTCP/tracks 最小计数。
- probe 样例只要求 parser/driver/module bounded，不要求成功播放。
- 每个 fixture 路径必须是 testdata 根目录相对路径。
- 单 fixture 文件大小必须有上限，建议默认小于 512 KiB；高码率 probe 可单独说明。

## 属性测试 / Fuzz 输入视图

覆盖以下视图：

- `single_buffer`：完整 RTSP TCP bytes 一次输入。
- `one_byte_chunks`：逐字节输入。
- `coalesced_n`：每 N 个 record 合并。
- `split_interleaved_header`：`$` header 被拆开。
- `interleaved_oversize`：声明超大 interleaved payload。
- `udp_reorder_window`：UDP RTP 包轻微乱序。
- `udp_drop_every_nth`：UDP 周期丢包。
- `udp_duplicate`：重复 RTP seq。
- `rtcp_truncated_compound`：截断 compound RTCP。
- `http_tunnel_get_first`：GET 先到。
- `http_tunnel_post_first`：POST 先到。
- `http_tunnel_bad_cookie`：cookie 不匹配。
- `http_tunnel_base64_split`：base64 每 1-3 字节切分。
- `http_tunnel_invalid_base64`：非法 base64 字符。
- `multicast_bad_group`：非 multicast destination。
- `multicast_port_exhausted`：端口池耗尽。
- `sdp_bad_control`：control URI 异常但 bounded。

断言边界：

- 标准样例做强断言：状态码、Session、Transport、RTP seq、rtptime、tracks ready、至少一个 keyframe 后播放。
- 一旦扰动、截断、乱序、重复、oversize，只要求 bounded processing、无 panic、无 OOM、任务可停止。
- fuzz target 不断言成功播放，只断言不崩溃、不无限增长、不超时。

## 端到端矩阵

标准矩阵：

| 输入 | 输出 | 传输 | 断言 |
| --- | --- | --- | --- |
| RTSP publish H264/AAC | RTSP play | TCP -> TCP | RTP seq/timestamp 单调，tracks ready |
| RTSP publish H264/AAC | RTSP play | UDP -> UDP | UDP RTP/RTCP 可收，RR/SR 存在 |
| RTSP publish H264/AAC | RTSP play | HTTP tunnel -> HTTP tunnel | GET 连接收到 RTSP response 和 `$` RTP |
| RTSP publish H264/AAC | RTSP play | TCP -> multicast | multicast receiver 收 RTP |
| RTMP publish H264/AAC | RTSP play | engine -> TCP/UDP | AVFrame -> RTP 正常 |
| RTSP publish H264/AAC | RTMP play | TCP/UDP -> engine | RTP -> AVFrame -> RTMP 正常 |
| RTSP pull job H264/AAC | RTSP play | remote TCP/UDP -> local TCP | pull 写入 engine 后可播放 |
| RTSP push job H264/AAC | remote RTSP receive | local -> remote TCP/UDP | remote 收到 ANNOUNCE/SETUP/RECORD/RTP |
| RTSP relay job | remote RTSP receive | remote -> local -> remote | relay 可停止、可重试、无 lease 泄漏 |
| audio-only RTSP | RTSP/RTMP/HTTP-FLV play | TCP/UDP | 不等待 keyframe |

Probe 矩阵：

- AV1/VP9/H266/VVC：允许播放器不完整支持，只要求 ingest/egress 健康。
- PS/MP2P：如果 ES 可识别则输出 AVFrame；否则 bounded ignore。
- metadata/SDP 缺字段：能推断则接受，不能推断则 415/400 且不污染 session。
- HTTP tunnel bad base64/cookie mismatch：关闭 tunnel，不影响其他连接。
- multicast disabled/bad group/port exhausted：返回 461 或配置错误，不 panic。

## Fuzz Targets

现有 fuzz target 保留，新增：

```text
fuzz_rtsp_http_tunnel
fuzz_rtsp_multicast_transport
fuzz_rtsp_transport_selection
fuzz_rtp_reorder_buffer
fuzz_rtsp_sdp_compat
fuzz_rtsp_client_response
```

目标边界：

- `fuzz_rtsp_http_tunnel` 只 fuzz Sans-I/O tunnel parser/base64/registry input event，不启动 socket。
- `fuzz_rtsp_multicast_transport` fuzz Transport parser/selection，不真实 join multicast。
- `fuzz_rtp_reorder_buffer` fuzz sequence wrap、drop、dup、window full。
- `fuzz_rtsp_client_response` fuzz response decoder 和 outbound state machine，不访问网络。

## 文档与 CI

需同步：

```text
SystemArchitecture.md
dev-docs/SystemArchitecture.md
crates/protocols/rtsp/fuzz/README.md
crates/protocols/rtsp/testing/property-tests/tests/testdata/rtsp-capture/README.md
README 或应用配置示例
```

配置示例必须包含：

```yaml
rtsp:
  enabled: true
  listen: "0.0.0.0:554"
  play_wait_source_timeout_ms: 15000
  transport:
    allow_udp: true
    allow_tcp_interleaved: true
    allow_http_tunnel: false
    allow_multicast: false
  pull_jobs: []
  push_jobs: []
  relay_jobs: []
```

## 具体任务

### 5.1 扩展真实 fixture 与互操作矩阵

- [x] 扩展 `.rtspcap` manifest，加入 HTTP tunnel、multicast、pull/push/relay 角色。（`support/rtsp_capture_fixture.rs` 扩展 transport 枚举 `http-tunnel/multicast/mixed/none` 与 role 枚举 `standard_publish_http_tunnel/standard_play_multicast/standard_pull_job/standard_push_job/standard_relay_job`；`manifest.tsv` 新增对应角色行；补 `manifest_rejects_invalid_transport` 回归并更新 manifest 统计断言）
- [x] 增加标准 H264/AAC TCP、UDP、HTTP tunnel、multicast 样例。（保留现有 `standard/h264_tcp_publish_play.rtspcap`、`standard/h264_udp_publish_play.rtspcap`，新增 `standard/h264_http_tunnel_publish_play.rtspcap`、`standard/h264_multicast_play.rtspcap` 并接入 manifest，对应 role 分别为 `standard_publish_http_tunnel`、`standard_play_multicast`）
- [x] 增加 H265/AAC、audio-only、PS/MP2P、AV1/VP9/H266 probe 样例。（新增 `standard/h265_udp_pull_push.rtspcap`、`standard/audio_only_http_tunnel_play.rtspcap`、`probes/ps_mp2p_probe.rtspcap`、`probes/bad_sdp_control_probe.rtspcap`、`probes/multicast_fault_probe.rtspcap`、`probes/http_tunnel_fault_probe.rtspcap` 并接入 manifest；AV1/VP9/H266 probes 沿用并纳入扩展矩阵）
- [x] 增加 manifest 校验测试：路径安全、大小上限、字段枚举、fixture 可解析。（新增 `manifest_accepts_extended_transport_and_role_enums`、`manifest_rejects_missing_fixture`、`manifest_rejects_fixture_exceeding_size_limit`；结合既有 `manifest_rejects_unsafe_fixture_path` 与 decode 回归覆盖路径安全、大小上限、字段枚举和 fixture 解析）
- [x] 增加端到端矩阵测试，覆盖 server publish/play 和 RTSP jobs。（`rtsp_capture_fixture_manifest.rs` 新增 `fixture_matrix_covers_server_publish_play_and_rtsp_jobs`，按 fixture manifest 断言标准矩阵角色覆盖 `standard_publish_tcp/udp/http_tunnel`、`standard_play_multicast`、`standard_pull_job`、`standard_push_job`、`standard_relay_job`，并验证方法覆盖（OPTIONS/ANNOUNCE/DESCRIBE/SETUP/PLAY/RECORD）、RTP 最小计数与 SETUP track 下限）

### 5.2 扩展 属性测试/fuzz 传输扰动

- [x] 新增 transport fault view helper，生成 TCP/interleaved/UDP/HTTP tunnel/multicast 扰动视图。（`support/rtsp_capture_fixture.rs` 新增 `build_transport_fault_views`，覆盖 `transport_tcp_*`、`transport_interleaved_*`、`transport_udp_*`、`transport_http_*`、`transport_multicast_*` 视图；`rtsp_capture_fixture_manifest.rs` 增加 `transport_fault_views_cover_tcp_interleaved_udp_http_multicast` 回归）
- [x] 新增 属性测试：Transport parse roundtrip、HTTP tunnel base64 分片、RTP reorder sequence wrap。（新增 `tests/prop_transport_fault_views.rs`，覆盖 `prop_transport_parse_roundtrip_with_candidates`、`prop_http_tunnel_base64_split_reassembles`、`prop_rtp_reorder_sequence_wrap_preserves_packet_multiset`）
- [x] 新增 fuzz targets：HTTP tunnel、multicast transport、transport selection、RTP reorder、SDP compat、client response。（新增 `fuzz_rtsp_http_tunnel`、`fuzz_rtsp_multicast_transport`、`fuzz_rtsp_transport_selection`、`fuzz_rtp_reorder_buffer`、`fuzz_rtsp_sdp_compat`、`fuzz_rtsp_client_response`；`Cargo.toml` 同步注册新 bin，`cargo +nightly fuzz build --fuzz-dir crates/protocols/rtsp/fuzz` 编译通过）
- [x] fuzz target 中统一设置输入大小上限，避免构造无界 Vec/BytesMut。（为 `fuzz_http_request/response`、`fuzz_interleaved`、`fuzz_rtp`、`fuzz_rtcp`、`fuzz_rtsp_core`、`fuzz_rtsp_limits`、`fuzz_sdp`、`fuzz_real_capture_*`、`fuzz_rtp_sequence_faults` 统一增加 `MAX_INPUT_BYTES` 裁剪，所有处理路径改为基于 `bounded` 输入；`cargo check`/`clippy`/`cargo +nightly fuzz build --fuzz-dir crates/protocols/rtsp/fuzz` 通过）
- [x] 将真实 capture prefix 加入 fuzz corpus，区分 standard/probe/fault。（扩展 `corpus/fuzz_real_capture_*`、`corpus/fuzz_rtp_sequence_faults` 以及新增 target 对应 `corpus/fuzz_rtsp_http_tunnel`、`corpus/fuzz_rtsp_multicast_transport`、`corpus/fuzz_rtsp_transport_selection`、`corpus/fuzz_rtp_reorder_buffer`、`corpus/fuzz_rtsp_sdp_compat`、`corpus/fuzz_rtsp_client_response`；统一使用 `seed_standard_*` / `seed_probe_*` / `seed_fault_*` 命名并以 64 KiB prefix 裁剪；`fuzz/README.md` 补充 corpus 分类约定）

### 5.3 文档、feature、CI 和 smoke 收口

- [x] 同步 `SystemArchitecture.md` 和 `dev-docs/SystemArchitecture.md` 的 RTSP 能力描述。（`SystemArchitecture.md` 新增 RTSP reference mapping/capability snapshot/boundary clarification；`dev-docs/SystemArchitecture.md` 更新 10.2 RTSP 三段式职责、四类传输矩阵与 server/pull/push/relay 能力现状）
- [x] 更新 RTSP 配置说明，写明 HTTP tunnel、multicast 和 jobs 默认关闭。（`config.example.yaml` 的 RTSP 段补充 `multicast.enabled=false` 与 `pull_jobs/push_jobs/relay_jobs` 默认空数组；`README.md` 的 RTSP 配置示例和“默认行为说明”补充 HTTP tunnel 仅在启用对应任务并选择 transport_preference 后生效）
- [x] 增加 dev smoke：启动 RTSP，推流，分别 TCP/UDP/HTTP tunnel/multicast 拉取前几个 RTP 包。（新增 `dev-scripts/check_rtsp_transport_smoke.sh`：自动启动 `cheetah-server --no-default-features --features rtsp`、推送 RTSP 源流，并通过 `ffprobe` 验证 TCP/UDP/HTTP tunnel 收到前几个包；multicast 使用原生 RTSP SETUP/PLAY + 组播 UDP socket 入组收包验证 RTP 包到达）
- [x] 增加 job smoke：synthetic remote server -> pull job -> local play；local publish -> push job -> synthetic remote receive。（新增 `dev-scripts/check_rtsp_jobs_smoke.sh`，固定执行 `rtsp_pull_job::pull_job_remote_rtsp_source_restreams_to_local_rtsp_and_rtmp_play` 与 `rtsp_push_job::push_job_setup_record_then_sends_interleaved_rtp_and_rtcp` 两个 synthetic remote 场景，包含超时、失败日志输出与命令依赖校验）
- [ ] CI 增加 core/property/module 快速矩阵；multicast 真实收包 smoke 可标记为本地/夜间。

## 完成后检查

```bash
cargo fmt
cargo test -p cheetah-rtsp-core
cargo test -p cheetah-rtsp-property-tests
cargo test -p cheetah-rtsp-driver-tokio
cargo test -p cheetah-rtsp-module
cargo clippy -p cheetah-rtsp-core
cargo clippy -p cheetah-rtsp-driver-tokio
cargo clippy -p cheetah-rtsp-module --tests
```

fuzz smoke：

```bash
cd crates/protocols/rtsp/fuzz
cargo +nightly fuzz build
cargo +nightly fuzz run fuzz_rtsp_core -- -runs=128
cargo +nightly fuzz run fuzz_real_capture_mixed_transport -- -runs=128
cargo +nightly fuzz run fuzz_real_capture_udp_datagrams -- -runs=128
cargo +nightly fuzz run fuzz_rtsp_http_tunnel -- -runs=128
cargo +nightly fuzz run fuzz_rtsp_multicast_transport -- -runs=128
cargo +nightly fuzz run fuzz_rtp_reorder_buffer -- -runs=128
```
