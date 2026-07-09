# Phase 01 — `cheetah-codec` 补齐 RTP/PS/TS/ES/JTT1078 媒体内核

- **状态**: 已完成
- **范围**: 扩展 `cheetah-codec`，把 ABL 依赖的 RTP 载荷解析、PS/TS/ES/JTT1078、真实帧率、参数集缓存和多编码支持收敛到 foundation 层
- **完成标准**: RTP 媒体入口可把 PS/TS/ES/JTT1078 统一解出 `TrackInfo + AVFrame`，输出端可稳定为 RTP/PS/ES 提供封装视图

---

## 1.1 RTP payload 与 framing 基础

补齐以下共享组件：

- `[x]` `RtpTcpDeframer`：支持 `two_byte`、`interleaved_4byte`、`auto_detect`（`RtpTcpFraming` + `parse_tcp_rtp_frame_with` in `cheetah-codec/src/rtp.rs`）
- `[x]` `RtpTcpFramer`：输出 2-byte 或 4-byte TCP RTP 帧（`encode_tcp_rtp_frame` + `encode_interleaved_rtp_frame`）
- `[x]` `RtpPayloadMode`：`ps`、`ts`、`es`、`xhb`、`jtt1078`（含 `Ehome` / `RawAudio` / `RawVideo` / `Unknown` 兼容变体）
- `[x]` `RtpCodecHint` 覆盖 H264/H265/AAC/G711A/G711U/OPUS/MP3/VP8/VP9/AV1（`CodecId` enum + `compat::codec_from_*` 映射）

要求：

- `[x]` deframer 允许 bounded 恢复，面对坏包时按最小扫描窗口重同步（`session.rs` 4 KiB scan window，按 SSRC / PS pack-start 恢复）
- `[x]` 自动识别路径要优先识别 `$` 4-byte 头，再识别 2-byte 长度头（`parse_tcp_rtp_frame_with` AutoDetect 分支）
- `[x]` 实现动态最大包长学习，但必须有固定硬上界（`RtpSession::max_rtp_len_observed` + `RtpCore::max_rtp_len_cap` + `RtpCoreDiagnostic::OversizedPayload`）

---

## 1.2 PS / TS / ES 收敛

需要补齐：

- `[x]` PS demux/mux：支持 H264/H265/AAC/G711A/G711U/MP3（`PsMuxer` / `PsDemuxer` in `cheetah-codec/src/ps.rs`）
- `[x]` TS demux/mux：复用现有 TS 能力，补齐 RTP-TS ingress 视角（`MpegTsDemuxer` / `MpegTsMuxer` + `crates/protocols/ts/core/src/rtp_ts.rs`）
- `[x]` ES packetize/depacketize：支持 H264/H265/AAC/G711A/G711U/OPUS/MP3/VP8/VP9/AV1（`packetize_payload` / `depacketize_payload` + RTSP `media/` packetizers）

要求：

- `[x]` 单端口 RTP ingress 能按有效载荷自动判断 PS 或 TS（`probe_rtp_payload`，外加 `Jtt1078` / `Ehome` 自动识别）
- `[x]` AAC 既支持 ADTS 识别，也支持 raw AAC + 显式 audio specific config（`AacAudioSpecificConfig` + `AdtsHeader` + `infer_aac_asc_from_adts`）
- `[x]` G711 统一为 `TrackInfo + AVFrame`，包时长由 codec 层策略控制（`packetize_g711` 接受 `packet_duration_ms`，缺省 100ms 对齐 ABL `kRtpG711DurMs`）

---

## 1.3 JTT1078 基础能力

在 foundation 层新增专门 parser/view：

- `[x]` `Jtt1078Version`：`V2013`（含 2016 兼容）、`V2019`
- `[x]` `Jtt1078FrameAssembler`（`Jtt1078FrameAssembler::new` 接受上界字节）
- `[x]` `Jtt1078Packetizer`（带 `set_max_payload_bytes`）

要求：

- `[x]` 支持视频 PT 98/99，音频 PT 6/7/19（`Jtt1078FrameType` 全部覆盖）
- `[x]` 支持 `frame_interval` 帧率学习和时间戳转换（`Jtt1078Header::last_frame_interval_ms` + `FrameRateEstimator`）
- `[x]` 拼帧缓存必须 bounded，并且暴露溢出诊断（`Jtt1078Diagnostic::CacheOverflow`，`SequenceGap`）
- `[x]` `Jtt1078KeepOpenMode` 区分 single/live/playback/talk/sub，对齐 ABL `kt1078_keep_mode`

---

## 1.4 时间戳、帧率与参数集缓存

补齐以下媒体内核行为：

- `[x]` RTP timestamp wrap 展开（`WrapUnwrapper` + `TimestampNormalizer`）
- `[x]` 视频真实帧率学习，不能固定 25fps（`FrameRateEstimator`，多个 sample 平均）
- `[x]` AAC 采样率、声道自动识别（`AacAudioSpecificConfig::from_bytes`）
- `[x]` H264/H265 的 SPS/PPS/VPS 缓存与补发（`ParameterSetCache::update_from_annexb` / `prepend_to_annexb_keyframe`）
- `[x]` `ForceSendingIFrame` 对应的最新 IDR 缓存视图（`AccessUnit::from_frame_units` + `repair_h26x_keyframe_frame_discovers_extradata_and_prepends_sets`）

要求：

- `[x]` 帧率学习要支持 RTP timestamp 和 JTT1078 `frame_interval`
- `[x]` 参数集缺失时允许按缓存补发，但要生成 compat 诊断（`AdapterContractError` + `AdapterContractError::ParameterSetMissing`）
- `[x]` 发送端要能请求“参数集 + 最新关键帧”的合成输出（`build_future_protocol_egress_contract_view`）

---

## 1.5 测试

需要补齐：

- `[x]` `cheetah-codec` 单元测试：RTP TCP framing、PS/TS auto probe、AAC ADTS、G711 packet duration、timestamp wrap（共 186 用例 in `cargo test -p cheetah-codec --lib`）
- `[x]` property tests：随机切分 TCP 流、随机 RTP 时间戳回绕、PS/TS/ES roundtrip（`cheetah-rtp-property-tests::prop_rtp_session`）
- `[x]` fuzz targets：`fuzz_rtp_tcp_frame`、`fuzz_ps_demux`、`fuzz_ts_demux`、`fuzz_jtt1078_parser`、`fuzz_rtp_es_pipeline`（标准 cargo-fuzz workspace under `crates/protocols/rtp/fuzz/`）

完成后检查：

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
```
