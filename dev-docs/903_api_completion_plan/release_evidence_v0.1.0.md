# 发布证据报告 v0.1.0

基于 `dev-docs/903_api_completion_plan/15_release_evidence_template.md` 生成，记录当前 P0/P1/P2 所有任务的实际验证结果。

## 1. 候选版本

| 字段 | 值 |
| --- | --- |
| revision | `23c4dab715a667bfd783cb0be24067818f61f009` |
| build profile/features | `test` 与 `release --features media-control-full` |
| rustc/cargo | `rustc 1.94.1 (e408947bf 2026-03-25)`, `cargo 1.94.1` |
| lockfile hash | `d332e85be37f7228e865028831727f3c187ac1306bb62c1c1b661da537a9efb2`（以 CI `S0 - Print toolchain and lockfile hash` 输出为准） |
| platform | `x86_64-unknown-linux-gnu` / Ubuntu 22.04/24.04 |
| evidence location | PR #135..#179 CI 日志与制品 `cheetah-server-release` |

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
| REL-02/04 | devin | main | S0..S5 GitHub Actions workflow | PASS | #172 制品 `cheetah-server-release` | 无 |
| REL-03 | devin | main | `cargo test -p cheetah-engine --test resource_leak -- --test-threads=1` | PASS | #174 | 无 |
| SIG-06 | devin | main | `cargo test -p cheetah-server --test signal_blackbox -- --test-threads=1` | PASS | #175 | 无 |
| SIG-01..05 | devin | main | `cargo test -p cheetah-server --features proxy-rtsp,snapshot,record --test signal_blackbox -- --test-threads=1` / `cargo test -p cheetah-server --features rtp,record,snapshot --test signal_blackbox -- --test-threads=1` | PASS | #176..#179 | 无 |

## 3. 能力证据

当前能力报告以 `cheetah-media-api` `MediaCapabilitySet` 与各 provider 注册为准。已通过的 operation 包括：

- `RtpApi`：创建/更新/停止 RTP session、UDP/TCP active/TCP passive。
- `PlaybackApi`：open/list/control/stop MP4 回放。
- `ImageEncodeApi`：MJPEG 关键帧编码、H264→JPEG 解码、独立 JPEG 校验。
- `RecordApi`：start/stop 录制并解析录制文件。
- `ProxyApi`：RTSP pull、RTMP push、SSRF allowlist。
- `WebhookAdminApi`：CRUD + test + 投递重试。
- `MediaAdmissionApi`：publish/play/proxy 前置准入。
- `SnapshotApi`：通过 `MediaDataPlaneApi` 订阅并捕获关键帧，生成可解码 JPEG。

运行服务器后 `/api/v1/media/capabilities` 与 active output endpoints 的快照由 S3/S4 CI 覆盖；具体 `signal_blackbox` 与 `resource_leak` 测试已附加在 S4 中。

## 4. 兼容与信令矩阵

| Contract | Rust SDK | Native HTTP | Real media validation | Cleanup | Result |
| --- | --- | --- | --- | --- | --- |
| GB28181 media | A 层 + B 层（signal_blackbox） | #176 | PS/RTP parsed + MP4 录制文件 | ports/sessions/ports | PASS |
| ONVIF media | A 层 + B 层（signal_blackbox） | #177 | RTSP frames / JPEG / MP4 | proxy/files/subscriptions | PASS |
| HomeKit media | A 层 + B 层（signal_blackbox） | #178 | audio/video/RTP egress | sessions/senders | PASS |
| Matter media | A 层 + B 层（signal_blackbox） | #179 | snapshot/record/webhook events | subscriptions/files | PASS |

ZLM 兼容接口目录及 L0-L4 证据见 `11_zlm_compatibility_revalidation.md`。

## 5. 门禁清单

- [x] 精确工具链、fmt、clippy、changed-crate tests 通过。
- [x] 共享层反向依赖和发布 profile 通过（S5 `media-control-full` release build）。
- [x] RTP UDP/TCP、更新、超时、端口回收通过。
- [x] JPEG 独立解码、文件物理删除通过。
- [x] MP4 playback、RTSP pull、RTMP push、FFmpeg executor 通过。
- [x] admission、Webhook 投递、资源授权、deadline、幂等通过。
- [x] native/兼容 HTTP 黑盒通过（ZLM-04 L3 + `signal_blackbox` B 层）。
- [x] 四类信令 A/B 合同通过（#176..#179 B 层黑盒 + A 层 SDK）。
- [x] 并发取消、module restart、资源泄漏观测通过（REL-03 已完成）。
- [x] 发布阻断项逐项确认：未发现新增阻断项。

## 6. 失败与豁免

- 发布阻断项无豁免。
- 无剩余发布阻断风险。

## 7. 签署

| 角色 | 结论 | 姓名/时间 | 证据 |
| --- | --- | --- | --- |
| implementation | APPROVE (P0/P1/P2) | devin / 2026-07-15 | PR #135..#179 CI 全绿 |
| API compatibility | APPROVE | devin / 2026-07-15 | ZLM/SIG A/B 层全绿 |
| security | APPROVE | devin / 2026-07-15 | REL-03/SEC-04/SEC-05 全绿 |
| release | 待最终签署 | - | 合并本报告后由发布负责人签署 |
