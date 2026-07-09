# Phase 05: Interop Fixtures Fuzz Hardening

- **状态**: 已完成
- **目标**: 把 OME 文档样例、OvenRtcTester、浏览器/播放器互操作、弱网与 fuzz/property-test 纳入本地回归体系，确保 OME 对标行为可持续验证。

## 实现范围

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| 现有 interop harness | 已有/复用 | 已有 ignored tests、docker compose、runner 文档 |
| OME SDP/URL fixtures | 已完成 | 新增 OME play UDP/relay+RED-ULPFEC/H265 low-latency fixtures 并接入 core 回归 |
| OvenPlayer/OvenRtcTester | 已完成 | 已新增 OME WS 与 OvenRtcTester 的 ignored interop 骨架 |
| 弱网矩阵 | 已完成 | 已保留 `weak_network_nack_recovery` ignored/netem 入口，并由 Phase 03/04 单元回归覆盖 OME 关键弱网链路 |
| fuzz/property-test | 已完成 | 已补 OME URL、WebSocket signaling、transport literal、SDP/fuzz corpus 样例 |

## 参考 OME 行为

OME 仓库自带：

- 文档中的 URL/配置组合
- `misc/oven_rtc_tester`
- publish/play 自定义信令与 WHIP 双路径

这些都适合变成本地回归资产。

## 开发任务

### Task 01: OME fixtures 入库

- **状态**: 已完成
- **建议文件**:
  - 新增: `crates/protocols/webrtc/*/tests/fixtures/ome/*`

验收点：

- 至少覆盖 publish/play、UDP/TCP/relay、simulcast、H264/H265、low-latency 典型样例。

实现记录：

- 新增 core OME play fixtures：
  - `play_udp_offer.sdp`
  - `play_relay_red_ulpfec_offer.sdp`
  - `play_h265_low_latency_offer.sdp`
- `ome_sdp_fixtures.rs` 增加统一 canonicalize 回归，确保 fixtures 可稳定进入 compat 预处理链路。

### Task 02: OvenRtcTester 与浏览器互操作

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/tests/interop*.rs`
  - 修改: `dev-docs/plans-27-webrtc-ome/*`

验收点：

- 能记录最小复现命令、环境变量、artifact 路径。
- 至少有 ignored 测试骨架覆盖 OME 官方测试器或等效流程。

实现记录：

- 在 `module/tests/interop.rs` 新增：
  - `ome_ws_request_offer_smoke`（`WEBRTC_INTEROP_OME_WS_URL`）
  - `ome_oven_rtc_tester_smoke`（`WEBRTC_INTEROP_OME_TESTER_BIN`）
- 两个用例均沿用 interop harness，自动记录 artifact 与运行时配置。

### Task 03: fuzz/property-test 补强

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/testing/property-tests/*`
  - 修改: `crates/protocols/webrtc/fuzz/*`

验收点：

- OME URL/query/signaling message/SDP fragment 不 panic。
- malformed `transport`、relay-only、candidate 组合能稳定返回结构化错误。

实现记录：

- 新增 property tests：`testing/property-tests/tests/property_ome_compat.rs`
  - OME URL 解析 totality
  - transport literal roundtrip
  - OME WS message decoder panic-free
- 新增 fuzz corpus seeds：
  - `fuzz/corpus/fuzz_sdp_compat/ome_playout_delay_seed`
  - `fuzz/corpus/fuzz_zlm_rtc_url/ome_transport_seed`

## 测试计划

```powershell
cargo test -p cheetah-webrtc-property-tests
cargo test -p cheetah-webrtc-module --test interop -- --ignored
cd crates/protocols/webrtc/fuzz; cargo +nightly fuzz run fuzz_sdp_compat
```
