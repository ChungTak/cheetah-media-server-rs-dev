# Phase 03: 服务端推流、拉流播放与媒体鲁棒性

- 状态：计划中
- 范围：完善 RTSP server 的 ANNOUNCE/RECORD 推流 ingest、DESCRIBE/PLAY 播放 egress、RTP/RTCP、RTP reorder、PS/codec 兼容和跨协议桥接质量。
- 完成标准：真实客户端通过 UDP/TCP/HTTP tunnel 推流后能进入 engine，被 RTSP/RTMP/HTTP-FLV 播放；本地 engine stream 可通过 UDP/TCP/HTTP tunnel/multicast 播放；乱序、丢包、参数集缺失、时间戳回绕不会破坏 module 健康。

## 目标文件与模块

重点修改：

```text
crates/foundation/cheetah-codec/src/rtp.rs
crates/foundation/cheetah-codec/src/ps.rs
crates/foundation/cheetah-codec/src/video.rs
crates/foundation/cheetah-codec/src/track.rs
crates/protocols/rtsp/module/src/media.rs
crates/protocols/rtsp/module/src/sdp.rs
crates/protocols/rtsp/module/src/session.rs
crates/protocols/rtsp/module/src/module/publish.rs
crates/protocols/rtsp/module/src/module/play.rs
crates/protocols/rtsp/module/src/module/cleanup.rs
crates/protocols/rtsp/module/src/module/response.rs
```

建议新增：

```text
crates/foundation/cheetah-codec/src/rtp_reorder.rs
crates/protocols/rtsp/module/src/media/packetize.rs
crates/protocols/rtsp/module/src/media/depacketize.rs
crates/protocols/rtsp/module/src/media/rtcp.rs
crates/protocols/rtsp/module/src/media/ps_compat.rs
crates/protocols/rtsp/module/tests/server_publish_play_matrix.rs
crates/protocols/rtsp/module/tests/server_compat_sdp.rs
```

如果不拆 `media.rs`，新增逻辑前应先按职责拆分。当前 `media.rs` 已明显很大，继续膨胀会影响可维护性。

## Publish Ingest 目标

支持流程：

```text
OPTIONS
ANNOUNCE rtsp://host/app/stream + SDP
SETUP track 0 Transport: UDP/TCP/HTTP tunnel
SETUP track 1 Transport: UDP/TCP/HTTP tunnel
RECORD
RTP/RTCP media
PAUSE
RECORD
TEARDOWN
```

关键行为：

- ANNOUNCE 成功后立即 acquire publisher lease，失败返回 403/406，不创建半状态。
- SDP 解析出的 tracks 先 `sink.update_tracks`，后续从 RTP 中发现参数集或 AAC ASC 时再刷新 tracks。
- RECORD 前 RTP 不进入 engine；RECORD 后 ingest。
- PAUSE 后 RTP 不进入 engine，但 session、UDP socket、interleaved mapping 保留。
- RECORD 再次调用恢复 ingest，RTCP RR continuity 保持。
- 连接关闭、TEARDOWN、module stop 必须 flush/cleanup 并 release publisher lease。

## Play Egress 目标

支持流程：

```text
OPTIONS
DESCRIBE rtsp://host/app/stream
SETUP selected tracks Transport: UDP/TCP/HTTP tunnel/multicast
PLAY Range: npt=0-
RTP/RTCP media
PAUSE
PLAY
TEARDOWN
```

关键行为：

- DESCRIBE 源不存在时可按配置等待 `play_wait_source_timeout_ms`，超时返回 404；默认保持当前快速 404 可配置。
- SETUP 可选择部分 tracks；PLAY 只输出已 SETUP tracks。
- PLAY response 的 RTP-Info 包含每个 selected track 的 url、seq、rtptime。
- 有视频时默认从关键帧起播；audio-only 不等待关键帧。
- packetizer 输出严格受 MTU/interleaved length 约束。
- egress pacing 使用 runtime time，不在 core 中调用系统时间。
- PAUSE 后保留 seq/ssrc/rtcp state；PLAY 恢复后不重置 seq。

## RTP Reorder / Jitter

参考 SMS `RtpSort`，但实现应有更明确边界：

```rust
pub struct RtpReorderBuffer {
    pub max_packets: usize,
    pub max_delay_ms: u64,
}
```

行为：

- 以 SSRC + payload type + track_id 为 key。
- 支持 sequence wrap。
- 窗口满或超时后释放可交付包。
- 重复包丢弃。
- 丢包计数进入 RTCP RR 和观测日志。
- buffer 大小、每 track 总缓存 bytes 必须有上限。

优先放到 `cheetah-codec`，因为 WebRTC/GB28181/RTP raw ingest 也会需要同类能力。

## RTCP 目标

Publish side：

- 收到 SR 记录 NTP/RTP mapping。
- 按间隔发送 RR + SDES。
- BYE 后停止 ingest 并可触发 session cleanup。

Play side：

- 按间隔发送 SR + SDES。
- TEARDOWN 发送 BYE。
- 收到 RR 更新 packet loss/jitter 观测，但不影响热路径。

所有 RTCP 包使用 `cheetah-rtsp-core` / `cheetah-codec` 的 bounded parser/builder，不手写裸数组散落在 module。

## PS/MP2P 兼容

simple-media-server 支持 `RtspPsMediaSource` 和 MP2P/PS 变体。本地已有 `cheetah-codec::ps` 基础 PES/PS parser，但 RTSP media path 还没完整接入。

首版处理策略：

- SDP 中 `m=video ... MP2P`、`rtpmap: MP2P/90000` 或 `payload type` 显式为 PS 时，TrackInfo 可先标记为 private PS ingest track。
- RTP payload 中的 PS/PES 由 `cheetah-codec::ps` bounded demux 拆出 H264/H265/AAC 等 ES。
- 如果无法识别 ES，作为 compat probe 保持 bounded，不进入 engine。
- 不在 RTSP module 中手写完整 PS demux；缺能力先补 `cheetah-codec::ps`。

## Codec 兼容

必须保持现有支持：

- Video：H264、H265、H266、AV1、VP8、VP9。
- Audio：AAC MPEG4-GENERIC、AAC LATM、Opus、ADPCM、G711A、G711U、MP3。

新增兼容候选：

- JPEG：仅当 `cheetah-codec` 新增 CodecId/JPEG frame model 后支持，否则作为 probe。
- AC3/G722/G723：仅当 foundation codec model 明确支持后进入强行为断言。
- Vendor private payload：默认 probe，bounded ignore。

## 具体任务

### 3.1 完善服务端 publish ingest

- [x] 拆分 `media.rs` 中 publish depacketize、packetize、rtcp、ps compat 逻辑，降低中心文件体积。（已拆分到 `media/depacketize.rs`、`media/packetize.rs`、`media/rtcp.rs`、`media/ps_compat.rs`）
- [x] RECORD 前 RTP 明确丢弃并计数，不进入 engine。
- [x] PAUSE/RECORD 恢复保持 UDP/TCP/HTTP tunnel transport 和 RTCP continuity。
- [x] 将 RTP reorder buffer 接入 UDP ingest，TCP interleaved 默认不排序或使用较小窗口。
- [x] 从 RTP 中发现 H26x 参数集、AAC ASC、AV1 sequence header 后更新 TrackInfo。
- [x] 增加 PS/MP2P bounded ingest 路径和 probe 测试。

### 3.2 完善服务端 play egress

- [x] DESCRIBE 支持可配置等待源上线。（新增 `rtsp.play_wait_source_timeout_ms`，默认 0 保持快速 404）
- [x] PLAY response 的 RTP-Info 使用真实 seq/rtptime。（跨 PAUSE/PLAY 持续回写运行态 `play_tracks` 的 seq/rtptime；新 PLAY 保留 RTP 连续性并重置 RTCP 首包标记）
- [x] egress packetizer 对 TCP/UDP/multicast 使用正确 MTU，HTTP tunnel 复用 TCP interleaved framing。（`play_packet_mtu` 对 TCP 使用 interleaved u16 长度上界，对 UDP/multicast 使用 `rtp_mtu`；新增 `http_tunnel_play_uses_tcp_interleaved_mtu_not_udp_rtp_mtu` 回归测试）
- [x] 对 selected tracks 独立 keyframe gate，避免音频永远被视频 gate 阻塞。（改为按 track 维度 gate：音频 track 可先发，视频 track 各自等待自身关键帧解锁；补 `play_start_gate_allows_audio_before_video_keyframe` 与 `play_start_gate_is_independent_per_selected_video_track` 回归测试）
- [x] RTCP SR/SDES/BYE 统一通过 helper 发送。（新增 `send_play_rtcp_packet`，统一构建与发送 SR/SDES/BYE，并对 build/send 错误分型处理）
- [x] 增加 UDP/TCP/HTTP/multicast server play matrix 测试。（新增 `server_publish_play_matrix.rs`，覆盖同一 stream 的四类 PLAY 传输矩阵）

### 3.3 下沉 RTP reorder/PS/compat 热路径

- [x] 在 `cheetah-codec` 增加 `RtpReorderBuffer` 或等价 bounded helper。（新增 `rtp_reorder.rs` 与单测；RTSP publish UDP ingest 重排接入 codec helper）
- [x] 补 `cheetah-codec::ps` 对实际 RTSP PS RTP payload 的 bounded demux 测试。（新增 `PsPacket::parse_bounded` 与 RTP payload 边界/截断/上界测试，并接入 RTSP MP2P probe）
- [x] 将 H26x 参数集补发、时间戳回绕、DTS/PTS normalize 继续集中在 `cheetah-codec`。（新增 `cheetah-codec::ingress` helper 统一 RTP ingress 时间戳 normalize 策略/步长；`ParameterSetCache::repair_h26x_keyframe_frame` 统一参数集发现+关键帧补发；RTSP publish 侧删除重复实现并改用 codec helper）
- [x] 对 unsupported codec 记录 sampled warn 并跳帧，不 panic、不关闭 session。（publish ingest 新增 unsupported codec 采样告警与按 track 计数，未知 codec 直接跳帧不进入时钟/解包热路径；补 `ingest_publish_rtp_payload_unsupported_codec_track_skips_ingest_with_count` 回归测试）
- [x] 增加真实设备兼容回归：payload type fallback、missing rtpmap、missing fmtp、absolute control、bad marker。（SDP：新增 `fmtp` 无 `rtpmap` 下的 codec+PT fallback；`normalize_control` 支持 absolute control URI + query 归一；补 `parses_h264_track_without_fmtp_and_with_absolute_control_uri`、`infers_video_codec_from_fmtp_without_rtpmap_and_fallbacks_payload_type`。RTP：补 `depacketize_h265_access_unit_bad_marker_drop_recovers_on_following_access_unit` 确保 bad marker 丢帧后可恢复且不阻塞会话）

## 测试要求

- 涉及时间戳、重排、参数集补发、PS demux 必须补单元测试。
- 标准 H264/AAC、H265/AAC、audio-only 样例必须做强断言。
- AV1/VP9/H266/PS/high-bitrate/vendor private 可先做 compat probe，断言健康和有界。
- 跨协议测试至少覆盖 RTSP publish -> RTMP play、RTMP publish -> RTSP play、RTSP publish -> HTTP-FLV play。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec rtp
cargo test -p cheetah-codec ps
cargo clippy -p cheetah-rtsp-module --tests
cargo test -p cheetah-rtsp-module publish_record
cargo test -p cheetah-rtsp-module play_pause
cargo test -p cheetah-rtsp-module bridge_rtsp_rtmp
cargo test -p cheetah-rtsp-module bridge_rtmp_rtsp
cargo test -p cheetah-rtsp-module server_publish_play_matrix
```
