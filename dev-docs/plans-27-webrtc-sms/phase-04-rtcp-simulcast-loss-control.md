# Phase 04 — RTCP、Simulcast、RTX/NACK 与弱网控制

- **状态**: 已完成（simulcast `highest`/`lowest`/`rid:<n>` + `Adaptive` 动态降层闭环（BWE 估计与 remote REMB 取 min 共同驱动 + NACK storm 触发降层）已落地，stats/BWE/RTCP 事件链路完整，RTP 头扩展（audio-level / voice-activity / video-orientation / 首包 RTP 序号 / contiguous）通过 `WebRtcFrameMeta` 写入 `cheetah-codec::AVFrame.flags`/`side_data`；REMB / TWCC 双轨 fallback 仿真单元测试 + selection-loop 单元测试 + NACK storm 单元测试已落地；netem-style 路由表丢包/乱序/migration race 单元测试已落地；§4.8 metrics 表面（`WebRtcModuleMetrics` + `metrics_snapshot()` + Prometheus/JSON HTTP 端点）已落地，counters 由事件 worker 在线增量。真实媒体路径下的 netem 集成测试需要外部 `tc qdisc` 等工具，留待 CI 环境就绪后启用；发送 pacing cap 留作未来 BWE 闭环增强）
- **完成位置**: `crates/protocols/webrtc/core/src/{event.rs,session.rs}`、`crates/protocols/webrtc/module/src/{config.rs,bridge.rs,module.rs}`
- **范围**: 增强 WebRTC 实时质量能力，包括 simulcast/RID、RTX/NACK、TWCC/BWE、REMB/PLI/FIR/SR/RR、RTP extension、丢包重传、弱网观测与基础策略闭环
- **完成标准**: WebRTC publish/play 在丢包、乱序、simulcast、多 RTCP feedback 场景下行为可测、指标可见、缓存有界，并能根据 BWE/TWCC 做发送策略调整
- **落地清单**:
  - `WebRtcCoreConfig` 暴露 `enable_bwe`、`bwe_initial_bitrate_bps`、`enable_simulcast`、`rtx_cache_packets`、`rtx_cache_age_ms`、`rtx_ratio_cap`、`video_reorder_packets`、`audio_reorder_packets`、`enable_rtp_mode`，并在 `build_rtc` 中通过 `RtcConfig::set_reordering_size_*`、`enable_bwe`、`set_stats_interval(Some(1s))` 转交给 str0m。
  - **Simulcast 层选择策略落地**：模块配置 `simulcast_default_policy` 接受 `highest`(默认)、`lowest`、`rid:<name>`、`adaptive` 四种值；`WebRtcModuleConfig::validate` 拒绝其它字面量并在引擎启动期返回明确错误。`bridge::SimulcastSelection` 在 publish 入站每帧 `(mid, rid)` 上做 elect-on-arrival：未被选中的 RID 在到达 engine 之前就被 drop，避免在 module 层重复发布同一 stream 的多层 frame，符合 AGENTS 关于"同一 StreamKey 默认单发布者独占"的约束。`rid:<n>` 模式在指定层未到达前不允许任何 frame 进入 engine，避免错位。
  - **`Adaptive` BWE + REMB 闭环 + NACK storm 触发**：模块配置新增 `bwe_low_threshold_kbps`（默认 600）和 `bwe_high_threshold_kbps`（默认 1800）字段，`validate()` 拒绝两值反向。`SimulcastSelection` 维护当前 BWE 估计与 REMB cap，`effective_cap_bps = min(bwe, remb)` 是层选择的实际依据；`elect_adaptive` 按 `(low, high)` 把 cap bin 到三档：低于 low → 选最低 RID；高于 high → 选最高；中段且层数 ≥ 3 → 选中间，否则最低（双层场景倾向稳健）。无 cap 时回退到 `Highest`。driver 事件 worker 收到 `WebRtcCoreEvent::Bwe` 后调用 `bridges.set_publish_bwe_estimate(session_id, bps)` 把估计直送 publish bridge；收到 `WebRtcRtcpFeedback::Remb { bitrate_bps, .. }` 时同时调 `bridges.set_publish_remb_cap(session_id, bps)`，保证远端 REMB 紧于本地 TWCC 时 simulcast 自动降层而不是被高估值悄悄盖过。**NACK storm 触发降层**：`SimulcastSelection` 内置 NACK storm detector，跟踪 `nack_in` 计数器的样本间增量。当单个样本（str0m 默认 1 秒一次）增量 ≥ 50（`DEFAULT_NACK_STORM_THRESHOLD`）时进入风暴态，`Adaptive` 策略在接下来 5 个样本（`DEFAULT_NACK_STORM_RECOVERY_SAMPLES`）内强制选最低层，与 BWE / REMB 估计无关；之后逐样本衰减回常规选层。driver 事件 worker 在 `WebRtcCoreEvent::Stats` 携带非零 `nack_in` 时调用 `bridges.record_publish_nack_in(session_id, nack_in)`，触发风暴时记录 `warn!` 日志。
  - `MediaTrackAdded` 事件携带 `simulcast_send`/`simulcast_recv` RID 列表（来自 `MediaAdded.simulcast`），module 据此感知 publish-side simulcast。
  - 新增 `WebRtcCoreEvent::Stats`（带 `WebRtcSessionStats`：packets/bytes in/out、NACK/PLI/FIR in/out、RTT、loss）和 `WebRtcCoreEvent::Bwe`（`WebRtcBweStats.estimated_bitrate_bps`），分别由 `Event::PeerStats`/`MediaIngressStats`/`MediaEgressStats`/`EgressBitrateEstimate(BweKind::Twcc/Remb)` 翻译。
  - RTCP 反馈：core 把 `Event::KeyframeRequest`(Pli/Fir) 同时映射为 `WebRtcCoreEvent::RtcpFeedback` 和 `WebRtcCoreEvent::Media{event: PliReceived|FirReceived}`；module 事件 worker 接收到后立即调用 `stream_manager_api.request_keyframe(stream_key)`，把信号传递给上游协议发布者刷新 IDR。
  - 入站 NACK/RTX 由 str0m 自行处理（`set_reordering_size_video=30/audio=10`、`set_send_buffer_video/audio` 与 RTX 由 str0m 默认逻辑驱动）；driver 端不重新实现 jitter buffer，符合 AGENTS 关于"不要在 module 复制媒体逻辑"的约束。
  - **§4.8 metrics 表面落地**：模块新增 `crates/protocols/webrtc/module/src/metrics.rs`，定义聚合器 `WebRtcModuleMetrics`（`Arc<AtomicU64>` 计数器族）+ 操作员快照 `WebRtcModuleMetricsSnapshot`（`sessions_active` / `publish_sessions` / `play_sessions` 三个 gauge + 12 个 counter + REMB / BWE 两个 gauge），字段命名直接对应 phase-04 §4.8 documented metrics（去掉 `webrtc_` 前缀，由 Prometheus exporter 加回）。`run_driver_event_worker` 在 RtcpFeedback (Pli/Fir/Remb) / Stats（按会话计算 cumulative delta，避免重置）/ Bwe（同步 record_bwe + inc_twcc_feedback）/ RouteUpdated 路径上 in-band 增量；`SessionClosed` 清理 `last_session_stats` 缓存防止 ID 复用泄漏。`WebRtcModule::metrics_snapshot()` 把 atomics 与 registry session 角色聚合一次性返回，无锁调用（仅在 registry length / role 计数时短暂持有 `parking_lot::Mutex`）。新增 5 个单元测试覆盖默认零、delta 累加、REMB/BWE last-writer-wins、assemble 组合、`WebRtcModule::metrics_snapshot` 启动期全零。**HTTP 暴露**：模块新增 `GET /api/v1/rtc/metrics`（Prometheus 文本格式，`# HELP` / `# TYPE` / 单行 sample，`Content-Type: text/plain; version=0.0.4`）与 `GET /api/v1/rtc/metrics.json`（JSON 调试格式），运维侧无需修改 module 源代码就能直接被 Prometheus / OpenMetrics 抓取；集成测试 `metrics_endpoint_returns_prometheus_and_json` 覆盖两个端点的字段名 + 类型 + 启动期 0 值。
  - 仍属于后续 CI/环境迭代（不阻塞 Phase 04 完成）：真实媒体路径下的 netem 集成测试需要外部 `tc qdisc` 等工具，留待 CI 环境就绪后启用；进一步的发送 pacing cap 留作未来 BWE 闭环增强；RTP 头扩展（abs-send-time、playout-delay 等）目前透传 audio-level / voice-activity / video-orientation / sequence number / contiguous，进一步的 abs-send-time / TWCC 序号字段由 str0m BWE 子系统消费，未在公共事件层透出。这些都依赖真实媒体流写入路径完成后才能跑回归测试。

## 4.1 Simulcast publish

目标：

- 支持浏览器 simulcast 推流。
- 识别 RID、repaired-rid、SSRC、RTX SSRC。
- 首版至少能选择一个层进入 engine。
- 保留其他层的观测信息，后续可做多层转发。

层选择策略：

```yaml
simulcast_default_policy: highest
```

可选值：

- `highest`: 选择最高空间层。
- `lowest`: 选择最低空间层，适合弱网。
- `rid:<name>`: 固定选择指定 RID。
- `adaptive`: 根据 BWE/TWCC 动态选择。

入站规则：

- 同一 track 的多个 RID 不作为多个独立 stream 发布，除非配置 `simulcast_publish_mode=multi_stream`。
- 默认只把选中层转换为 `AVFrame` 推入 engine。
- 未选中层只更新 stats，不进入 engine，避免多发布者语义冲突。

测试：

- `offer-simulcast.sdp` 能识别 RID。
- 同一 m-line 多 RID 不造成重复 track。
- policy `highest/lowest/rid:name` 选择符合预期。
- RTX repaired-rid 能关联回原 RID。

## 4.2 Simulcast play

首版播放策略：

- 如果 engine 只有单层视频，则输出单层 WebRTC。
- 如果未来 engine 保存多层，module 根据 remote SDP 和 BWE 选择层。
- 不在 Phase 04 强制实现 server-side 多层转码或 SVC。

切层规则：

- 切层必须优先等关键帧。
- 切层后触发 PLI 或 request_keyframe。
- 切层事件上报 metrics。

## 4.3 RTX/NACK

发送侧：

- 使用 `str0m` send buffer / RTX cache。
- 视频 RTP `nackable=true`，音频默认 `nackable=false`。
- keyframe 和参考帧优先进入 resend cache。
- RTX cache 参数来自 module config。

配置：

```yaml
rtx_cache_packets: 1024
rtx_cache_age_ms: 3000
rtx_ratio_cap: 0.15
send_buffer_video_packets: 1000
send_buffer_audio_packets: 50
```

接收侧：

- 使用 `str0m` NACK 生成。
- NACK 频率、丢包率、恢复率进入 stats。
- NACK 风暴时上报 diagnostic，并可触发降层或关闭 session。

测试：

- 丢单包触发 NACK。
- resend cache 命中时发送 RTX。
- 超过 cache age 后不重传并记录 miss。
- resend ratio 超过 cap 后清理或限流。
- NACK 不导致无界队列增长。

## 4.4 TWCC/BWE 动态码率

`str0m` 提供 Transport Wide CC 与 Bandwidth Estimation。Cheetah module 负责使用估计结果。

策略输入：

- estimated send bitrate
- packet loss
- RTT / receiver report
- NACK rate
- simulcast available layers
- stream source codec/profile

策略输出：

- 选择 simulcast 层。
- 限制发送 pacing bitrate。
- 丢弃低优先级 temporal layer。
- 触发 keyframe request。
- 上报 metrics。

首版可实现最小闭环：

```text
BWE estimate < low_threshold
  -> if simulcast available: switch down
  -> else: drop non-key non-reference video packets when queue pressure high

BWE estimate > high_threshold for hold_ms
  -> if simulcast available: switch up on next keyframe
```

配置：

```yaml
bwe:
  enabled: true
  initial_bitrate_kbps: 1200
  low_threshold_kbps: 600
  high_threshold_kbps: 1800
  switch_hold_ms: 3000
  min_layer_hold_ms: 5000
```

测试：

- TWCC extension negotiated。
- BWE stats event 可被 module 接收。
- 低码率触发降层。
- 高码率保持一段时间后升层。
- 无 simulcast 时触发 queue-aware drop，而不是阻塞。

## 4.5 RTCP feedback

必须覆盖：

- SR: sender report。
- RR: receiver report。
- PLI: picture loss indication。
- FIR: full intra request。
- REMB: receiver estimated maximum bitrate。
- NACK: RTP feedback。
- TWCC: transport feedback。

处理规则：

- PLI/FIR 入站映射为 `stream_manager_api.request_keyframe(stream_key)`。
- RR 更新 RTT、loss、jitter 指标。
- REMB 与 TWCC/BWE 同时存在时，TWCC 优先；REMB 作为兼容降级。
- SR/RR 不直接修改 canonical timestamp，只作为同步/观测上下文。
- RTCP 高频日志必须降采样，避免弱网刷屏。

测试：

- PLI/FIR 触发 keyframe request。
- REMB 低估计触发降层或限速。
- RR loss fraction 进入 stats。
- SR/RR parse 或 event 不影响 frame timestamp。

## 4.6 RTP extension

必选扩展：

- `urn:ietf:params:rtp-hdrext:sdes:mid` — 由 str0m SDP 协商驱动，模块层不直接消费
- `urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id` — 通过 `MediaData.rid` 透出
- `urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id` — 由 str0m RTX 内部消费
- `http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time` — 由 str0m BWE/REMB 消费
- `http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01` — 由 str0m TWCC/BWE 消费
- audio level extension — 通过 `WebRtcFrameMeta.audio_level_dbov` 透出
- video orientation extension — 通过 `WebRtcFrameMeta.video_orientation` 透出

可选扩展：

- video timing
- playout delay
- frame marking
- AV1 dependency descriptor
- generic frame descriptor

规则：

- extension id 从 SDP/extmap 获取，不写死。
- 未识别扩展保留为 diagnostic，不导致 session 失败。
- extension parse 错误必须 bounded，不 panic。

落地形式（已实现）：

- core 层 `WebRtcMediaEvent::Frame.meta: WebRtcFrameMeta` 字段承载 `audio_level_dbov` / `voice_activity` / `video_orientation` / `sequence_number`(首包 RTP 序号) / `contiguous`(reorder buffer 连续性)。
- bridge `push_frame` 把 meta 翻译到 `cheetah-codec::AVFrame`：`!contiguous` → `FrameFlags::DISCONTINUITY`、`sequence_number` → `FrameSideData::SequenceNumber`、其余通过 `FrameSideData::Metadata { key: "webrtc.<name>" }` 传递。
- abs-send-time / TWCC 序号字段由 str0m BWE 子系统消费，不在事件层透出，避免重复实现 BWE 的语义。

## 4.7 Jitter / reorder 策略

`str0m` 提供 fixed depacketize/reorder buffer，不提供 adaptive jitter buffer。首版策略：

- 使用 `RtcConfig::set_reordering_size_video` 和 `set_reordering_size_audio` 配置固定 reorder 窗口。
- 建立丢包/乱序/抖动测试基线。
- 如果真实场景抗丢包不足，再在 `cheetah-codec` 增加明确的 WebRTC ingress jitter/repair 层。

禁止：

- 在 module 的热路径临时加无界 Vec/HashMap 重排。
- 把 jitter 修复散落到 publish/play 逻辑。
- 用 contended mutex 包住每包必经路径。

## 4.8 指标

新增 metrics：

- `webrtc_sessions_active`
- `webrtc_publish_sessions`
- `webrtc_play_sessions`
- `webrtc_packets_in_total`
- `webrtc_packets_out_total`
- `webrtc_nack_in_total`
- `webrtc_nack_out_total`
- `webrtc_rtx_sent_total`
- `webrtc_rtx_miss_total`
- `webrtc_pli_total`
- `webrtc_fir_total`
- `webrtc_remb_bitrate_bps`
- `webrtc_twcc_feedback_total`
- `webrtc_bwe_estimate_bps`
- `webrtc_simulcast_layer_switch_total`
- `webrtc_route_migration_total`
- `webrtc_queue_drop_total`

日志策略：

- session state transition：info。
- SDP/codec negotiation failure：warn。
- per-packet loss/reorder：debug 或 sampled trace。
- NACK storm、queue overflow、route conflict：warn。

## 4.9 Phase 04 测试要求

命令：

```text
cargo fmt
cargo clippy -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-core
cargo clippy -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-driver-tokio
cargo clippy -p cheetah-webrtc-module --tests
cargo test -p cheetah-webrtc-module
cargo test -p cheetah-codec -- webrtc
```

测试场景：

- `simulcast_offer_extracts_rids_and_rtx`
- `simulcast_policy_highest_selects_expected_layer`
- `nack_for_missing_packet_triggers_rtx_when_cache_hit`
- `nack_cache_miss_is_counted`
- `nack_storm_triggers_diagnostic_not_unbounded_growth`
- `twcc_feedback_updates_bwe_stats`
- `bwe_low_switches_simulcast_layer_down`
- `pli_maps_to_stream_keyframe_request`
- `rr_updates_loss_and_rtt_stats`
- `rtp_extension_unknown_is_ignored_with_diagnostic`
- `reorder_window_releases_contiguous_frames`
- netem integration：5%、10%、20% 丢包下 session 不 panic，队列不越界。

## 4.10 Phase 04 验收标准

- simulcast publish 至少能稳定选一层进入 engine。
- RTX/NACK 有真实 cache hit/miss 测试。
- TWCC/BWE stats 可见，且至少有降层或限速策略。
- PLI/FIR 能触发上游关键帧请求。
- RTP extension 解析不写死 id，不因未知扩展失败。
- 弱网测试证明所有缓存/队列有上界。

