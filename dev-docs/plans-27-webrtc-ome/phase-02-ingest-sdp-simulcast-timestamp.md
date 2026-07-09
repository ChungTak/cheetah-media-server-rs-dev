# Phase 02: Ingest SDP Simulcast Timestamp

- **状态**: 已完成
- **目标**: 补齐 OME 发布侧 SDP、payload、RID/simulcast、CompositionTime 与 RTCP-SR 时间戳兼容，使浏览器与 OME 风格推流端更稳定进入本地引擎。

## 实现范围

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| 现有 SDP 兼容层 | 已有/复用 | 复用 `core/src/sdp_compat.rs`、fixture 体系 |
| H264/H265/VP8/Opus publish 协商 | 已完成 | 已新增 OME publish simulcast/H265 descriptor fixtures，并复用既有浏览器/SMS/ZLM Opus/H264/H265 回归覆盖 offer/answer 兼容路径 |
| RID/simulcast 兼容 | 已完成 | 已将 OME/ZLM `q/h/f` 质量序固定为 `q < h < f`，并覆盖 Highest/Lowest/Adaptive/REMB/NACK storm 选择 |
| CompositionTime/CTS | 已完成 | OME fixture 覆盖 `toffset`/`video-timing`，ingress fast-start 模式保持 canonical PTS/DTS 单调 |
| RTCP-SR timestamp 模式 | 已完成 | 已新增 `rtcp_based_timestamp` 配置，默认 fast-start 归零，开启后保留 RTP epoch |

## 参考 OME 行为

OME 在 ingest 侧强调：

- `extmap-allow-mixed`
- H264/H265/VP8/Opus 常见 payload/rtcp-fb/fmtp 组合
- simulcast RID 与 recv layer 处理
- `CompositionTime` 扩展与 B 帧 DTS 重排
- `RtcpBasedTimestamp=false/true` 两种时钟模型

## 开发任务

### Task 01: 对齐 OME SDP fixtures

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/core/tests/*`
  - 新增: `crates/protocols/webrtc/core/tests/fixtures/ome/*`

验收点：

- OME publish offer/answer fixtures 能被当前 core 接受或给出结构化 compat 诊断。
- 覆盖 H264、H265、VP8、Opus、simulcast、`extmap-allow-mixed`。

实现记录：

- 已新增 `core/tests/fixtures/ome/publish_simulcast_offer.sdp`，覆盖 Opus、VP8、H264、RID simulcast、`extmap-allow-mixed`、`toffset`、`video-timing`，并验证 `AcceptOffer` 可生成 answer。
- 已新增 `core/tests/fixtures/ome/publish_h265_descriptor.sdp`，覆盖 H265 payload/fmtp descriptor，进入 `preprocess_remote_sdp` 诊断入口并保持 canonical CRLF。

### Task 02: Simulcast 与 RID 对齐

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/core/src/session.rs`
  - 修改: `crates/protocols/webrtc/module/src/bridge.rs`

验收点：

- OME 风格 RID/simulcast offer 能建立稳定层标识。
- 与现有 `Adaptive` / `MultiStream` 策略不冲突。

实现记录：

- 已调整 module 侧 simulcast RID election：`q`/`quarter`/`low` 为低层，`h`/`half`/`mid` 为中层，`f`/`full`/`high` 为高层；未知 RID 保持原有字典序回退。
- 已同步 Highest、Lowest、Adaptive、REMB cap、NACK storm 降级/恢复测试，`MultiStream` 继续全层透传。

### Task 03: CTS 与 RTCP-SR 时间戳模式

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/foundation/cheetah-codec/src/*`
  - 修改: `crates/protocols/webrtc/module/src/bridge.rs`
  - 修改: `crates/protocols/webrtc/module/src/config.rs`

验收点：

- `rtcp_based_timestamp=false` 保持快速起播。
- `rtcp_based_timestamp=true` 保留 RTP timestamp epoch，为后续 RTCP-SR wall-clock 对齐提供输入。
- H264/H265 含 CTS/B 帧时不会破坏 canonical timeline。

实现记录：

- 已新增 module 配置 `rtcp_based_timestamp`，默认 `false`。
- 默认 fast-start 模式按 `(mid, first_rtp_timestamp)` 建立零点，输出零基准 `AVFrame.pts/dts/pts_us/dts_us`。
- `rtcp_based_timestamp=true` 时保留原始 RTP timestamp epoch，为后续 RTCP-SR wall-clock 对齐保留 sender epoch。

## 测试计划

```powershell
cargo test -p cheetah-webrtc-core ome
cargo test -p cheetah-codec webrtc
cargo clippy -p cheetah-webrtc-core
```
