# 13 · 服务器装配、迁移与灰度

## 1. MIG-01：Feature 与配置

`apps/cheetah-server` 新增可选 `signaling-control-plane`，并纳入 `media-control-full`；默认
feature 不变。配置 schema 使用 `deny_unknown_fields`/显式校验，至少包含：

```text
enabled
grpc listen/advertised endpoint/message limits
registry endpoint/node identity/zone/addresses
contract min/max/checksum
store path/limits/retention
events limits/retention/cursor key handle
capacity limits
TLS/server identity/client CA
SecretExchange
rollout mode
```

危险配置变更（node ID、store path、contract range、listen/TLS identity）要求 adapter
restart；capacity/retention 等安全项可热更新但不得低于当前 usage。

## 2. MIG-02：旧控制入口

Native/ZLM 保持兼容，但与 cluster 资源严格隔离：

- local adapter 创建 system tenant/local owner 资源；
- local 请求不能 query/mutate cluster tenant；
- cluster gRPC 不枚举 local resource；
- 共享底层 provider 仍执行统一 admission/capacity/fencing guard。

旧 generic gRPC command 不进入生产新路径；若为迁移保留，只能通过独立 compatibility feature、
默认关闭、有下线版本，并映射到相同 guard，不能绕过 typed context。

## 3. MIG-03：GB listener 唯一 owner

新增明确配置：

```text
gb28181.control_owner = local | signaling
```

- `local`：现有 media 内 GB listener 负责信令，cluster mutation 不得同时建立同一业务会话；
- `signaling`：媒体内 SIP/GB control listener disabled，RTP/media module 继续可用；
- 启动时发现双 owner 配置直接失败；
- 切换需要 drain/观察窗口，不在运行中无保护热切；
- 真实 GB contract 与对账通过前不得删除旧 listener 代码。

本轮不把 signaling 的 SIP/目录/设备逻辑迁入 media。

## 4. MIG-04：部署阶段

### R0：feature-off

默认制品回归；无 tonic/contracts/SQLite；904 P0 PASS。

### R1：register-only

启动 gRPC、store、registry heartbeat；capability/query 可用，所有 mutation gate 关闭。比较
capability/load/health 与 signaling scheduler 预期。

### R2：shadow query/event

signaling 消费 query/event 但不驱动业务；验证 cursor、gap、tenant、instance replacement 和
reconciliation。

### R3：canary mutation

按 allowlisted tenant/node/operation 开放 RTP/proxy/snapshot 等 typed create；每个 operation
有独立 kill switch，旧 owner 不并写。

### R4：production

全量 typed path；旧 GB owner 按实例切换 signaling；观察窗口内保留快速回滚。

### R5：legacy removal

只有真实合同、长稳、升级/回滚演练和两个版本观察窗口通过后，另立任务删除旧 generic/owner
路径，不在本轮暗中删除。

## 5. 回滚规则

- 回滚先关闭 create gate，不直接停止已有数据面；
- signaling desired state 通过 query/typed stop 收敛；
- store schema 使用向后兼容 migration，禁止 downgrade 破坏现有 DB；
- binary 回滚必须支持读取当前 schema 或在发布前提供验证过的 export/restore；
- instance epoch 不能回退；
- capability generation 在每次启停/回滚后单调推进；
- rollback artifact/命令/预计时间进入 release evidence。

## 6. 构建与制品

- x86_64/aarch64；
- default、signaling-control-plane、media-control-full 三类显式制品；
- container 以非 root 运行，store/data/cert 权限明确；
- SBOM 包含 tonic/prost/rusqlite/signaling contract revision；
- health/readiness、graceful shutdown、volume 和证书轮换说明齐全；
- 不把测试 CA/private key 打进生产镜像。

## 7. 文档同步

实现时同步：

- `SystemArchitecture.md`：新控制面数据流与依赖；
- `AGENTS.md`：只有新增长期工程边界时更新；
- server README/config example；
- migration/upgrade/rollback/operations；
- signaling integration compatibility matrix。

计划文档不预先把未实现配置写成已支持。

## 8. 验收

- feature-off/default 行为不变。
- register-only/shadow/canary/prod/rollback 演练。
- local/signaling GB unique owner，双 owner 启动失败。
- kill switch 只阻断目标 operation，query/stop 保留。
- rolling upgrade 覆盖 old/new contract reader/writer 与 instance replacement。
- 回滚后无重复资源、旧 owner 或 orphan。
