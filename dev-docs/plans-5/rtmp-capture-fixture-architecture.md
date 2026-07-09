# RTMP 真实抓包 Fixture 架构设计

- 状态：已完成
- 范围：定义从 `test_media_files/dump_rtmp_sms_gst` 抽取 RTMP 测试数据的格式、样例选择、断言分层和测试边界。
- 完成标准：实现者能够据此生成可提交 fixture，并在 core、module、pbt、fuzz 中复用同一批真实抓包样例。

## 架构目标

本计划不把原始 pcap 直接放进常规测试。根 `.gitignore` 已忽略 `test_media_files`，并且当前目录里大量 `.pcap` 是 0 字节，直接读取会让 CI 与本地环境不一致。正确做法是把 pcap 当作来源材料，从可解析抓包中抽取小型 RTMP TCP payload fixture，并把抽取结果纳入 crate 自身的 `tests/testdata`。

新的 fixture 模型固定为三层：

1. 原始来源层：`test_media_files/dump_rtmp_sms_gst/*.pcap`，只在本地生成工具使用。
2. 可提交 fixture 层：`.rtmpflow` 保存真实 TCP payload 边界，`manifest.tsv` 保存来源、角色、预期事件和样例说明。
3. 测试消费层：core/pbt/module/fuzz 只读取 `.rtmpflow` 和 manifest，不读取原始 pcap。

## Fixture 格式

`.rtmpflow` 使用一个极简二进制格式，避免测试依赖外部 pcap parser：

```text
magic: 4 bytes = "CRF1"
record_count: u32 big-endian
records:
  payload_len: u32 big-endian
  payload_bytes: [payload_len]
```

每个 record 对应一个真实 TCP payload，不包含 IP/TCP 头。测试可以按原始 record 边界喂入，也可以把多个 record 合并模拟 TCP 粘包，或拆成 1 字节片段模拟半包。

`manifest.tsv` 字段固定为：

```text
case	source_pcap	stream_name	media_sig	role	fixture	expect_connected	expect_publish	expect_play	expect_media_min	notes
```

字段规则：

- `role` 只允许 `server_publish_c2s`、`server_play_c2s`、`client_publish_s2c`、`client_play_s2c`、`robustness_probe`。
- `expect_*` 字段使用 `0/1` 或整数，不使用自由文本。
- `notes` 记录增强 RTMP、fallback、空 pcap 跳过原因等非结构化说明。

## 样例选择

首批只选择非空且 tcpdump 可解析的 pcap，覆盖标准协议和真实兼容场景：

- H264/AAC 标准样例：`from_file_017_source_200kbps_768x320_fc8200c552.pcap`
- H265/AAC 标准样例：`from_file_003_bbb_1920x1080_hevc_5d24fd11cc.pcap`
- H265 大 payload 样例：`from_file_018_spreed_1080p_hevc_ccd40e8693.pcap`
- audio-only 样例：`from_file_010_fallback_audio_f1932e2a04.pcap`
- AV1 probe：`from_file_005_big_buck_bunny_av1_1080_10s_5mb_dd25581306.pcap`
- VP8 probe：`from_file_011_fallback_video_vp8_fb9c7e866d.pcap`
- VP9 probe：`from_file_012_fallback_video_vp9_72cd0a041b.pcap`
- H266/VVC probe：`from_file_014_chainsaw_man_04_vvc_1080p_aac_qpa0_qp20_ae3b4d9277.pcap`

标准样例用于强断言，probe 样例用于非标准和 enhanced/fallback 健壮性。0 字节基础命名样例如 `h264_aac.pcap`、`aac_only.pcap` 只在 manifest 备注或生成日志中记录，不生成 fixture。

## 断言分层

### 标准样例

- server core 重放 publish C2S 时必须产生 `Connected` 和 `PublishRequested`。
- 接受 publish 后必须产生至少 `expect_media_min` 个 `MediaData`。
- media timestamp 必须单调非递减。
- module 层必须能由 raw TCP publish replay 推入 engine，并被 RTMP play client 拉到音频或视频事件。

### 非标准和鲁棒性样例

- enhanced、fallback、截断、乱序、丢片样例不要求进入 Playing 或 Publishing。
- core/fuzz 只断言不 panic、不 OOM、不无限循环。
- module 只断言 rtmp module 仍为 `Running`，engine health 仍 live/ready，连接关闭不会拖垮模块。

## 具体任务

### A.1 明确 fixture 格式与边界

- [x] 固定 `.rtmpflow` magic、record_count 和 record length 编码。
- [x] 固定 manifest 字段和 `role` 枚举。
- [x] 明确单 fixture 默认上限 256 KiB，超过时只截取完成 handshake、connect、publish/play、metadata/config 和前若干 media record 的前缀。
- [x] 明确 fixture 只保留 TCP payload，不保留 IP/TCP 头和时间戳。

A.1 的实现边界固定如下：

- `.rtmpflow` 是测试 fixture 格式，不是协议格式；生产 crate 不得读取或导出该格式。
- `record_count` 必须等于后续 length-prefixed record 数；解析时不允许静默忽略尾部脏数据。
- `payload_len` 为 `u32` big-endian；生成工具必须拒绝 0 长度 record，测试 helper 可把 0 长度视为格式错误。
- 截取超限 pcap 时只能按 record 边界截断，不允许截断 record 内部 payload 生成标准 fixture；半包、截断 record 只能在鲁棒性测试视图中动态构造。
- 标准 fixture 必须包含完整 C0/C1、C2、connect、createStream、publish 或 play 相关命令；如果因上限无法保留首个 metadata/config/media record，该 pcap 只能降级为 probe。
- manifest 中 `fixture` 路径以 `crates/cheetah-rtmp-pbt/tests/testdata/rtmp-capture/` 为根的相对路径记录，避免不同工作目录导致测试读取失败。

### A.2 明确可解析 pcap 样例集

- [x] 生成工具必须跳过 0 字节 pcap。
- [x] 生成工具必须验证 pcap global header，当前已知 linktype 包括 Linux cooked v2。
- [x] 对每个 pcap 按 TCP flow 聚合 payload，并按端口 1935 与 payload 大小选择 publish C2S、play C2S、server S2C flow。
- [x] manifest 中记录被选 flow 的来源 pcap、stream name 和媒体签名。

A.2 的样例集和错误处理规则固定如下：

- 当前可用 pcap 的 linktype 为 276（Linux cooked v2）；生成工具必须显式支持该 linktype，并可选支持 Ethernet linktype 1。其他 linktype 返回结构化错误，不生成 fixture。
- 0 字节 pcap 必须跳过并记录为 `skipped_empty_pcap`；不能把空文件当作空 fixture。
- pcap global header magic、endianness、packet header length、captured length 必须校验；遇到截断 packet 或 payload 越界时，该 pcap 降级为 `skipped_malformed_pcap`。
- TCP payload flow key 使用 `(src_ip, src_port, dst_ip, dst_port)`；`dst_port == 1935` 为 C2S，`src_port == 1935` 为 S2C。
- publish C2S 候选优先选择 `dst_port == 1935` 且 payload bytes 最大的 flow；play C2S 候选选择包含 handshake/play 命令的小 C2S flow；play S2C 候选选择 `src_port == 1935` 且 payload bytes 最大的非 publish response flow。
- 如果一个 pcap 没有任何 1935 TCP payload flow，跳过并记录为 `skipped_no_rtmp_payload`。
- manifest 中 `source_pcap` 使用原始文件名，`stream_name` 来自 summary 或文件名中 `stream_target` 后缀，`media_sig` 来自 summary 的 `media_sig`，缺失时写 `unknown`，不能留空。

首批标准样例：

| case | source pcap | media_sig | 角色 |
| --- | --- | --- | --- |
| `h264_aac_publish` | `from_file_017_source_200kbps_768x320_fc8200c552.pcap` | `v=h264@768x320;a=aac@ch2` | standard publish/play |
| `h265_aac_publish` | `from_file_003_bbb_1920x1080_hevc_5d24fd11cc.pcap` | `v=h265@1920x1080;a=aac@ch6` | standard publish/play |
| `h265_large_publish` | `from_file_018_spreed_1080p_hevc_ccd40e8693.pcap` | `v=h265@1920x1080;a=aac@ch2` | large-payload standard |
| `audio_only_publish` | `from_file_010_fallback_audio_f1932e2a04.pcap` | `v=none@0x0;a=pcm@ch1` | audio-only standard |

首批 probe 样例：

| case | source pcap | media_sig | 角色 |
| --- | --- | --- | --- |
| `av1_probe` | `from_file_005_big_buck_bunny_av1_1080_10s_5mb_dd25581306.pcap` | `v=av1@1920x1080;a=none@ch0` | enhanced/compat probe |
| `vp8_probe` | `from_file_011_fallback_video_vp8_fb9c7e866d.pcap` | `v=vp8@320x240;a=none@ch0` | fallback video probe |
| `vp9_probe` | `from_file_012_fallback_video_vp9_72cd0a041b.pcap` | `v=vp9@320x240;a=none@ch0` | fallback video probe |
| `h266_probe` | `from_file_014_chainsaw_man_04_vvc_1080p_aac_qpa0_qp20_ae3b4d9277.pcap` | `v=h266@1920x1080;a=aac@ch2` | VVC compatibility probe |

### A.3 明确标准/非标准断言分层

- [x] 标准 H264/H265/audio-only 样例进入 core/module 强断言。
- [x] AV1/VP8/VP9/H266/enhanced/fallback 样例进入 probe 和 fuzz，默认只做鲁棒性断言。
- [x] 传输扰动场景使用统一 helper 生成 single-buffer、original-record、one-byte、coalesced、truncated、duplicated、reordered、dropped 视图。

A.3 的断言分层固定如下：

| 输入类别 | core 断言 | module 断言 | pbt/fuzz 断言 |
| --- | --- | --- | --- |
| standard publish C2S | 必须出现 `Connected`、`PublishRequested`；注入 `AcceptPublish` 后必须出现 `MediaData`；同 media type timestamp 单调 | raw TCP publish 后 RTMP play 必须收到对应 audio/video media；timestamp 单调 | 未扰动视图做同 core 强断言 |
| standard play/client S2C | client post-handshake replay 必须能处理 control/command/media，不 panic；可观察 `ClientStateChanged` 或 `MediaData` 时做最小事件断言 | 不直接使用完整 S2C fixture 驱动 module | 未扰动视图做 bounded client replay |
| probe/enhanced/fallback | `Err` 可接受；不得 panic、不得无限循环、不得内存无界增长 | module 必须保持 `Running`，engine health 必须 live/ready；连接关闭可接受 | 只做 bounded robustness，不要求成功解码或播放 |
| transport faults | `Err` 可接受；不得 panic；输入处理次数受 record 数上限约束 | 只对少量截断/丢片 replay 验证 module 健康度 | 作为主要 fuzz/PBT 扰动输入 |

统一 helper 必须提供这些输入视图：

- `single_buffer`：合并所有 record，模拟一次性读到完整 TCP byte stream。
- `original_records`：按真实 TCP payload 边界喂入。
- `one_byte_chunks`：逐字节喂入，验证半包 buffering。
- `coalesced_n`：每 N 个 record 合并，模拟 TCP 粘包。
- `truncated_prefix`：只喂入前缀，验证截断输入不 panic。
- `duplicate_record`：重复指定 record，验证重复输入不会破坏 bounded processing。
- `swap_adjacent`：交换相邻 record，验证乱序输入鲁棒性。
- `drop_every_nth`：按 N 丢弃 record，作为 RTMP 范围内的 datagram-like 丢片模型。

断言边界：

- 强断言只适用于未扰动 standard fixture；一旦应用截断、乱序、丢片、重复，测试不得要求协议成功。
- probe fixture 发现真实兼容缺口时，先形成回归样例；生产修复必须回到 `cheetah-codec`、RTMP core 或明确 module ingest/egress 边界，不能在测试里放宽为“成功”。
- 所有 fuzz/PBT helper 必须有最大 record 数和最大输入字节数；超限输入应被截断为 bounded test view，而不是让测试长时间运行。

## 最新进展

- 2026-05-03：完成 A.3。标准、probe、transport fault 三类输入的断言边界已固定；标准样例强制行为断言，probe 与扰动样例只做 bounded robustness，module probe 只要求模块和 engine 健康度保持正常。
- 2026-05-03：完成 A.2。可解析 pcap 样例集、flow 选择规则和跳过/错误处理规则已固定；当前非空候选均为 Linux cooked v2，标准样例覆盖 H264/AAC、H265/AAC、H265 大 payload、audio-only，probe 样例覆盖 AV1、VP8、VP9、H266/VVC。
- 2026-05-03：完成 A.1。`.rtmpflow` 二进制布局、manifest 字段、`role` 枚举、256 KiB 默认上限、按完整 record 前缀截取、只保留 TCP payload 的边界已固定；同时明确半包/截断 record 不进入标准 fixture，只在测试视图中动态构造。
- 2026-05-03：计划已创建，任务未开始。已确认 fixture 策略为“提取小夹具”，不是运行时读取原始 pcap；“udp 丢包”在 RTMP 范围内按 datagram-like 输入扰动建模。

## 完成后检查

```bash
cargo fmt
cargo test -p cheetah-rtmp-pbt --test capture_fixture_manifest
cargo test -p cheetah-rtmp-core capture
```
