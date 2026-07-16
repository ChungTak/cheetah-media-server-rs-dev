# 发布证据报告 v0.1.0

基于 `dev-docs/903_api_completion_plan/15_release_evidence_template.md` 生成，记录当前 P0/P1 与 ZLM 任务的实际验证结果。

## 1. 候选版本

| 字段 | 值 |
| --- | --- |
| revision | `fad382796f1b104145812b2a165322e7795a2a9c` |
| build profile/features | `test` 与 `release --features media-control-full` |
| rustc/cargo | `rustc 1.94.1 (a7cd1e014 2026-06-30)`, `cargo 1.94.1` |
| lockfile hash | 以 CI `S0 - Print toolchain and lockfile hash` 输出为准 |
| platform | `x86_64-unknown-linux-gnu` / Ubuntu 22.04/24.04 |
| evidence location | PR #135..#172 CI 日志与制品 `cheetah-server-release` |

## 2. 任务证据

| Task ID | Owner | Revision | Command | Result | Artifact | Residual risk |
| --- | --- | --- | --- | --- | --- | --- |
| REL-01 | devin | main | `cargo fmt --check` + S0 CI | PASS | #135 | 无 |
| CAP-01..04 | devin | main | `cargo test -p cheetah-sdk` / `cheetah-engine` | PASS | #136..#139 | 无 |
| SEC-01..05 | devin | main | `cargo test -p cheetah-engine` / `cheetah-webhook-dispatcher` | PASS | #140..#142/#166/#167 | 无 |
| HTTP-01 | devin | main | `cargo test -p cheetah-media-module` | PASS | #143 | 无 |
| RTP-01..04 | devin | main | `cargo test -p cheetah-rtp-module` | PASS | #144..#147 | 无 |
| IMG-01..04 | devin | main | `cargo test -p cheetah-snapshot-module` + `cheetah-media-module` | PASS | #148..#151 | 无 |
| VOD-01..04 | devin | main | `cargo test -p cheetah-mp4-module` + `cheetah-media-module` | PASS | #152..#155 | 无 |
| PRX-01..05 | devin | main | `cargo test -p cheetah-proxy-module` | PASS | #156..#160 | 无 |
| EVT-01..05 | devin | main | `cargo test -p cheetah-engine` + `cheetah-webhook-dispatcher` | PASS | #161..#165 | 无 |
| ZLM-01..04 | devin | main | `cargo test -p cheetah-media-module` | PASS | #168..#171 | 无 |
| REL-02/04 | devin | #172 | S0..S5 GitHub Actions workflow | PASS | #172 制品 `cheetah-server-release` | REL-03 尚未实施 |
| REL-03 | - | - | 待补充并发取消/module restart 后资源泄漏观测 | N/A | - | 资源泄漏观测缺失 |
| SIG-01..06 | - | - | `cargo test -p cheetah-sdk --test signal_contracts` 仅覆盖 A 层 | PARTIAL | - | B 层 native HTTP 黑盒 runner 缺失 |

## 3. 能力证据

当前能力报告以 `cheetah-media-api` `MediaCapabilitySet` 与各 provider 注册为准。已通过的 operation 包括：

- `RtpApi`：创建/更新/停止 RTP session、UDP/TCP active/TCP passive。
- `PlaybackApi`：open/list/control/stop MP4 回放。
- `ImageEncodeApi`：MJPEG 关键帧编码、JPEG 独立解码。
- `RecordApi`：start/stop 录制并解析录制文件。
- `ProxyApi`：RTSP pull、RTMP push、SSRF allowlist。
- `WebhookAdminApi`：CRUD + test + 投递重试。
- `MediaAdmissionApi`：publish/play/proxy 前置准入。

运行服务器后 `/api/v1/media/capabilities` 与 active output endpoints 的快照需在 REL-03/SIG 完成后作为最终制品证据补充。

## 4. 兼容与信令矩阵

| Contract | Rust SDK | Native HTTP | Real media validation | Cleanup | Result |
| --- | --- | --- | --- | --- | --- |
| GB28181 media | #169/#170? (A 层 SDK) | 待补充 | PS/RTP parsed (A 层) | ports/tasks (A 层) | PARTIAL |
| ONVIF media | A 层通过 | 待补充 | RTSP frames/JPEG | proxy/files | PARTIAL |
| HomeKit media | A 层通过 | 待补充 | audio/video/RTP | subscriptions | PARTIAL |
| Matter media | A 层通过 | 待补充 | files/events | subscription | PARTIAL |

ZLM 兼容接口目录及 L0-L4 证据见 `11_zlm_interface_evidence_catalog.md`。

## 5. 门禁清单

- [x] 精确工具链、fmt、clippy、changed-crate tests 通过。
- [x] 共享层反向依赖和发布 profile 通过（S5 `media-control-full` release build）。
- [x] RTP UDP/TCP、更新、超时、端口回收通过。
- [x] JPEG 独立解码、文件物理删除通过。
- [x] MP4 playback、RTSP pull、RTMP push、FFmpeg executor 通过。
- [x] admission、Webhook 投递、资源授权、deadline、幂等通过。
- [x] native/兼容 HTTP 黑盒通过（ZLM-04 L3 已覆盖 getApiList/version/getMediaList）。
- [ ] 四类信令 A/B 合同通过（A 层通过，B 层 runner 缺失）。
- [ ] 并发取消、module restart、资源泄漏观测通过（REL-03 待实施）。
- [x] 发布阻断项逐项确认：未发现新增阻断项。

## 6. 失败与豁免

- 发布阻断项无豁免。
- REL-03 与 SIG-06 为当前剩余发布阻断风险；需在发布前完成。

## 7. 签署

| 角色 | 结论 | 姓名/时间 | 证据 |
| --- | --- | --- | --- |
| implementation | APPROVE (P0/P1/ZLM) | devin / 2026-07-15 | PR #135..#172 CI 全绿 |
| API compatibility | 待最终签署 | - | ZLM/SIG B 层完成后 |
| security | 待最终签署 | - | REL-03 完成后 |
| release | 待最终签署 | - | 全部门禁清单打勾后 |
