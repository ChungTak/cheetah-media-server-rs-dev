# 02 · 905 收口与依赖门禁

## 1. 原则

本计划不复制 905 的实现任务，但 GB 的 production signaling 接管依赖其真实闭环。媒体数据面
与 signaling wire contract 解耦推进；任何需要最终 Proto 字段的工作必须等待 CT-01。

## 2. 依赖门禁

| Gate | GB 可继续的工作 | 被阻塞的工作 | PASS 条件 |
| --- | --- | --- | --- |
| CL904-02..05 | core/driver/module 开发和测试 | 正式 release | 904 同候选证据签署 |
| CT-01 | Domain API、fake、media lifecycle | 真实 signaling adapter | 固定 tag/revision/descriptor |
| CT-02/03 | 内部 typed port | old/new contract rollout | compatibility suite |
| GRPC-01..10 | local adapter 测试 | cluster mutation/query/event | 全 typed service + mapper |
| NODE/EVT/REC | 单节点媒体流程 | lease/drain/reconciliation | 生产 registry/event/recovery |
| CRED/FETCH/SEC | 无 secret 的 RTP 路径 | handle exchange/fetch | mTLS、scope、rotation 与 leak test |
| REL | 开发分支和 fixture | 生产声明 | CI、双架构、SBOM、24h、签名 |

## 3. P0 任务

| ID | 实施 | 完成证据 |
| --- | --- | --- |
| AUD-01 | 固定 media/signaling/reference revisions 与 capability inventory | revision + 01 差距表 |
| AUD-02 | 对每个 905 task 标记 NOT_STARTED/CODE_PASS/ASSEMBLED/RELEASE_PASS/BLOCKED | 可追溯状态表 |
| 905-01 | 检查 adapter 是否注册真实 typed service，不接受 health-only | reflection/service list |
| 905-02 | 检查 production Registry/SecretExchange/Snapshot/recovery/event 装配 | feature-on black-box |
| 905-03 | 检查默认 feature 无 tonic/contracts/SQLite | cargo tree artifact |
| 905-04 | 把 904 与 905 evidence 分开签署 | 两份候选报告 |
| DOC-01 | 修正 SystemArchitecture 与实际 ACK/SDP/auth/lease 状态 | doc diff + reviewer |
| DOC-02 | 建立 GB capability matrix：Supported/Experimental/Unsupported | fixture 链接 |

## 4. CT-01 等待期间允许的工作

- `cheetah-media-api` 内 runtime-neutral 的 RTP session Domain contract。
- admission、capacity、port/publisher lease 的原子生命周期与 fake tests。
- PS/RTP/RTCP/JTT/Ehome core 与 driver，不引用 generated DTO。
- GB module 的 typed port 迁移和 local mode E2E。
- signaling simulator interface 可用本仓 Domain 类型表达，但不得生成或提交临时 Proto。

禁止事项：复制 signaling Proto、继续增加 generic JSON command、在 gRPC handler 访问 engine 私有
对象、把 SIP/XML/device database 移进 media process。

## 5. 905 对 GB 的退出条件

- signaling 固定合同能够表达 open/connect/update/stop/query RTP session 及 generation/fencing。
- media node register/heartbeat/drain/lease loss 能阻止新 GB create，但允许 query/stop 收敛。
- RTP session 进入 durable resource index，重启后可 reconcile 实际 socket/task/lease。
- response loss 重试只产生一个有效资源；old owner/instance/generation 无副作用。
- EventStream 能报告 session state、publisher state、loss/timeout 与 cleanup outcome，并支持 gap 对账。
- `control_owner=local|signaling` 有唯一 owner、灰度、drain、rollback 的黑盒 evidence。
