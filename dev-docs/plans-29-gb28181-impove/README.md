# 29 · GB28181 媒体能力完善计划

> 面向下一轮执行实现的详细开发指导。本目录审计
> `dev-docs/905_signaling_control_plane_plan` 的真实完成度，并结合 ABLMediaServer、
> ZLMediaKit 与 simple-media-server 的 GB28181 工程实践，完善 Cheetah 的媒体数据面、
> 非标准兼容、资源生命周期与生产鲁棒性。

## 1. 审计基线与结论

- Cheetah 审计提交：`512e4a5a4650231167c0eba04ff5a64e6892e459`。
- 参考实现：
  - `/dataset/datavol/workspace/media_server/ABLMediaServer-src-2026-07-02/ABLMediaServer`
  - `vendor-ref/ZLMediaKit`
  - `vendor-ref/simple-media-server`
- 905 当前结论：**BLOCKED**。领域类型、SQLite store、幂等/事件/恢复/容量算法与 mTLS health
  骨架已存在；typed media RPC、生产 Registry/SecretExchange/Snapshot client、engine facade
  装配、信令接管、CI 发布矩阵和签名 evidence 尚未闭环。
- 当前 GB28181/RTP crate 的 unit、integration、property tests、runtime boundary check 和
  `clippy --tests -- -D warnings` 通过；这只证明当前代码基线可继续开发，不代表功能完成。

## 2. 不可变边界

- 协议保持 `core + driver-tokio + module`；core 必须是显式 Input/Output/Event/Timer 的
  Sans-I/O 状态机。
- SIP、MANSCDP、设备目录和信令数据库归外部 `cheetah-signaling` 所有；媒体进程不得建设
  第二套生产信令平台。
- `control_owner=local` 仅用于 legacy 迁移；`control_owner=signaling` 时本地生产 SIP listener
  必须关闭，双 owner 必须启动失败。
- GB module 通过 runtime-neutral typed port 调用 RTP 能力，不通过 module-to-module HTTP/JSON。
- 所有资源创建先执行 deadline、fencing、drain、capacity 与 `MediaAdmissionApi::authorize`；
  Deny 或任一步失败不得遗留端口、socket、worker、task、publisher lease 或成功幂等记录。
- 所有媒体进入 engine 前收敛为 `AVFrame + TrackInfo`；PS、时间戳、参数集、Access Unit 与
  payload sniffing 的共享逻辑进入 `cheetah-codec`。
- 厂商兼容必须集中、具名、可配置、可观测；入口允许宽容，内部格式必须规范化。
- 所有 buffer、queue、重排窗口、预鉴权缓存、连接和会话数量都有上限。

## 3. 阶段与门禁

| 阶段 | 目标 | 进入条件 | 退出条件 |
| --- | --- | --- | --- |
| P0 | 固定真实基线与 905 依赖 | 当前 main 可构建 | AUD/905/DOC 全部完成 |
| P1 | typed API 与原子资源生命周期 | P0 PASS | API/ADM/LIFE 全部通过 |
| P2 | PS/RTP/RTCP/JTT/Ehome 数据面 | P1 公共接口稳定 | CODEC/RTP/DRV 全部通过 |
| P3 | GB module、对讲、回放和下载 | P2 数据面稳定 | MOD/TALK/PLAY 全部通过 |
| P4 | 外部信令接管与 legacy 收敛 | CT-01..03 固定 | SIG/MIG/REC 全部通过 |
| P5 | 互操作、安全、长稳和发布 | P1..P4 完成 | TEST/SEC/OBS/REL 全部 PASS |

P1～P3 不依赖最终 Proto，可在 CT-01 外部 blocker 存在时推进。P4 的 cluster/signaling 路径
不得使用临时 Proto 或 generic command 代替固定合同。

## 4. 文档索引

1. [审计基线与差距登记](01_audited_baseline_and_gap_register.md)
2. [905 收口与依赖门禁](02_905_closeout_and_dependency_gates.md)
3. [参考行为与兼容目录](03_reference_behavior_and_compatibility_catalog.md)
4. [架构与公共契约](04_architecture_and_public_contracts.md)
5. [PS、RTP、RTCP 与时间线](05_codec_ps_rtp_rtcp_timeline.md)
6. [RTP core、driver 与传输鲁棒性](06_rtp_core_driver_transport_robustness.md)
7. [GB module 准入与生命周期](07_gb_media_module_admission_lifecycle.md)
8. [信令所有权与 legacy SIP](08_signaling_ownership_and_legacy_sip.md)
9. [对讲、回放与下载](09_voice_talk_playback_download.md)
10. [安全、观测与运维](10_security_observability_operations.md)
11. [测试、互操作、Fuzz 与 CI](11_test_interop_fuzz_ci.md)
12. [执行路线与 Agent 交接](12_execution_roadmap_and_agent_handoff.md)
13. [发布证据模板](13_release_evidence_template.md)

## 5. 全局完成定义

- [ ] 905 的状态与代码、应用装配、CI/evidence 一致，不再以算法单测代替生产闭环。
- [ ] GB module 不再通过 HTTP/JSON 调用 RTP module，公共 API 无 Tokio/tonic 类型泄漏。
- [ ] admission 拒绝、超时、取消、SIP 失败和 socket 失败均实现零资源残留。
- [ ] UDP/TCP active/passive、2/4-byte framing、PS/ES/TS、RTP/RTCP 与非标准 PT 有真实样例。
- [ ] PS/JTT/Ehome 的已支持能力均通过 wire fixture；未验证能力明确返回 Unsupported。
- [ ] `local`/`signaling` 唯一 owner、drain、重启、reconciliation 与 generation fencing 通过。
- [ ] ABL/ZLM/SMS 兼容矩阵、恶意输入、故障注入、性能和 24 小时长稳 evidence 齐全。
- [ ] `SystemArchitecture.md`、相关 README、配置与功能矩阵同步更新。
