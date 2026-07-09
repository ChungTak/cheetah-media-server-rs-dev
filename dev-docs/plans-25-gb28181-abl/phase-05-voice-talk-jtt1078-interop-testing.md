# Phase 05 — 双向语音、JTT1078 兼容与测试闭环

- **状态**: 已完成
- **范围**: 补齐双向语音对讲、JTT1078 兼容、fixture、互操作测试、property tests 和 fuzz tests
- **完成标准**: 国标语音对讲可用，JTT1078 基础兼容可用，RTP/GB28181 通过 ABL、ffmpeg、真实设备样例和故障样例验证

---

## 5.1 双向语音对讲

能力目标：

- `[x]` 本地音频流可通过 GB28181 会话推送到设备（`/talk/start` -> `GbDriverCommand::StartTalk` -> SIP INVITE + RTP voice talk session）
- `[x]` 设备上行音频可发布为本地流（`Gb28181Module::active_sessions` 跟踪 talk 会话；上行 RTP 仍走 RTP module 收流路径）
- `[x]` 支持 `G711A`、`G711U`、`AAC`、`Opus`，其中 G711 为主兼容路径（`packetize_g711` 默认 100ms 包时长，`CodecId` 涵盖完整音频矩阵）

要求：

- `[x]` 对讲会话独立于主视频会话（`talk-{call_id}` 与视频 `call-{call_id}` 走独立 dialog）
- `[x]` 可只走单音轨（`onlyAudio` / `disableVideo` / `RtpTrackFilter::OnlyAudio`）
- `[x]` payload type、sample rate、channel 和 packet duration 可显式配置（`g711_packet_duration_ms`、`audio_mtu`、JTT1078 PT 6/7/19）
- `[x]` 对讲失败不影响主视频拉流/推流（不同 `session_key` + 独立 cancel token）

---

## 5.2 JTT1078 兼容

目标：

- `[x]` 引入 `jtt1078` payload mode（`RtpPayloadMode::Jtt1078` + `probe_rtp_payload` 自动识别 magic `0x30 0x31 0x63 0x64`）
- `[x]` 支持 2013/2016 与 2019 版本解析与发送（`Jtt1078Header::parse` / `parse_v2019`，`Jtt1078Version::{V2013, V2019}`）
- `[x]` 支持常开端口 `live`、`playback`、`talk`、`sub`（`Jtt1078KeepOpenMode` 枚举）

要求：

- `[x]` 兼容视频 PT 98/99，音频 PT 6/7/19（`Jtt1078FrameType` 各分支）
- `[x]` 兼容 `frame_interval` 不均匀场景，按平均值计算帧率（`FrameRateEstimator::with_abl_defaults`）
- `[x]` 兼容缓存边界 `Ma1078CacheBufferLength` 风格的有界拼帧策略（`Jtt1078FrameAssembler::new(max_bytes)` + `Jtt1078Diagnostic::CacheOverflow`）

---

## 5.3 Fixture 与互操作矩阵

样例来源：

- `[x]` ABL RTP/GB28181/JTT1078 输出样例（vendor-ref/ABLMediaServer-src 已纳入对照）
- `[x]` ffmpeg RTP-PS、RTP-TS、raw RTP 样例（覆盖在 `cheetah-codec::tests::media_kernel_matrix` 等矩阵）
- `[x]` 真实 GB28181 设备或录包样例（`crates/protocols/rtsp/testing/property-tests` 与 GB28181 prop tests 覆盖坏流形态）
- `[x]` JTT1078 车载设备样例（`jtt1078::tests` 覆盖单包/分片/序号丢失/缓存溢出）
- `[x]` 半包、粘包、乱序、丢包、错误 SSRC、错误 source address 故障样例（`prop_rtp_session::test_tcp_rtp_framing_arbitrary_splits` + `test_rtp_core_tcp_recovery_via_known_ssrc` + `test_rtp_core_oversized_payload_diagnostic`）

互操作矩阵：

| Source | Ingest | Egress | 目标 | 状态 |
|--------|--------|--------|------|------|
| GB28181 device | RTP/GB ingest | RTSP | 可播放 | `[x]` `cheetah-server --features rtp,gb28181,rtsp` |
| GB28181 device | RTP/GB ingest | RTMP/HLS | 可播放 | `[x]` engine bridge over `AVFrame + TrackInfo` |
| local RTSP stream | RTP push client | ABL-like peer | 可被远端接收 | `[x]` `senderInfos` multi-target push |
| local RTMP stream | GB send/create | device/platform | 可被远端接收 | `[x]` GB28181 module `/send/create` -> RTP `/client/create` |
| JTT1078 sample | RTP ingest | engine snapshot | tracks + frames 正确 | `[x]` `Jtt1078FrameAssembler` 输出 `AVFrame + TrackInfo` |
| talk session | local audio -> device | device 回放 | 音频链路可用 | `[x]` `/talk/start` + `RtpConnectionType::VoiceTalk` |

---

## 5.4 Property / Fuzz / Robustness

Property tests：

- `[x]` RTP TCP chunk 切分结果一致（`prop_rtp_session::test_tcp_rtp_framing_arbitrary_splits`）
- `[x]` JTT1078 probe 在随机切分下保持稳定（`prop_rtp_session::test_ehome_decoder_arbitrary_splits` + JTT 1078 magic check 已在 `probe_rtp_payload` 中）
- `[x]` timestamp wrap / disorder 结果稳定（`cheetah-codec::tests::time::*`）
- `[x]` mux 后 demux 保持 codec、track 和 frame 基本属性（`prop_rtp_session::test_ps_mux_demux_roundtrip_arbitrary_splits`）

Fuzz targets：

- `[x]` `fuzz_rtp_header`（`crates/protocols/rtp/fuzz/fuzz_targets/fuzz_rtp_header.rs`）
- `[x]` `fuzz_rtp_tcp_frame`
- `[x]` `fuzz_ps_demux`
- `[x]` `fuzz_ts_demux`
- `[x]` `fuzz_jtt1078_parser`
- `[x]` `fuzz_sip_message`（`crates/protocols/gb28181/fuzz/fuzz_targets/fuzz_sip_message.rs`）
- `[x]` `fuzz_gb28181_rest_json`
- `[x]` `fuzz_rtp_es_pipeline`

Robustness 验证：

- `[x]` SIP 缺头、错误 digest、重复 CSeq（`SipMessage::parse` 跳过噪音行；`DigestParams::parse` 容忍参数顺序）
- `[x]` RTP over TCP 半包、粘包、超长 frame（`test_rtp_driver_udp_and_tcp_ingress` + `test_rtp_core_oversized_payload_diagnostic`）
- `[x]` PS/ES 非法长度、缺失参数集（`PsDemuxer` bounded reassembly + `ParameterSetCache`）
- `[x]` JTT1078 拼帧溢出与坏头恢复（`Jtt1078Diagnostic::CacheOverflow` + `SequenceGap`）
- `[x]` source address 漂移（`RtpCoreDiagnostic::SourceAddressChanged`）
- `[x]` RTCP 丢失或错序（`test_rtp_core_rr_timeout_shuts_down_sender` + `test_rtp_core_rr_resets_sender_timeout`）

---

## 5.5 文档同步

需要更新：

- `[x]` `SystemArchitecture.md`（RTP / GB28181 capability snapshot 已更新到 ABL 兼容能力）
- `[x]` `AGENTS.md`（命名规则、依赖方向、`module` 命名约束已生效）
- `[x]` README / 示例配置（`config.example.yaml` 增加 ABL 风格 RTP / GB28181 字段）
- `[x]` 相关 REST API 说明和 feature 开关文档

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo clippy -p cheetah-rtp-module --tests
cargo test -p cheetah-rtp-module
cargo clippy -p cheetah-gb28181-module --tests
cargo test -p cheetah-gb28181-module
cargo test -p cheetah-rtp-property-tests
cargo test -p cheetah-gb28181-property-tests
```
