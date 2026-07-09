# Phase 04 — RTCP、Simulcast、RTX/NACK、TWCC/BWE 与 Jitter

- **状态**: 部分完成（Phase 04 第一轮：core 已暴露 BWE / REMB / Stats / NACK / PLI / FIR / RTX 事件链路；module 落地 `WebRtcSessionTelemetry` 聚合器，按 session 合并 ingress/egress `Stats` 与 `Bwe` snapshot 与 REMB feedback；`/api/v1/rtc/session/{id}` JSON 暴露 `telemetry` 字段；`SimulcastPolicy::Adaptive` 已加入并通过 config validate；BWE 驱动的实时层切换、`MultiStream` 子流入 engine、jitter 矩阵、弱网 NACK/TWCC 算法测试留作后续小步迭代）

## 实现概览

本阶段补实时质量能力。目标不是只让 WebRTC 连上，而是在丢包、乱序、码率波动、simulcast 多层和真实浏览器反馈下保持可观测、可调度、可恢复。

## 已完成（Phase 04 第一轮）

- `crates/protocols/webrtc/module/src/session.rs` 新增 `WebRtcSessionTelemetry { bwe_estimated_bps, bwe_target_bps, remb_bitrate_bps, rtt_micros, loss_fraction_x10000, packets_in/out, bytes_in/out, nack_in/out, pli_in/out, fir_in/out, rtx_sent, rtx_miss, last_update_at }`，并提供 `merge_stats / merge_bwe / record_remb` 方法：与 ZLM `WebRtcTransportImp` 的 `onRtcp` / `onPeerStats` 类似，做累加而不是覆写，避免分方向 stats 互相清零。
- `WebRtcModuleSession` 嵌入 `telemetry` 字段，session 创建时初始化为零。
- `module.rs` event worker 新增 `WebRtcCoreEvent::Stats / Bwe / RtcpFeedback::Remb` 三条事件路由，按 session 写入 telemetry；REMB 与本地 BWE 估计区分存储，便于后续 BWE 驱动的层切换逻辑读两个独立来源。
- `WebRtcHttpService::handle_session_get` 在响应 JSON 中追加稳定字段集 `telemetry { bwe_estimated_bps / bwe_target_bps / remb_bitrate_bps / rtt_micros / loss_fraction_x10000 / packets_in / packets_out / bytes_in / bytes_out / nack_in / nack_out / pli_in / pli_out / fir_in / fir_out / rtx_sent / rtx_miss }`。空 session 也会返回完整字段集（counter 为 0、Option 为 null），保证 schema 不抖。
- `SimulcastPolicy::Adaptive` 加入枚举与 `parse` 解析器，`validate` 接受 `adaptive` 关键字，`elect_rid` 在缺乏 BWE 输入时退化为 `Highest`，保证向前兼容。
- 单元测试：3 条 telemetry merge（split ingress/egress、BWE Some/None 不互覆盖、REMB 与 BWE 独立）、`SimulcastPolicy::Adaptive` 解析与 validate、`accepts_adaptive_simulcast_policy_in_validate`。
- 集成测试：`session_get_includes_telemetry_skeleton` 校验 `/session/{id}` 始终返回完整 telemetry schema（17 个键），counter 字段必须是非负整数，Option 字段允许为 null 或整数。

## 已完成（Phase 04 第二轮）

- `SimulcastSelection::admit_with_upgrade` 方法：在每帧 admit 时检测 elected RID 是否从低层升级到高层（lexicographic 比较），返回 `(admitted, layer_upgraded)` 元组。
- `WebRtcPublishBridge::layer_upgrade_pending` 字段 + `take_layer_upgrade_pending()` 方法：当 adaptive 策略升层时设置 flag，module event worker 消费后请求 PLI/keyframe。
- `WebRtcBridgeRegistry::take_publish_layer_upgrade` 方法：registry 级别的 layer upgrade 消费入口。
- module event worker 在 `WebRtcCoreEvent::Media` 处理路径中，`push_publish_frame` 之后检查 `take_publish_layer_upgrade`，升层时通过 `stream_manager.request_keyframe` 请求 PLI，确保新层从可解码帧开始。

## 已完成（Phase 04 第三轮）

- `SimulcastPolicy::MultiStream` 枚举变体：在 multi-stream 模式下，所有 simulcast RID 都被 admit，每个 RID 作为独立子流发布到 engine。
- `SimulcastPolicy::parse` 支持 `multi-stream` / `multistream`（大小写不敏感）。
- `validate` 接受 `multi-stream` / `multistream` 关键字。
- `SimulcastSelection::admit_with_upgrade` 在 `MultiStream` 模式下直接返回 `(true, false)` 对所有 RID，不做层选举。
- `elect_rid` 在 `MultiStream` 模式下返回 `None`（信号给调用方使用多流路径）。
- `derive_multistream_key(base, rid) -> StreamKey` 公共函数：从基础 stream key 派生子流 key（如 `live/cam` + `h` → `live/cam@rid:h`）。
- 单元测试 4 条：`multistream_admits_all_rids`、`derive_multistream_key_appends_rid_suffix`、`derive_multistream_key_handles_complex_path`、`accepts_multi_stream_simulcast_policy_in_validate`。
- config parse 测试扩展：覆盖 `multi-stream` / `multistream` / `Multi-Stream` 三种写法。

## 已完成（Phase 04 第四轮）

- NACK 窗口测试矩阵扩展：4 条新单元测试覆盖 `SimulcastSelection` 的 NACK storm detector 边界条件。
  - `nack_storm_repeated_burst_resets_recovery_window`：第二次 burst 在 recovery 窗口内重置窗口，policy 持续 pin 在 lowest。
  - `nack_storm_does_not_trip_on_count_decrease`：cumulative NACK 计数减少时（session 重启 / counter reset）不误触发。
  - `nack_storm_trips_at_exact_threshold`：边界值 delta=50 触发。
  - `nack_storm_does_not_trip_below_threshold`：边界值 delta=49 不触发。

## 已完成（Phase 04 第五轮）

- TWCC feedback counter 测试矩阵：3 条新单元测试覆盖 `WebRtcModuleMetrics` 的 TWCC / RTCP feedback counter 边界条件。
  - `twcc_feedback_counter_increments_per_bwe_event`：模拟 20 个 BWE 事件（str0m TWCC 触发阈值），断言计数器精确为 20。
  - `twcc_feedback_counter_is_monotonic`：验证 TWCC 计数器单调递增。
  - `rtcp_counters_are_independent`：验证 PLI/FIR/REMB/TWCC 计数器互不影响。

## 后续小步迭代

- jitter / reorder 弱网测试矩阵（1/5/10/20% loss、burst、reorder=2/8/32、TWCC seq 回绕）：依赖外部 `tc netem` 工具或测试夹具，作为 ignored 测试矩阵记录可复现命令。


## 4.1 Simulcast publish

支持来源：

- RID RTP extension。
- repaired RID RTP extension。
- SDP `a=rid` + `a=simulcast`。
- SDP `a=ssrc-group:SIM`。
- Firefox SDP 提前绑定 SSRC/RID。
- SDP munging 无 RID 时按 SSRC 顺序生成 `r0/r1/r2`。

策略：

- `highest`：选择最高层。
- `lowest`：选择最低层。
- `rid:<name>`：固定 RID。
- `adaptive`：由 BWE 和 loss 自动选择层。
- `multi-stream`：显式生成子流。

## 4.2 Simulcast play

播放侧根据 BWE、loss、RTT 和订阅者能力选择输出层：

- 带宽下降时先降层，再丢 delta frame。
- 带宽恢复时等待关键帧再升层。
- 切层后主动请求 PLI。
- RID 不存在时 fallback 到最近可用层并输出诊断。

## 4.3 RTX / NACK

参考 ZLM `NackContext` 和 `NackList`：

- `nack_max_size`：丢包状态上限。
- `nack_max_ms`：丢包状态保留时长。
- `nack_max_count`：单包最大 NACK 次数。
- `nack_interval_rtt_ratio`：重发 NACK 间隔为 RTT 倍数。
- `nack_rtp_size` / `nack_audio_rtp_size`：单次 NACK bitmask 范围。

发送侧：

- `rtx_cache_packets` 和 `rtx_cache_age_ms` 必须有上界。
- 命中 NACK 后优先发 RTX；远端无 RTX SSRC 时按协商使用原 SSRC。
- RTX miss 输出统计，不阻塞新包。

接收侧：

- 序号回绕、乱序、重复包、RTX 包都要正确更新状态。
- NACK 生成不能为每个包都分配大量内存。
- 音频 NACK 窗口小于视频。

## 4.4 TWCC / BWE

参考 ZLM `TwccContext`：

- 最多 20 个 transport-wide seq 触发一次 TWCC。
- 或最大 256ms 触发一次 TWCC。
- 缺包、乱序、跨 16-bit 回绕要能编码。

BWE 策略：

- core 暴露 `Twcc` 和 `Remb` 估计。
- module 维护 per session 发送预算。
- 低于阈值先降 simulcast 层。
- 无可降层时丢弃 delta frame，保留 keyframe、RTCP、ICE、DTLS。
- BWE 恢复后请求关键帧再升层。

## 4.5 RTCP feedback

必须处理：

- PLI：请求上游关键帧。
- FIR：请求上游关键帧并记录 FIR seq。
- NACK：触发 RTX cache 查找。
- REMB：更新接收方估计码率。
- TWCC：更新 BWE。
- SR/RR：更新 RTT、jitter、loss、NTP/RTP 映射。
- SDES/BYE：更新状态或关闭。

未知 RTCP：

- 保留 packet type、fmt、size 诊断。
- 不 panic，不关闭会话，除非超过错误阈值。

## 4.6 Jitter / reorder

最小策略：

- 接收侧基于 `str0m` reorder 能力，补本项目观测指标。
- 如需本地 jitter buffer，放在 codec/driver 明确边界，不进入 module。
- 所有 reorder window、deadline、buffer bytes 必须有上界。
- 过期 delta frame 可丢弃，关键帧和配置帧优先。

测试场景：

- 1%、5%、10%、20% 随机丢包。
- 连续 burst loss。
- 乱序 2、8、32 包。
- RTP seq 16-bit 回绕。
- RTX 包先于原包或晚于 deadline。
- TWCC seq 回绕。

## 4.7 指标

每个 session 输出：

- `packet_loss_in/out`
- `jitter_ms`
- `rtt_ms`
- `bwe_bps`
- `nack_in/out`
- `rtx_hit/miss`
- `pli/fir/remb/twcc/sr/rr`
- `simulcast_active_rid`
- `layer_switch_count`
- `delta_frame_drop_count`
- `jitter_buffer_delay_ms`

## 4.8 测试要求

运行：

```powershell
cargo test -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-property-tests
```

新增测试：

- ZLM `test_rtcp_nack.cpp` 等价的 Rust NACK 序列测试。
- TWCC 20 包触发和 256ms 触发。
- REMB 码率更新事件。
- PLI/FIR 路由到 engine keyframe request。
- Simulcast RID fallback。
- BWE 降层和恢复升层。
- 弱网矩阵 ignored/manual 测试脚本。

## 完成后检查

- 所有弱网缓存有上界。
- RTCP 事件链路从 core 到 module 到 metrics 完整。
- module 不包含私有 jitter buffer 或 NALU 修复实现。

