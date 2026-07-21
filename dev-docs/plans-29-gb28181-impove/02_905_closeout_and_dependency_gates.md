# 02 · 905 审计与可选控制面门禁

## 1. 原则

本计划不复制 905 的实现任务。905 提供可选的集群控制面基础设施，但 GB 媒体能力不依赖本项目
实现信令。媒体 Domain API 由本项目定义；第三方信令适配器消费该 API，任何第三方 Proto 都不得
进入 media core/module。904/905 未闭环必须如实记录，但不得阻塞不依赖这些 feature 的 GB 媒体
core/driver/module 制品。

## 2. 依赖门禁

| Gate | GB 可继续的工作 | 被阻塞的工作 | PASS 条件 |
| --- | --- | --- | --- |
| CL904-02..05 | 全部 GB 媒体工作 | 启用 904 processing 的组合制品发布 | 904 同候选证据签署 |
| media API revision | core/driver/module | 对应 HTTP/gRPC adapter 发布 | API revision/schema 固定 |
| adapter compatibility | 内部 typed port | old/new adapter rollout | compatibility suite |
| GRPC-01..10 | trait/HTTP adapter | 可选 gRPC control profile | 全 typed service + mapper |
| NODE/EVT/REC | 单节点媒体流程 | lease/drain/reconciliation | 生产 registry/event/recovery |
| CRED/FETCH/SEC | 无 secret 的 RTP 路径 | handle exchange/fetch | mTLS、scope、rotation 与 leak test |
| REL-GB | 开发分支和 fixture | GB 媒体生产声明 | CI、双架构、SBOM、24h、签名 |

## 3. P0 任务

| ID | 实施 | 完成证据 |
| --- | --- | --- |
| AUD-01 | 固定 media/reference revisions 与 capability inventory | revision + 01 差距表 |
| AUD-02 | 对每个 905 task 标记 NOT_STARTED/CODE_PASS/ASSEMBLED/RELEASE_PASS/BLOCKED | 可追溯状态表 |
| 905-01 | 检查 adapter 是否注册真实 typed service，不接受 health-only | reflection/service list |
| 905-02 | 检查 production Registry/SecretExchange/Snapshot/recovery/event 装配 | feature-on black-box |
| 905-03 | 检查默认 feature 无 tonic/contracts/SQLite | cargo tree artifact |
| 905-04 | 把 904 与 905 evidence 分开签署 | 两份候选报告 |
| DOC-01 | 从 SystemArchitecture 移除本项目承担 SIP/SDP/auth 的描述 | doc diff + reviewer |
| DOC-02 | 建立 GB capability matrix：Supported/Experimental/Unsupported | fixture 链接 |

## 4. 外部合同冻结前允许的工作

- `cheetah-media-api` 内 runtime-neutral 的 RTP session Domain contract。
- admission、capacity、port/publisher lease 的原子生命周期与 fake tests。
- PS/RTP/RTCP/JTT/Ehome core 与 driver，不引用 generated DTO。
- GB module 的 typed port 迁移和 media API E2E。
- adapter simulator 使用本仓 Domain 类型表达；外部 Proto 只存在对应 adapter。

禁止事项：复制第三方信令 Proto 到 Domain、继续增加 generic JSON command、在 adapter 访问 engine
私有对象、实现 SIP/SDP/XML/parser/listener/transaction 或 device database。

## 5. 可选 905 控制面 profile 的退出条件

- 对外 media contract 能表达 open/connect/update/stop/query RTP session 及 generation/fencing。
- media node lease/drain 能阻止新 GB create，但允许 query/stop 收敛。
- RTP session 进入 durable resource index，重启后可 reconcile 实际 socket/task/lease。
- response loss 重试只产生一个有效资源；old owner/instance/generation 无副作用。
- EventStream 能报告 session state、publisher state、loss/timeout 与 cleanup outcome，并支持 gap 对账。
- 制品不存在 GB 信令监听；外部控制 adapter 的灰度、drain、rollback 有黑盒 evidence。

不启用 905 控制面 feature 的 GB 媒体候选版本在 evidence 中将本节标记为 `N/A` 并附 cargo tree；
不得因可选控制面 BLOCKED 把已满足 REL-GB 的纯媒体制品误标失败，也不得反向宣称控制面已完成。
