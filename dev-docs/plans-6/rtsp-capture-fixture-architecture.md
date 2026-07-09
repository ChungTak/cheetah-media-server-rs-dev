# RTSP 真实抓包 Fixture 架构设计

- 状态：计划中
- 范围：定义从 `test_media_files/dump_rtsp_sms_gst` 抽取 RTSP 测试数据的格式、样例选择、断言分层和测试边界。
- 完成标准：实现者能够据此生成可提交 fixture，并在 core、module、pbt、fuzz 中复用同一批真实 RTSP 抓包样例。

## 架构目标

本计划不把原始 pcap 直接放进常规测试。根 `.gitignore` 已忽略 `test_media_files`，原始 pcap 体积大且本地基础短名 pcap 存在大量 0 字节文件，直接读取会让 CI 与本地环境不一致。正确做法是把 pcap 当作来源材料，从非空 `from_file_*` 抓包中抽取小型 RTSP fixture，并把抽取结果纳入 crate 自身的 `tests/testdata`。

新的 fixture 模型固定为三层：

1. 原始来源层：`test_media_files/dump_rtsp_sms_gst/*.pcap`，只在本地生成工具使用。
2. 可提交 fixture 层：`.rtspcap` 保存真实 RTSP TCP payload、TCP interleaved payload 和 UDP RTP/RTCP datagram，`manifest.tsv` 保存来源、角色、传输、媒体签名和预期事件。
3. 测试消费层：core/pbt/module/fuzz 只读取 `.rtspcap` 和 manifest，不读取原始 pcap。

## Fixture 格式

`.rtspcap` 使用极简二进制格式，避免测试依赖外部 pcap parser：

```text
magic: 4 bytes = "RSF1"
record_count: u32 big-endian
records:
  kind: u8
  flags: u8
  flow_id: u16 big-endian
  delta_us: u32 big-endian
  payload_len: u32 big-endian
  payload_bytes: [payload_len]
```

字段规则：

- `kind` 枚举：
  - `1 = rtsp_tcp_c2s`：客户端到服务端 RTSP TCP payload，可包含多个 RTSP request 或 `$` interleaved frame。
  - `2 = rtsp_tcp_s2c`：服务端到客户端 RTSP TCP payload，可包含 response 或 `$` interleaved frame。
  - `3 = udp_publish_rtp`：publisher 发往服务器的 RTP datagram。
  - `4 = udp_publish_rtcp`：publisher 发往服务器的 RTCP datagram。
  - `5 = udp_play_rtp`：服务器发往 player 的 RTP datagram。
  - `6 = udp_play_rtcp`：服务器发往 player 的 RTCP datagram。
  - `7 = tcp_interleaved_rtp`：从 RTSP TCP payload 中切出的 `$` RTP frame payload，payload 不含 4 字节 `$` 头。
  - `8 = tcp_interleaved_rtcp`：从 RTSP TCP payload 中切出的 `$` RTCP frame payload，payload 不含 4 字节 `$` 头。
- `flags` bit：
  - `0x01 = standard_assertable`，未扰动时可做强断言。
  - `0x02 = probe_only`，只做鲁棒性断言。
  - `0x04 = truncated_source_prefix`，生成时因上限按 record 前缀截取。
- `flow_id` 是生成工具按五元组和角色稳定分配的小整数，不保存 IP/TCP/UDP 头。
- `delta_us` 是相对 fixture 首条 record 的微秒偏移，只供重放节奏或诊断使用；core/PBT 默认不按真实时间 sleep。
- `payload_len` 为 `u32` big-endian；生成工具必须拒绝 0 长度 record。

## Manifest 格式

`manifest.tsv` 字段固定为：

```text
case	source_pcap	stream_name	media_sig	push_transport	pull_transport	role	fixture	expect_methods	expect_rtp_min	expect_rtcp_min	expect_tracks_min	notes
```

字段规则：

- `role` 只允许 `standard_publish_tcp`、`standard_publish_udp`、`standard_play_tcp`、`standard_play_udp`、`compat_probe`、`transport_fault_seed`。
- `push_transport` / `pull_transport` 只允许 `tcp` 或 `udp`。
- `expect_methods` 使用逗号分隔 RTSP 方法名，例如 `OPTIONS,ANNOUNCE,SETUP,RECORD` 或 `OPTIONS,DESCRIBE,SETUP,PLAY`。
- `expect_rtp_min`、`expect_rtcp_min`、`expect_tracks_min` 使用整数。
- `notes` 记录空 pcap、fallback、厂商兼容、截断前缀、已知 codec 缺口等说明。

## 样例选择

首批只选择非空且可解析的 `from_file_*` pcap，覆盖标准协议和真实兼容场景。基础短名 pcap 如 `h264_aac__push_tcp__pull_tcp.pcap` 当前为 0 字节，只在生成日志或 README 中记录为 skipped。

标准样例：

| case | source pcap | media_sig | 传输 | 用途 |
| --- | --- | --- | --- | --- |
| `h264_tcp_publish_play` | `from_file_017_source_200kbps_768x320_fc8200c552__push_tcp__pull_tcp.pcap` | `v=h264@768x320;a=aac@ch2` | push tcp / pull tcp | 标准 TCP interleaved publish + play |
| `h264_udp_publish_play` | `from_file_017_source_200kbps_768x320_fc8200c552__push_udp__pull_udp.pcap` | `v=h264@768x320;a=aac@ch2` | push udp / pull udp | 标准 UDP RTP/RTCP publish + play |
| `h265_tcp_publish_play` | `from_file_003_bbb_1920x1080_hevc_5d24fd11cc__push_tcp__pull_tcp.pcap` | `v=h265@1920x1080;a=aac@ch6` | push tcp / pull tcp | H265 TCP interleaved 标准样例 |
| `audio_only_udp_publish_play` | `from_file_010_fallback_audio_f1932e2a04__push_udp__pull_udp.pcap` | `v=none@0x0;a=pcm@ch1` | push udp / pull udp | audio-only UDP 标准样例 |

probe 样例：

| case | source pcap | media_sig | 传输 | 用途 |
| --- | --- | --- | --- | --- |
| `av1_probe` | `from_file_005_big_buck_bunny_av1_1080_10s_5mb_dd25581306__push_tcp__pull_tcp.pcap` | `v=av1@1920x1080` | tcp/tcp | AV1 compatibility probe |
| `vp8_probe` | `from_file_011_fallback_video_vp8_fb9c7e866d__push_udp__pull_udp.pcap` | `v=vp8@320x240` | udp/udp | VP8 fallback probe |
| `vp9_probe` | `from_file_012_fallback_video_vp9_72cd0a041b__push_tcp__pull_udp.pcap` | `v=vp9@320x240` | tcp/udp | VP9 fallback probe |
| `h266_probe` | `from_file_014_chainsaw_man_04_vvc_1080p_aac_qpa0_qp20_ae3b4d9277__push_tcp__pull_tcp.pcap` | `v=h266@1920x1080;a=aac@ch2` | tcp/tcp | VVC compatibility probe |
| `high_bitrate_probe` | `from_file_016_hd_club_4k_chimei_inn_40mbps_b01f503846__push_tcp__pull_tcp.pcap` | `v=h264@3840x2160` | tcp/tcp | 4K/high-bitrate large payload probe |

## Flow 归类规则

生成工具必须解析 pcap 并按以下规则归类：

- 支持 pcap little/big endian global header。
- 支持 Linux cooked v2 linktype 276，建议同时支持 Ethernet linktype 1。
- 只保留 IPv4 TCP/UDP payload；非 IP、DNS、mDNS、DHCP、外部网络噪声全部跳过。
- TCP flow key 使用 `(src_ip, src_port, dst_ip, dst_port)`；`dst_port == 8554` 为 C2S，`src_port == 8554` 为 S2C。
- 对 RTSP TCP payload，保留原始 TCP payload record，同时额外解析 `$` interleaved frame 生成 `tcp_interleaved_rtp/rtcp` 逻辑记录供 core/PBT/fuzz 使用。
- UDP RTP/RTCP flow 不能只靠 payload 第一字节判断；必须优先从 RTSP SETUP request/response 的 `Transport` 头提取 `client_port`、`server_port`、`interleaved` 和 `mode`，再按端口和方向归类。
- 如果无法从控制面确定 UDP flow 角色，只能把该 UDP flow 标为 probe，不能进入标准强断言。

## 断言分层

| 输入类别 | core 断言 | module 断言 | pbt/fuzz 断言 |
| --- | --- | --- | --- |
| standard RTSP control | 必须解析到预期方法序列；CSeq、Session、Transport、Range 基本字段可读 | raw replay 后 module 能返回 2xx/预期 4xx，标准 publish/play 能建立 stream/tracks | 未扰动视图做同 core 强断言 |
| standard TCP interleaved RTP/RTCP | 必须解析到 `RtspEvent::InterleavedFrame`，RTP/RTCP payload 可被 core parser 解析 | TCP publish 后 player 能收到 interleaved RTP；RTCP RR/SR 不破坏会话 | 覆盖粘包、半包、截断、重复、相邻乱序 |
| standard UDP RTP/RTCP | `RtpPacket::parse` / `RtcpPacket::parse` 成功；同 SSRC sequence 基本单调 | UDP publish/play 可转发至少一个 RTP，RTCP SR/RR 基本路径可见 | 覆盖 UDP 丢包、乱序、重复、截断 datagram |
| compat probe | `Err` 可接受；不得 panic、不得无限循环、不得内存无界增长 | module 必须保持 Running，engine health live/ready；连接关闭可接受 | 只做 bounded robustness，不要求成功解码或播放 |
| transport faults | `Err` 可接受；输入处理次数受 record/datagram 上限约束 | 只对少量 fault replay 验证 module 健康度 | 作为主要 fuzz/PBT 扰动输入 |

## 具体任务

### A.1 固定 RTSP capture fixture 格式

- [x] 固定 `.rtspcap` magic、record_count、record header 和 `kind` 枚举。
- [x] 固定 manifest 字段、`role` 枚举和数值字段规则。
- [x] 明确单 fixture 默认上限 512 KiB，超过时只能按完整 record/datagram 前缀截取。
- [x] 明确 fixture 只保留传输 payload，不保留 IP/TCP/UDP 头。

### A.2 固定 pcap 样例集和跳过规则

- [x] 生成工具必须跳过 0 字节 pcap，并记录 `skipped_empty_pcap`。
- [x] 生成工具必须验证 pcap global header、endianness、linktype、packet header 和 captured length。
- [x] 生成工具必须支持 Linux cooked v2，当前非空样例已确认是该格式。
- [x] 首批标准样例固定为 H264 TCP、H264 UDP、H265 TCP、audio-only UDP。
- [x] 首批 probe 样例固定为 AV1、VP8、VP9、H266/VVC、4K/high-bitrate。

### A.3 固定标准/probe/fault 断言分层

- [x] 标准样例进入 core/module/PBT 强断言。
- [x] probe 样例进入 core/PBT/fuzz 的鲁棒性断言，module 只做健康度回归。
- [x] fault view 统一覆盖 TCP single buffer、original records、one-byte chunks、coalesced N、prefix truncated、duplicate record、swap adjacent、drop every Nth。
- [x] UDP/RTP fault view 统一覆盖 drop datagram、duplicate datagram、swap adjacent datagrams、reverse small window、truncate payload、RTP sequence reorder。

## 完成后检查

```bash
cargo fmt
cargo test -p cheetah-rtsp-pbt --test rtsp_capture_fixture_manifest
cargo test -p cheetah-rtsp-core capture
```
