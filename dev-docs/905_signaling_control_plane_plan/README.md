# 905 · Signaling 集群控制面与媒体节点生产接入计划

> 面向下一轮执行实现的详细开发指导。本目录承接 904 尚未闭环的发布工作，并实现
> `cheetah-signaling` 对媒体服务器提出的跨进程、集群化、幂等与 fencing 要求。

## 1. 文档地位

审计基线固定为：

- `cheetah-media-server-rs`：`d41ecbec4764519939d2b720141f275886a9bd8c`
- `cheetah-signaling`：`f295c1b73345da0708d72ed06e283b166a17c73a`
- 下游要求：`dev-docs/003_next_round_vibe_coding_plan/90_media_server_upstream_requirements.md`

904 已实现主要媒体处理 provider、派生流、协议接入、安全与观测代码，但没有完成正式
release evidence、五条真实协议 E2E、C0–C6 完整矩阵、供应链报告和 24 小时长稳。
因此 904 当前结论是 **功能主体已落地，发布闭环 BLOCKED**，不得作为已发布能力跳过。

signaling 基线中的 `cheetah.media.v1` 仍以 generic command/message 为主，详细计划要求的
typed RPC、完整 mutation context、effect outcome、replay cursor 和 gap 尚未发布为固定
合同。媒体仓只能消费 signaling 发布的固定 tag/revision 与 descriptor，不复制 Proto，
也不在本仓自行发明 wire 字段。

## 2. 不可变边界

- 信令业务、SIP、SOAP、设备目录和信令数据库不得进入媒体进程。
- generated DTO 只存在于 gRPC adapter，不进入 engine/domain 持久状态。
- gRPC handler 只调用 `cheetah-media-api` ports 和控制面 facade，不访问 engine 私有对象。
- 公共 Domain/SDK API 保持 runtime-neutral、framework-neutral，不暴露 tonic、prost、
  Tokio、SQLite connection 或 signaling DTO。
- 默认 feature 不编译 gRPC、signaling contracts、SQLite 或媒体处理能力。
- 受控资源必须携带 tenant、MediaSession、MediaBinding、owner epoch、media node instance
  epoch、generation 和幂等关联。
- 所有 create/open/start/connect 在端口、任务、租约、worker 或文件分配前完成
  deadline、fencing、授权、drain 与容量检查。
- 同 StreamKey 单发布者、协议 `core + driver + module`、`AVFrame + TrackInfo` 等既有边界
  不变。
- 密码、Authorization、URL userinfo、原始 secret、内部路径和原始协议报文不得进入日志、
  audit、event、error、cursor 或幂等记录。

## 3. 发布范围

| 能力 | 本轮交付 |
| --- | --- |
| 904 closeout | 依赖清理、C0–C6、五条 E2E、性能/24h soak、SBOM/license、正式证据 |
| 合同消费 | 固定 signaling tag/revision、descriptor checksum、版本协商与兼容测试 |
| gRPC server | Capability、Query、RTP、Proxy、Record、Snapshot、Playback、Output、Control、Event |
| 节点接入 | register、heartbeat、load、lease、drain、deregister、自我隔离 |
| 一致性 | mutation context、owner/instance fencing、generation、持久幂等、结果 outcome |
| 对账 | 所有受控资源 Get/List、稳定 cursor、typed cleanup、orphan reconciliation |
| 事件 | 持久 sequence、至少一次投递、resume、retention、gap、慢消费者隔离 |
| 安全 | mTLS、证书轮换、credential handle、SecretExchange、restricted snapshot fetch |

本轮不实现集中媒体数据库、跨媒体节点共享 SQLite、通用工作流引擎、任意 URL fetch、
任意文件路径、SIP/ONVIF SOAP、设备控制或 signaling scheduler。

## 4. 执行阶段

| 阶段 | 目标 | 进入条件 | 退出条件 |
| --- | --- | --- | --- |
| P0 | 关闭 904 发布缺口 | 当前 main 可构建 | CL904-01..05 全部有同候选制品证据 |
| P1 | 合同与 Domain 基础 | P0 PASS；signaling 发布固定合同 | CT-01..03、CTX-01..03、ARCH-01..03 |
| P2 | 持久控制面与 typed RPC | P1 公共接口稳定 | STORE-01..04、GRPC-01..10 |
| P3 | 节点、事件、对账与安全 | P2 RPC contract 通过 | NODE、EVT、QRY、SEC、CRED 全部通过 |
| P4 | 迁移、互操作和发布 | P3 真实 adapter 可用 | MIG、OPS、REL 与下游 contract suite 全绿 |

P0 是新控制面发布门禁。P1 之后同一时间只能有一个迁移序列修改
`cheetah-media-api` 公共请求、资源或错误类型。

## 5. 文档索引

1. [审计基线与差距登记](01_audited_baseline_and_gap_register.md)
2. [904 收尾与发布门禁](02_904_closeout_and_release_gates.md)
3. [共享合同依赖与兼容发布](03_contract_dependency_and_compatibility.md)
4. [架构、crate 与数据流](04_architecture_crates_and_data_flow.md)
5. [请求上下文、fencing 与错误](05_request_context_fencing_and_errors.md)
6. [持久幂等与资源索引](06_durable_idempotency_and_resource_index.md)
7. [Typed gRPC server 与 mapper](07_typed_grpc_server_and_mappers.md)
8. [节点注册、lease、drain 与容量](08_node_registry_lease_drain_and_capacity.md)
9. [可重放事件流与对账](09_replayable_event_stream_and_reconciliation.md)
10. [Cursor 查询与资源生命周期](10_cursor_queries_and_resource_lifecycle.md)
11. [凭据、Proxy 与受限 Snapshot Fetch](11_credentials_proxy_and_snapshot_fetch.md)
12. [mTLS、安全、观测与运维](12_mtls_security_observability_and_operations.md)
13. [服务器装配、迁移与灰度](13_server_assembly_migration_and_rollout.md)
14. [测试、CI 与跨仓 Contract Suite](14_test_ci_and_contract_suite.md)
15. [执行路线与 Agent 交接](15_execution_roadmap_and_agent_handoff.md)
16. [发布证据模板](16_release_evidence_template.md)

## 6. 全局完成定义

- [ ] 904 release evidence 在同一候选制品上签署 PASS。
- [ ] 媒体仓只消费 signaling 固定合同，不存在可独立修改的 Proto 副本。
- [ ] 默认制品无 tonic/signaling contracts/SQLite；显式 feature 制品可完整启动。
- [ ] 所有 mutation 缺少强上下文、过期、旧 owner 或旧 instance 时无副作用。
- [ ] response loss、重复请求和进程恢复不产生第二个有效资源。
- [ ] drain/lease loss 后拒绝 create，query/stop 仍可用于收敛。
- [ ] event gap 可检测并由 cursor query 对账收敛。
- [ ] credential、URL userinfo、内部路径和跨 tenant 状态无泄漏。
- [ ] signaling simulator 与真实 media adapter 运行同一 contract suite。
- [ ] x86_64/aarch64 制品、SBOM、升级/回滚说明和发布证据齐全。
