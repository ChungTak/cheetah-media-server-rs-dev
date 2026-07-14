# 12 · 执行路线图、任务清单与 Agent 交接

> 本文件是实现编程体的主工单。每个阶段开始前阅读对应分册；每项只有满足 DoD 和验收命令后才能勾选。

## 1. 总阶段

| 阶段 | 主题 | 前置 | 主要分册 |
| --- | --- | --- | --- |
| S0 | 工具链与测试基线恢复 | 无 | 01、11 |
| S1 | capability/registry 收敛 | S0 | 02 |
| S2 | stream/session/data-plane | S1 | 03 |
| S3 | record/snapshot/file/VOD | S1、S2 部分 | 04 |
| S4 | proxy/output URL | S1、S2 | 05 |
| S5 | RTP/GB 媒体闭环 | S1、S2 | 06 |
| S6 | event bus/webhook | S1，各 provider 发布点 | 07 |
| S7 | native HTTP/security/config | S1–S6 对应能力 | 08 |
| S8 | ZLM 全目录 | S1–S7 | 09 |
| S9 | 四类 production contract | S2–S8 | 10 |
| S10 | CI、发布验收、文档同步 | S0–S9 | 11 |

关键路径：`S0 → S1 → S2 → S5 → S6 → S7 → S8 → S9 → S10`。S3/S4 可在 S2 数据面稳定后并行。

## 2. S0 — 恢复可信基线

- [ ] **S0-T1** 固定可获取的 Rust 1.94.1，验证 rustfmt/clippy。
- [ ] **S0-T2** 修复 signal fake provider 的 `get_rtp_session`。
- [ ] **S0-T3** 所有 RtpSenderRequest 补 `codec_hint`，加 serde 兼容。
- [ ] **S0-T4** 跑 01 中基线命令并更新 gap matrix。
- [ ] **S0-T5** 为 media module 建立最小 route test harness。

DoD：默认 cargo 命令可运行；相关既有测试全部编译；不引入功能行为变化。

## 3. S1 — Capability 与 Provider Registry

- [ ] **S1-T1** 删除默认 record/snapshot/proxy/RTP stub 注册。
- [ ] **S1-T2** 实现 registry-backed facade。
- [ ] **S1-T3** provider registration/generation/unregister。
- [ ] **S1-T4** module lifecycle 与 capability state 接线。
- [ ] **S1-T5** capability SDK/native 查询。
- [ ] **S1-T6** restart、并发替换、stale provider 测试。

DoD：capability 与生产 provider 实时一致；无第二份静态 capability。

## 4. S2 — Stream、Session 和数据面

- [ ] **S2-T1** 真实 session directory 和全局 ID。
- [ ] **S2-T2** 协议 publisher/player 注册。
- [ ] **S2-T3** list/kick/close 精确控制。
- [ ] **S2-T4** domain publisher lease bridge。
- [ ] **S2-T5** domain subscriber bridge。
- [ ] **S2-T6** Rust MediaDataPlaneApi。
- [ ] **S2-T7** StreamInfo 时间、统计、tracks、URL。

DoD：publish→engine→subscriber 有真实 AVFrame；单 session 可精确关闭。

## 5. S3 — Record、Snapshot、File、VOD

- [ ] **S3-T1** record 幂等和完整 MediaKey。
- [ ] **S3-T2** task/file 有界分页和稳定排序。
- [ ] **S3-T3** playback provider 四命令。
- [ ] **S3-T4** snapshot module 与 encoder。
- [ ] **S3-T5** file store、授权下载、range。
- [ ] **S3-T6** record/snapshot 事件。

DoD：在线流可录制、抓图、下载和控制回放；无绝对路径泄漏。

## 6. S4 — Proxy 与 URL

- [ ] **S4-T1** proxy module/registry/幂等。
- [ ] **S4-T2** RTSP pull→engine。
- [ ] **S4-T3** engine→RTMP push。
- [ ] **S4-T4** retry/cancel/restart。
- [ ] **S4-T5** typed FFmpeg jobs。
- [ ] **S4-T6** URL resolver 和短期签名。

DoD：ONVIF 模拟客户端可用 RTSP URI 创建媒体并获得真实播放 URL。

## 7. S5 — RTP

- [ ] **S5-T1** 抽取唯一 session orchestrator。
- [ ] **S5-T2** 动态/指定 port bind 和 ack。
- [ ] **S5-T3** UDP/TCP receiver ingress。
- [ ] **S5-T4** sender subscriber/egress worker。
- [ ] **S5-T5** passive/talk。
- [ ] **S5-T6** SSRC/check/timeout/event。
- [ ] **S5-T7** GB production contract。

DoD：测试 socket 实际收发 RTP，不能只检查 session map。

## 8. S6 — Event 与 Webhook

- [ ] **S6-T1** 有界 MediaEventBus 和 subscription handle。
- [ ] **S6-T2** stream/session/record/snapshot/RTP/proxy 发布点。
- [ ] **S6-T3** webhook dispatcher、retry、熔断。
- [ ] **S6-T4** 决策 hook。
- [ ] **S6-T5** 通知 hook golden。
- [ ] **S6-T6** SSRF 与日志脱敏。

DoD：Matter contract 能实际收到完成事件；慢 webhook 不阻塞媒体。

## 9. S7 — Native API

- [ ] **S7-T1** 全 route 和动态 path matching。
- [ ] **S7-T2** auth provider 和 scope。
- [ ] **S7-T3** request context/deadline/idempotency。
- [ ] **S7-T4** audit。
- [ ] **S7-T5** adapter enabled/prefix/restart 配置。
- [ ] **S7-T6** 默认/full feature capability 一致。

DoD：route contract 与安全矩阵全绿；未知 route 正确 404。

## 10. S8 — ZLM Compatibility

- [ ] **S8-T1** 建立 64 route 编译期/测试 catalog。
- [ ] **S8-T2** L1 真实 provider 映射。
- [ ] **S8-T3** L2 optional provider 映射。
- [ ] **S8-T4** L3/L4 capability guard。
- [ ] **S8-T5** secret/login profile。
- [ ] **S8-T6** endpoint-specific response DTO。
- [ ] **S8-T7** golden/interop fixtures。

DoD：64/64 route 有状态，L1 均为生产成功路径，未实现项只以 `-501` 明示。

## 11. S9 — 第三方信令生产 Contract

- [ ] **S9-T1** fake/production support 分离。
- [ ] **S9-T2** GB28181 production contract。
- [ ] **S9-T3** ONVIF production contract。
- [ ] **S9-T4** HomeKit production contract。
- [ ] **S9-T5** Matter production contract。
- [ ] **S9-T6** restart、权限、deadline 通用失败测试。

DoD：四类测试都启动真实 Engine/provider，不依赖公网或真实信令设备。

## 12. S10 — 发布

- [ ] **S10-T1** 运行 11 的全部门禁。
- [ ] **S10-T2** 修复所有 clippy/test/安全失败。
- [ ] **S10-T3** 输出 capability、route、hook、contract 验收报告。
- [ ] **S10-T4** 同步 `SystemArchitecture.md`、`AGENTS.md`、相关 README/config 示例。
- [ ] **S10-T5** 清理已完成 TODO/stub 和误导性注释。

## 13. 多 Agent 所有权

| 工作流 | 主写区域 | 主要冲突点 |
| --- | --- | --- |
| registry/session | media-api、sdk、engine media provider | EngineContext、MediaServices |
| record/snapshot/file | system record/snapshot、MP4 | FileHandle、event |
| proxy/URL | connector、proxy module | data-plane、protocol features |
| RTP | RTP module/driver | session directory、event |
| event/webhook | engine event、media module | provider 发布点 |
| native/ZLM | media module | route catalog、DTO/auth |
| signal/CI | SDK tests、server profile | production support builder |

涉及共享文件前先同步任务 ID；禁止多个 Agent 同时大改 `MediaServices`、`EngineContext` 或 media adapter 主路由文件。

## 14. Agent 交接格式

每次交接必须给出：

```text
完成任务：Sx-Ty
修改 crate/文件：...
公共 API 变化：...
运行测试：命令 + 结果
能力状态变化：Unavailable/Unsupported/Available
剩余问题：...
下一任务前置：...
```

未运行测试、依赖 fake、存在隐藏 TODO 或 capability 不一致时，不得写“完成”。

