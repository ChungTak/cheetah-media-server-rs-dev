# Phase 03: 协议出站导出视图

- 状态：已完成
- 范围：RTMP/RTSP 出站从 canonical timeline 导出目标协议时间戳，消除入站为目标协议提前修正的问题。
- 完成标准：RTMP、RTSP 输出分别获得协议合法时间戳，bootstrap 和 pacing 只依赖 canonical 时间，不依赖源协议私有时间字段。

## 具体任务

### 3.1 RTMP egress 时间导出视图

- [x] RTMP timestamp 只由 canonical DTS 转毫秒得到。
- [x] RTMP CTS 只由 canonical PTS-DTS 转毫秒得到，负值按 FLV/RTMP 兼容策略处理。
- [x] late join timestamp rebase 只作用于 RTMP egress view，不回写 engine frame。
- [x] RTMP egress monotonic repair 只修目标封装输出，不改变 canonical timeline。

### 3.2 RTSP egress RTP timestamp 导出视图

- [x] RTSP 视频 RTP timestamp 优先由 canonical PTS 导出。
- [x] RTSP 音频 RTP timestamp 优先由 canonical DTS/PTS 中更适合音频连续性的时间导出。
- [x] 如果存在 source RTP timestamp 且同协议转发安全，可作为保真导出参考，但必须受 canonical discontinuity 与 pacing 约束。
- [x] TCP interleaved 与 UDP RTP 发送路径共享同一 egress timestamp view。

### 3.3 bootstrap pacing 与 source/canonical 分离

- [x] RingBuffer bootstrap 继续基于 canonical keyframe/discontinuity 选择 GOP。
- [x] 启动 pacing 使用 canonical media timestamp，不使用原始 RTP epoch 或 RTMP tag timestamp。
- [x] 首个 codec config 和首个 keyframe 立即发送，后续历史帧按 canonical 时间线 pacing。
- [x] 大幅 reset 后重建 pacing anchor，不跨 discontinuity 回放旧 GOP。

### 3.4 SRT/WebRTC egress 契约预留

- [x] SRT egress 明确只消费 canonical timeline 和 codec config view。
- [x] WebRTC egress 保留 RTP timestamp/RTCP feedback 的协议需求，但时间导出仍通过 codec egress view。
- [x] 未来协议不得直接读取 RTSP/RTMP module 私有 session 状态来生成媒体时间。

## 最新进展

- 2026-04-29：完成 3.4。`cheetah-codec` 新增 future protocol egress 显式契约导出 `build_future_protocol_egress_contract_view`，按协议产出 `SrtEgressContractView` 与 `WebRtcEgressContractView`：SRT 仅消费 canonical 派生的 `dts_ms/composition_time_ms` 与 `codec_config/parameter_set_replay`；WebRTC 消费 canonical 派生 `rtp_timestamp_ticks`、AU 边界、`codec_config/parameter_set_replay`，并保留 `random_access/discontinuity` 作为 RTP/RTCP 反馈决策输入。`EgressAdapterView` 同步补齐 `random_access/discontinuity` 只读字段，确保 future 协议无需读取 RTMP/RTSP module 私有 session 状态。新增回归测试：`srt_egress_contract_view_uses_canonical_timeline_and_codec_config`、`webrtc_egress_contract_view_uses_exported_rtp_timestamp_only`。验证通过：`cargo fmt`、`cargo clippy -p cheetah-codec`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module --test bridge_rtsp_rtmp`、`cargo test -p cheetah-rtsp-module --test bridge_rtmp_rtsp`。
- 2026-04-29：完成 3.3。RTMP/RTSP play 启动 pacing 明确绑定 canonical 时间线：新增首帧零延时回归测试 `play_start_pacing_first_frame_is_immediate_even_with_large_epoch_timestamp`（RTMP/RTSP 各一条），锁定“首个媒体帧不因随机 epoch 被延迟”。RTMP egress 引入 `should_reset_rtmp_egress_timeline_for_discontinuity`，仅在 `DISCONTINUITY` 且时间戳出现大幅向后回退时重置 rebase/clamp/mute 状态，避免跨段继承旧 timeline；对长时程正向跃迁保持连续（不误重置），确保 30 分钟级时间跨度回归仍通过。验证通过：`cargo fmt`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module`、`cargo clippy -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-module --test bridge_rtsp_rtmp`。
- 2026-04-29：完成 3.2。RTSP play egress 在统一时间视图下导出 RTP timestamp：`media_timestamp_priority(select_egress_timestamps)` 保持“视频优先 PTS、音频优先 DTS”；新增 `source_rtp_timestamp_for_egress`，在同协议安全 codec 且存在 source RTP side data 时作为保真参考；并在 `DISCONTINUITY` 或首包时重置单调修复锚点，保证 source 参考受 canonical 切段约束，不跨段续接旧时间线。TCP interleaved 与 UDP RTP 发送路径均复用同一 `raw_timestamp -> monotonic repair -> packetize` 逻辑。新增单测 `source_rtp_timestamp_for_egress_uses_supported_codec_only`，并通过 `cheetah-rtsp-module` 全量与 bridge 回归。
- 2026-04-29：完成 3.1。RTMP egress 去除 `FastPts` 模式对 `timestamp_ms` 的私有缩放（`dts_ms * 0.95`），统一由 canonical DTS 毫秒导出 RTMP timestamp；CTS 继续由 canonical `PTS-DTS` 导出并对负值执行 FLV/RTMP 兼容 clamp。保留播放链路的 egress-only 处理：`rebase_play_media_command_timestamp` 仅重置出站视图时间起点，`clamp_media_command_timestamp` 仅修目标封装单调性，不回写 engine `AVFrame`。新增回归测试 `h264_egress_fast_pts_mode_keeps_canonical_dts_timestamp`。验证通过：`cargo clippy -p cheetah-codec`、`cargo test -p cheetah-codec`、`cargo clippy -p cheetah-rtmp-module`、`cargo test -p cheetah-rtmp-module` 及 RTSP/RTMP 双向 bridge 测试。
- 2026-04-29：计划已创建，任务未开始。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo clippy -p cheetah-rtmp-module
cargo test -p cheetah-rtmp-module
cargo clippy -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-module --test bridge_rtsp_rtmp
cargo test -p cheetah-rtsp-module --test bridge_rtmp_rtsp
```
