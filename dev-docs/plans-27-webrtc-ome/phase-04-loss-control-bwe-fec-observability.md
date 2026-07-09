# Phase 04: Loss Control BWE FEC Observability

- **状态**: 已完成
- **目标**: 补齐 OME 风格的 RTX/NACK、TransportCC/REMB、RED/ULPFEC、RTCP 反馈与观测面，使弱网恢复和运维指标更完整。

## 实现范围

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| RTX/NACK/TWCC/REMB 基础 | 已有/复用 | 本地已具备主要骨架与指标 |
| RED/ULPFEC SDP 与策略 | 已完成 | 新增 `enable_red_ulpfec`，默认对本地 SDP 按 media section 做 RED/ULPFEC payload 过滤，并级联移除指向被禁用 PT 的 RTX |
| abs-send-time/video-timing/playout-delay 观测 | 已完成 | core 识别 extmap 并通过 module session telemetry / `GET /session/{id}` 暴露 `rtp_extensions` operator surface |
| RTCP 细粒度统计 | 已完成 | 新增 per-session `rtcp_sr` / `rtcp_rr` / `rtcp_nack` 观测 |
| ABR 闭环验证 | 已完成 | 已补 OME/ZLM RID 质量序、Adaptive BWE/REMB cap、NACK storm 降级/恢复与 `webrtc_auto_abr` 启停回归 |

## 参考 OME 行为

OME 在播放侧同时使用：

- TransportCC 与 REMB。
- RTX 重传与发送记录。
- 可选 RED/ULPFEC。
- playlist/rendition 切换与带宽估计联动。
- 丰富的 RTCP/传输统计。

## 开发任务

### Task 01: RED/ULPFEC 协商与配置

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/core/src/*`
  - 修改: `crates/protocols/webrtc/module/src/config.rs`

验收点：

- 可显式配置是否在 SDP 中协商 RED/ULPFEC。
- 不支持的数据面路径要给出明确降级或禁用策略。

实现记录：

- module 配置新增 `enable_red_ulpfec`（默认 `false`）。
- core `sdp_compat` 新增 RED/ULPFEC payload 过滤函数；module 在本地 SDP 下发前执行兼容重写，默认按 media section 剔除 RED/ULPFEC 相关 `rtpmap/fmtp/rtcp-fb` 与 m-line payload id，并级联移除 `rtx apt=<RED/ULPFEC PT>`，避免留下孤儿 RTX payload。

### Task 02: BWE/ABR/RTCP 观测增强

- **状态**: 已完成

实现记录：

- `WebRtcSessionTelemetry` 新增 `rtcp_sr` / `rtcp_rr` / `rtcp_nack` 字段，并在 driver event worker 的 RTCP 事件路径累积更新。
- `GET /session/{id}` 追加以上字段，和既有 TWCC/REMB/RTX/BWE 共同构成观测面。
- `webrtc_auto_abr` 开关与 Phase 03 联动，允许在运行时关闭 BWE/REMB 驱动的自适应层切换，仅保留遥测记录。
- 已将 `RtpExtensionObserved` 纳入 session telemetry，并在 `GET /session/{id}` 的 `telemetry.rtp_extensions` 中暴露 extmap id、类型、原始 URI、规范 URI 与 direction，覆盖 abs-send-time、video-timing、playout-delay 等扩展的 operator surface。

### Task 03: 弱网恢复回归

- **状态**: 已完成

验收点：

- 区分 TWCC 与 REMB 的最后观测值。
- 能观测 RTX hit/miss、NACK、PLI/FIR、RR/SR 基本统计。
- session/metrics 接口可看出当前层、目标层、BWE/REMB 状态。

实现记录：

- 保留并扩展 `weak_network_nack_recovery` ignored 场景作为 netem/弱网入口。
- 增补 Phase 03/04 相关单测（SDP 过滤、playout-delay 注入、播放延迟观测、RR/SR/NACK 计数、BWE/REMB 自适应层选择、NACK storm 降级/恢复），覆盖关键恢复链路的本地回归。

验收点：

- 覆盖 UDP 丢包、重排、TCP fallback、BWE 降层、关键帧恢复。
- 明确哪些是模拟测试，哪些需要外部 netem/互操作环境。

## 测试计划

```powershell
cargo test -p cheetah-webrtc-module rtcp
cargo test -p cheetah-webrtc-module bwe
cargo clippy -p cheetah-webrtc-module
```
