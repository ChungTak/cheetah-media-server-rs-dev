# 902 · 流媒体系统 API 生产交付完善计划

> **读者对象**：后续外部编程体。读完本目录即可按阶段修改代码，无需重新选择架构、接口边界或验收口径。
>
> **前序设计**：`dev-docs/901_api_plan/`。901 定义目标架构，本目录负责把当前“骨架和部分接线”推进到可生产交付。
>
> **实现约束**：同时遵守仓库根目录 `AGENTS.md`、`SystemArchitecture.md` 与当前源码。文中“已实现”必须有生产 provider 和测试证据，不能以 trait、route 或 fake provider 存在为依据。

---

## 0. 交付结论

`dev-docs/902_api_delivery_plan` 已按 S0–S10 全部交付：

- `cheetah-media-api`、`MediaServices`、native/ZLM HTTP module、record/RTP/snapshot/proxy provider 均已完成生产接入。
- `cheetah-sdk` 第三方信令 contract tests 可编译且通过，fake 与 production support 已分离，production contract 启动真实 Engine/provider。
- publish/subscribe、session directory、snapshot、proxy、record playback、媒体事件投递均实现并带测试。
- RTP provider 完整复用 `RtpSession` 端口分配、egress/talk 编排和事件发布。
- ZLM adapter 挂载 64/64 个已规划 API，L1 真实生产路径、L2–L4 显式 capability 状态，字段级 golden fixture 测试已补齐。
- native/ZLM adapter 完成 `ControlAuthApi`、scope、request context、audit、adapter config 和 live reload。

外部进程可通过 native HTTP 操作媒体；同进程 Rust 项目可通过 runtime-neutral SDK 操作控制面和数据面；GB28181、ONVIF、HomeKit、Matter 测试客户端均能通过生产 provider 完成其媒体流程。详细验收报告见 [`13_s10_release_report.md`](13_s10_release_report.md)。

## 1. 固定交付边界

| 边界 | 本轮结论 |
| --- | --- |
| 外部控制面 | native HTTP 是正式 API；ZLM HTTP 是兼容 API |
| 同进程集成 | runtime-neutral Rust SDK 是正式 API |
| 媒体数据面 | `AVFrame + TrackInfo`、RTP、RTSP、RTMP、HTTP-FLV、HLS、WHIP/WHEP 等 |
| 二进制 RPC | 本轮不实现；继续作为未来 adapter |
| 信令协议 | 不实现 SIP、ONVIF SOAP、HAP、Matter cluster |
| ZLM 范围 | 全目录分级交付；核心媒体能力真实实现，非媒体/危险能力可显式 capability-gated |

## 2. 文件索引

| 文件 | 主题 | 主要阶段 |
| --- | --- | --- |
| [01_audit_baseline_and_gap_matrix.md](01_audit_baseline_and_gap_matrix.md) | 当前证据和缺口矩阵 | S0 |
| [02_capability_registry_and_domain_contract.md](02_capability_registry_and_domain_contract.md) | capability、registry、公共契约收敛 | S1 |
| [03_stream_session_and_data_plane.md](03_stream_session_and_data_plane.md) | 真实 session、publish/subscribe、Rust 数据面 | S2 |
| [04_record_snapshot_file_and_vod.md](04_record_snapshot_file_and_vod.md) | 录制、回放、快照、文件 | S3 |
| [05_proxy_and_output_url.md](05_proxy_and_output_url.md) | 拉推代理、FFmpeg、播放 URL | S4 |
| [06_rtp_and_gb28181_media_flow.md](06_rtp_and_gb28181_media_flow.md) | RTP provider 和 GB28181 媒体闭环 | S5 |
| [07_event_bus_and_webhooks.md](07_event_bus_and_webhooks.md) | 内部事件和出站 webhook | S6 |
| [08_native_http_security_and_config.md](08_native_http_security_and_config.md) | native API、安全、配置 | S7 |
| [09_zlm_full_compatibility_catalog.md](09_zlm_full_compatibility_catalog.md) | ZLM 全目录和字段兼容 | S8 |
| [10_third_party_signal_contracts.md](10_third_party_signal_contracts.md) | 四类信令项目生产 contract | S9 |
| [11_test_ci_security_and_release.md](11_test_ci_security_and_release.md) | 测试、CI、安全和发布门禁 | 全阶段 |
| [12_execution_roadmap_and_agent_handoff.md](12_execution_roadmap_and_agent_handoff.md) | 任务图、DoD、Agent 交接 | S0–S10 |

推荐阅读顺序：README → 01 → 02 → 按阶段阅读 03–10 → 11 → 12。

## 3. 总依赖图

```text
S0 工具链、基线与测试恢复
 │
 └─► S1 capability/registry 收敛
      │
      ├─► S2 stream/session/data-plane ─────────────┐
      │                                             │
      ├─► S3 record/snapshot/file/VOD               │
      ├─► S4 proxy/output URL                       ├─► S7 native HTTP
      ├─► S5 RTP/GB 媒体闭环                        ├─► S8 ZLM compatibility
      └─► S6 event bus/webhook ─────────────────────┘
                                                    │
                                                    └─► S9 信令生产 contract
                                                         │
                                                         └─► S10 发布门禁
```

## 4. 不允许回退的内容

- `cheetah-media-api` 保持协议无关，不依赖 Tokio、Axum、数据库或具体协议 module。
- `MediaKey` 与 `StreamKey` 使用统一可逆 bridge，不在 adapter 中重复拼接。
- 同一 `StreamKey` 保持单发布者独占语义。
- 所有协议媒体进入 engine 前统一为 `AVFrame + TrackInfo`。
- core 保持 Sans-I/O；socket、timer、spawn 和 backpressure 留在 driver。
- native 与 ZLM adapter 只翻译，不复制媒体业务状态。
- 未实现能力返回真实 Unsupported/Unavailable，不返回伪成功。

## 5. 总体验收

- [x] 默认工具链可获取，仓库规定的 fmt/clippy/test 命令可执行。
- [x] capability 与实际 provider、编译 feature、module 生命周期一致。
- [x] 真实 session 可查询、踢出、关闭；publish/subscribe 有生产数据面。
- [x] record、snapshot、file、VOD、proxy、RTP 高价值能力端到端可用。
- [x] native API 有认证、授权、审计、幂等、deadline 和稳定错误。
- [x] ZLM 64/64 API 有路由、映射、能力状态和测试归属。
- [x] 全部 ZLM hook 有真实出站事件映射或明确 capability 状态。
- [x] 四类信令 contract 使用生产 provider，不以 fake 作为完成证据。
- [x] `cheetah-media-module` 有 route、错误、鉴权和 golden 测试，不再是 0 tests。
