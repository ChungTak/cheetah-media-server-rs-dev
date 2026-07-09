# Phase 01: RTMP 抓包测试数据落地

- 状态：已完成
- 范围：在 `crates/cheetah-rtmp-pbt/tests/testdata` 下新增真实抓包 fixture、manifest、生成工具和数据完整性测试。
- 完成标准：干净 checkout 不依赖 `test_media_files` 即可运行基于 fixture 的 core/pbt/module/fuzz 测试；本地有 pcap 时可重新生成同等结构的 fixture。

## 目标目录

新增目录：

```text
crates/cheetah-rtmp-pbt/tests/testdata/rtmp-capture/
  manifest.tsv
  README.md
  standard/h264_aac_publish.rtmpflow
  standard/h265_aac_publish.rtmpflow
  standard/h265_large_publish.rtmpflow
  standard/audio_only_publish.rtmpflow
  probes/av1_probe.rtmpflow
  probes/vp8_probe.rtmpflow
  probes/vp9_probe.rtmpflow
  probes/h266_probe.rtmpflow
```

新增测试/工具代码：

```text
crates/cheetah-rtmp-pbt/tests/capture_fixture_manifest.rs
crates/cheetah-rtmp-pbt/tests/support/capture_fixture.rs
dev-scripts/rtmp_extract_capture_fixtures.py
```

`support/capture_fixture.rs` 只被测试 crate 使用，负责读取 `.rtmpflow`、解析 manifest、生成不同输入视图。`dev-scripts/rtmp_extract_capture_fixtures.py` 是本地再生成工具，不进入运行时 crate。

## 具体任务

### 1.1 新增 RTMP capture fixture 目录

- [x] 创建 `crates/cheetah-rtmp-pbt/tests/testdata/rtmp-capture/README.md`，说明 fixture 来源、格式、再生成命令和断言分层。
- [x] 创建 `manifest.tsv`，首行固定字段为 `case	source_pcap	stream_name	media_sig	role	fixture	expect_connected	expect_publish	expect_play	expect_media_min	notes`。
- [x] 创建 `standard/` 与 `probes/` 子目录，标准样例和非标准 probe 分开管理。

### 1.2 新增 pcap 抽取工具与 manifest 校验

- [x] 新增 `dev-scripts/rtmp_extract_capture_fixtures.py`，只使用 Python 标准库解析 pcap global header、packet header、Linux cooked v2、IPv4、TCP header 和 TCP payload。
- [x] 工具参数固定为：

```bash
python3 dev-scripts/rtmp_extract_capture_fixtures.py \
  --source-dir test_media_files/dump_rtmp_sms_gst \
  --out-dir crates/cheetah-rtmp-pbt/tests/testdata/rtmp-capture \
  --max-fixture-bytes 262144
```

- [x] 工具必须按 TCP flow 聚合 record，优先选择目的端口 1935 且 payload 最大的 C2S flow 作为 publish fixture。
- [x] 对拉流相关 fixture，选择目的端口 1935 的小 C2S play flow 与源端口 1935 的 S2C media flow，供 client/post-handshake fuzz 使用。
- [x] 新增 `capture_fixture_manifest.rs`，校验 manifest 字段数、fixture 文件存在、magic 正确、record_count 与实际 record 数一致、单 fixture 不超过上限。

### 1.3 生成首批标准与非标准样例

- [x] 从 H264/AAC、H265/AAC、H265 大 payload、audio-only 可解析 pcap 生成 `standard/*.rtmpflow`。
- [x] 从 AV1、VP8、VP9、H266/VVC 可解析 pcap 生成 `probes/*.rtmpflow`。
- [x] manifest 中标准样例设置 `expect_connected=1`、`expect_publish=1`、`expect_media_min>=1`。
- [x] manifest 中 probe 样例设置 `expect_media_min=0`，`notes` 明确为 enhanced/fallback/compat probe。
- [x] 对 0 字节 pcap 不生成 fixture；在 README 中记录“基础命名 pcap 目前多为空文件，不作为 CI 输入”。

## 最新进展

- 2026-05-03：完成 1.3。已提交首批 8 个 fixture：4 个标准 publish 样例和 4 个 probe 样例。标准样例来自 H264/AAC、H265/AAC、H265 大 payload、audio-only 可解析 pcap，manifest 设置 `expect_connected=1`、`expect_publish=1`、`expect_media_min=1`；probe 样例来自 AV1、VP8、VP9、H266/VVC 非空 `from_file_*` 抓包，manifest 设置 `expect_media_min=0`，notes 明确为 enhanced/fallback/compat probe。当前基础短名 probe pcap 为空文件，README 已记录它们不作为 CI 输入。
- 2026-05-03：完成 1.2。新增 Python 标准库 pcap 抽取脚本，可从本地 pcap 中解析 Linux cooked v2/Ethernet + IPv4/TCP payload、按 flow 聚合、选择 publish/play/server-media 方向并按 `.rtmpflow` record 边界截断到 256 KiB；新增 pbt 测试 helper 与 `capture_fixture_manifest.rs`，覆盖 manifest 表头/字段、role/flag/number、路径安全、fixture 存在性、大小上限、magic、record 长度、截断、零长度 record 和尾随字节。脚本 smoke 当前可从本地标准 pcap 生成 4 个 fixture，AV1/VP8/VP9/H266 基础 probe pcap 为空文件，因此按预期跳过，后续 1.3 会根据可解析 probe 来源调整样例落地。
- 2026-05-03：完成 1.1。`rtmp-capture/README.md`、`manifest.tsv`、`standard/`、`probes/` 已落地；当前 manifest 只有固定表头，`.rtmpflow` 样例会在 1.3 由生成工具产出。
- 2026-05-03：计划已创建，任务未开始。已通过只读勘察确认可解析 pcap 采用 Linux cooked v2，且同一 pcap 中通常包含 publish 与 play 两组 TCP 连接。

## 完成后检查

```bash
cargo fmt
cargo test -p cheetah-rtmp-pbt --test capture_fixture_manifest
python3 dev-scripts/rtmp_extract_capture_fixtures.py \
  --source-dir test_media_files/dump_rtmp_sms_gst \
  --out-dir /tmp/cheetah-rtmp-capture-fixtures \
  --max-fixture-bytes 262144
```

生成工具的 `/tmp` 输出必须能通过同一个 manifest 校验逻辑，避免提交 fixture 与本地再生成格式漂移。
