# 903 · 流媒体 API 未完成能力收敛计划

> 面向执行实现的外部编程体。本文档集给出固定接口、实施顺序、测试和发布证据要求，不需要执行者重新决定架构。

## 1. 文档地位

901 给出目标架构，902 给出过一次交付计划；本目录基于当前源码重新审计。凡 902 的完成标记、能力结论或验收口径与本目录冲突，以 903 为准。只以生产 provider、真实媒体链路和可复现测试为完成证据；trait、路由、固定 JSON、fake provider 或仅保持 session 存活均不构成完成。

本轮不实现 SIP、ONVIF SOAP、HAP 或 Matter cluster 等信令，只完成这些外部信令服务器需要的流媒体控制面、数据面、文件和事件接口。

## 2. 不可变边界

- Domain API 使用 Rust 原生类型，serde 只用于可交换数据，不绑定 HTTP 字段。
- native HTTP 与兼容 HTTP 是互相独立的 adapter，只做认证、校验、翻译和错误映射。
- 公共 API runtime-neutral；协议仍遵循 `core + driver-tokio + module`，core 保持 Sans-I/O。
- 媒体统一为 `AVFrame + TrackInfo`，同一 `StreamKey` 保持单发布者租约。
- 未注册、未启动或不能完成真实操作的能力返回 `Unavailable`/`Unsupported`，不得伪成功。
- 只新增本目录，不回写 901/902 的历史文本。

## 3. 执行阶段

| 阶段 | 目标 | 进入条件 | 退出条件 |
| --- | --- | --- | --- |
| P0 | 纠正公开事实和安全语义 | 当前基线测试可运行 | CAP、HTTP、RTP、IMG、SEC 全部通过 |
| P1 | 打通真实异步媒体能力 | P0 公共契约稳定 | VOD、PRX、EVT 全部通过 |
| P2 | 兼容与第三方交付 | P1 真实 provider 可用 | ZLM、SIG、REL 全部通过 |

每个阶段先改 Domain/SDK，再改 provider，随后改 adapter，最后补测试。公共 trait 变更不得与 provider 迁移并行落到不兼容状态。

## 4. 文件索引

1. [审计基线与差距登记](01_audited_baseline_and_gap_register.md)
2. [架构与公共契约](02_architecture_and_public_contracts.md)
3. [能力与 URL 真实性](03_capability_and_url_honesty.md)
4. [Native HTTP 契约](04_native_http_contract_completion.md)
5. [RTP 与 GB28181 媒体闭环](05_rtp_and_gb28181_completion.md)
6. [快照、编码与文件生命周期](06_snapshot_image_and_file_lifecycle.md)
7. [录制、VOD 与回放](07_record_vod_and_playback_integration.md)
8. [代理连接器与 FFmpeg](08_proxy_connector_and_ffmpeg_execution.md)
9. [事件、Webhook 与准入](09_event_webhook_and_admission.md)
10. [鉴权、deadline、幂等与安全](10_auth_deadline_idempotency_and_security.md)
11. [兼容接口重验](11_zlm_compatibility_revalidation.md)
12. [第三方信令生产合同](12_signal_server_production_contracts.md)
13. [工具链、CI 与发布门禁](13_test_toolchain_ci_and_release_gates.md)
14. [执行路线与交接](14_execution_roadmap_and_agent_handoff.md)
15. [发布证据模板](15_release_evidence_template.md)

## 5. 全局完成定义

- [ ] 能力、operation、URL 与 module/provider 实际状态一致。
- [ ] RTP 收发、参数更新、超时和清理有真实网络测试。
- [ ] JPEG 可解码；文件删除同时清理受控物理文件和元数据。
- [ ] MP4 回放实际读取、解复用并向引擎输出媒体帧。
- [ ] RTSP 拉流、RTMP 推流和 FFmpeg 任务执行有成功链路。
- [ ] 准入决策进入发布/播放主路径，事件可由外部服务可靠订阅。
- [ ] 资源级授权、deadline、幂等和 SSRF 防护有负向测试。
- [ ] 四类信令合同同时覆盖 Rust SDK 与独立 HTTP 客户端视角。
- [ ] 发布报告中的每个结论都有命令、日志或制品证据。

