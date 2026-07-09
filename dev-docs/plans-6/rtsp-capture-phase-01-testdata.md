# Phase 01: RTSP 抓包测试数据落地

- 状态：计划中
- 范围：在 `crates/cheetah-rtsp-pbt/tests/testdata` 下新增真实 RTSP 抓包 fixture、manifest、生成工具和数据完整性测试。
- 完成标准：干净 checkout 不依赖 `test_media_files` 即可运行基于 fixture 的 core/pbt/module/fuzz 测试；本地有 pcap 时可重新生成同等结构的 fixture。

## 目标目录

新增目录：

```text
crates/cheetah-rtsp-pbt/tests/testdata/rtsp-capture/
  README.md
  manifest.tsv
  standard/h264_tcp_publish_play.rtspcap
  standard/h264_udp_publish_play.rtspcap
  standard/h265_tcp_publish_play.rtspcap
  standard/audio_only_udp_publish_play.rtspcap
  probes/av1_probe.rtspcap
  probes/vp8_probe.rtspcap
  probes/vp9_probe.rtspcap
  probes/h266_probe.rtspcap
  probes/high_bitrate_probe.rtspcap
```

新增测试/工具代码：

```text
crates/cheetah-rtsp-pbt/tests/rtsp_capture_fixture_manifest.rs
crates/cheetah-rtsp-pbt/tests/support/rtsp_capture_fixture.rs
dev-scripts/rtsp_extract_capture_fixtures.py
```

`support/rtsp_capture_fixture.rs` 只被测试 crate 使用，负责读取 `.rtspcap`、解析 manifest、生成不同输入视图。`dev-scripts/rtsp_extract_capture_fixtures.py` 是本地再生成工具，不进入运行时 crate。

## Fixture README 要求

`README.md` 必须说明：

- fixture 来源为 `test_media_files/dump_rtsp_sms_gst/from_file_*.pcap`。
- 原始 pcap 被 `.gitignore` 忽略，不是 CI 输入。
- 基础短名 pcap 当前多为 0 字节，因此不作为首批 fixture 来源。
- `.rtspcap` 是测试格式，不是协议格式。
- 标准样例和 probe 样例的断言差异。
- 再生成命令和 fixture 大小上限。

再生成命令固定为：

```bash
python3 dev-scripts/rtsp_extract_capture_fixtures.py \
  --source-dir test_media_files/dump_rtsp_sms_gst \
  --out-dir crates/cheetah-rtsp-pbt/tests/testdata/rtsp-capture \
  --max-fixture-bytes 524288
```

## 生成工具规则

`dev-scripts/rtsp_extract_capture_fixtures.py` 必须只使用 Python 标准库，避免 CI 和开发机依赖 tshark/scapy：

- 解析 pcap global header，支持 little/big endian。
- 解析 pcap packet header，拒绝 captured length 越界和截断 packet。
- 支持 Linux cooked v2 linktype 276，建议支持 Ethernet linktype 1。
- 解析 IPv4 header，跳过非 TCP/UDP payload。
- 解析 TCP header，按 flow 聚合 payload record，保留真实 TCP payload 边界。
- 解析 UDP header，按 flow 聚合 datagram，保留真实 datagram 边界。
- 解析 RTSP TCP payload 中的 request/response start line、headers、`Content-Length`、`Transport`、`Session`。
- 从 SETUP request/response 推断 UDP `client_port`、`server_port` 与 TCP interleaved channel。
- 从 `$` interleaved frame 切出 RTP/RTCP payload，按 channel 奇偶和 SETUP `interleaved=x-y` 归类。
- 按完整 record/datagram 前缀截取到 `--max-fixture-bytes`，标准 fixture 不允许截断单条 record/datagram。

## 具体任务

### 1.1 新增 RTSP capture fixture 目录

- [ ] 创建 `crates/cheetah-rtsp-pbt/tests/testdata/rtsp-capture/README.md`。
- [ ] 创建 `manifest.tsv`，首行固定为：

```text
case	source_pcap	stream_name	media_sig	push_transport	pull_transport	role	fixture	expect_methods	expect_rtp_min	expect_rtcp_min	expect_tracks_min	notes
```

- [ ] 创建 `standard/` 与 `probes/` 子目录，并放置 `.gitkeep`。
- [ ] README 中明确 `from_file_*` 非空抓包与基础短名空 pcap 的边界。

### 1.2 新增 pcap 抽取工具和 manifest 校验

- [ ] 新增 `dev-scripts/rtsp_extract_capture_fixtures.py`。
- [ ] 工具必须可从 `summary_from_files.tsv` 读取 `case`、`media_sig`、`push_transport`、`pull_transport`、`stream_target`。
- [ ] 工具必须把 0 字节 pcap 写入生成日志或 README skipped 段落，不写入 manifest standard/probe 行。
- [ ] 新增 `rtsp_capture_fixture_manifest.rs`，校验 manifest 表头、字段数、路径安全、fixture 存在、文件大小、magic、record_count、record 长度、尾部脏数据、非法 kind、0 长度 payload。
- [ ] 新增 `tests/support/rtsp_capture_fixture.rs`，提供 manifest 加载、fixture 解码、按角色筛选、TCP/UDP/RTP fault view 构造。

### 1.3 生成首批标准与 probe fixture

- [ ] 从 H264 TCP、H264 UDP、H265 TCP、audio-only UDP 非空 pcap 生成 `standard/*.rtspcap`。
- [ ] 从 AV1、VP8、VP9、H266/VVC、4K/high-bitrate 非空 pcap 生成 `probes/*.rtspcap`。
- [ ] manifest 中 standard 样例设置 `expect_methods` 和 `expect_rtp_min>=1`。
- [ ] manifest 中 probe 样例可设置 `expect_rtp_min=0`，`notes` 明确 compatibility/probe。
- [ ] 对每个 fixture 记录是否因大小上限被按完整 record/datagram 前缀截取。

## 完成后检查

```bash
cargo fmt
cargo test -p cheetah-rtsp-pbt --test rtsp_capture_fixture_manifest
python3 dev-scripts/rtsp_extract_capture_fixtures.py \
  --source-dir test_media_files/dump_rtsp_sms_gst \
  --out-dir /tmp/cheetah-rtsp-capture-fixtures \
  --max-fixture-bytes 524288
```

生成工具的 `/tmp` 输出必须能通过同一个 manifest 校验逻辑，避免提交 fixture 与本地再生成格式漂移。
