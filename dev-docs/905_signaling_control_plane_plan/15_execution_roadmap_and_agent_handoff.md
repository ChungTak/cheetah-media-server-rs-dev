# 15 · 执行路线与 Agent 交接

## 1. 依赖图

```text
CL904-01..05
  -> CT-01 -> CT-02/03
  -> ARCH-01/02/03
  -> CTX-01/02/03 -> ERR-01 -> FENCE-01/02
  -> STORE-01 -> IDEM-01/02 -> STORE-02/03/04
  -> QRY-01..05
  -> GRPC-01..10
  -> CAP-01/02 -> NODE-01..05
  -> EVT-01..06 -> REC-01/02
  -> CRED-01..03 -> FETCH-01..03
  -> SEC-01..03 -> OBS-01..03 -> OPS-01/02
  -> MIG-01..04 -> REL-01..04
```

`CT-01` 是外部 blocker。等待期间只能完成不依赖最终 wire 字段的测试 harness/Domain 设计，
不得合入临时 Proto 副本。

## 2. P0：904 Closeout

| Task | 实施 | 完成证据 |
| --- | --- | --- |
| CL904-01 | 删除直接 image 依赖 | rg/Cargo tree |
| CL904-02 | C0–C6 CI | 全 lane logs |
| CL904-03 | 五条真实 E2E | decoded/browser/player artifact |
| CL904-04 | benchmark + 24h soak | 固定机器报告 |
| CL904-05 | SBOM/license/evidence | 904 PASS 签署 |

P0 未 PASS 时后续代码可在 feature branch 开发，但不得发布或默认装配。

## 3. P1：合同与公共基础

| Task | 实施 | 完成证据 |
| --- | --- | --- |
| CT-01..03 | 固定合同、descriptor、compat | checksum + old/new tests |
| ARCH-01..03 | 两 crate、feature、装配骨架 | feature-off/on build |
| CTX-01..03 | IDs、mutation/read/local context | API contract tests |
| ERR-01 | code/outcome/resource ref | exhaustive mapper tests |
| FENCE-01/02 | instance/owner/generation guard | no-side-effect tests |

P1 同一时间只允许一个公共 API owner。先迁移所有 workspace literal/fake，再开始下一 trait 变更。

## 4. P2：持久控制面与 RPC

| Task | 实施 | 完成证据 |
| --- | --- | --- |
| STORE-01 | traits、SQLite、migration | WAL/restart/corrupt tests |
| IDEM-01/02 | canonical digest/state machine | duplicate/conflict/crash |
| STORE-02..04 | resource index/recovery/retention | startup reconciliation |
| QRY-01..05 | CursorPage、filters、provider migration | stable pagination matrix |
| GRPC-01..10 | typed services/mapper/limits/health | fake + real facade contract |

顺序固定为 Domain → store/control facade → provider migration → adapter → black-box。

## 5. P3：集群、事件与安全

| Task | 实施 | 完成证据 |
| --- | --- | --- |
| CAP-01/02 | 原子 permit、overload | concurrency race |
| NODE-01..05 | register/heartbeat/lease/drain | FakeClock state matrix |
| EVT-01..06 | durable event/cursor/gap | replay/slow subscriber |
| REC-01/02 | startup/gap/unknown/orphan 对账 | convergence tests |
| CRED-01..03 | handle/exchange/URL migration | secret scope/leak tests |
| FETCH-01..03 | restricted fetch/FileStore | SSRF/content/cancel |
| SEC/OBS/OPS | mTLS、rotation、metrics、fault | security/fault matrix |

事件和 query 必须同时可用后才能开启 mutation canary，避免副作用无法对账。

## 6. P4：迁移与发布

| Task | 实施 | 完成证据 |
| --- | --- | --- |
| MIG-01 | feature/config/server assembly | three artifact profiles |
| MIG-02 | local/cluster resource isolation | cross-tenant tests |
| MIG-03 | GB unique owner | double-owner rejection |
| MIG-04 | R0–R5 rollout/rollback | canary/rollback report |
| REL-01 | external contract suite | simulator + real adapter |
| REL-02 | x86_64/aarch64/container/SBOM | artifacts/checksums |
| REL-03 | performance/24h/upgrade | reports |
| REL-04 | docs/evidence/sign-off | final PASS |

## 7. 每项任务执行模板

1. 在 01 差距表确认 task/current state。
2. 记录依赖的合同 revision/checksum 和 feature。
3. 先写能证明缺口的失败测试。
4. 修改最小公共契约并迁移全部 workspace 调用方。
5. 按 Domain → provider/control plane → adapter → black-box 实施。
6. 注入 deadline、cancel、fencing、crash、overload 和 cleanup。
7. 运行 changed crate、反向依赖和对应 CI lane。
8. 更新本路线状态与 16 发布证据。

## 8. Agent 交接字段

交接必须写明：

```text
task ID / owner / branch / revision
contract tag/full revision/descriptor checksum
public API/schema/config changes
feature/profile
SQLite schema/migration version
fencing/idempotency/outcome rules
resource/queue/retention limits
tests/commands/artifacts
unfinished branch/blocker
rollback point
```

禁止使用“基本完成”“理论支持”“应该可用”。外部 blocker 写清 owner、解除条件和下一条验证命令。

## 9. 并行规则

- 904 E2E/perf 可并行，但 release evidence 由单一 owner 汇总。
- CT-01 完成后，mapper 可与 store internals 并行；公共 context/error/query 只有一个 owner。
- 各资源 provider 在统一 meta/query trait 稳定后并行迁移。
- registry client 与 gRPC server 可并行，但共享 node runtime state 由单一 owner。
- event journal 与 cursor codec 可并行；EventStream 等二者稳定后开始。
- Proxy credential 与 Snapshot Fetch 共用 policy/credential port，禁止各自设计 API。
- migration/rollout 等全 contract/security gate 通过后开始。

## 10. 最终 DoD

所有任务有唯一 owner、revision、命令和 artifact；904 与 905 evidence 均 PASS；同一候选制品
通过 signaling simulator/真实 adapter contract、双架构、rolling upgrade、故障矩阵和
24 小时长稳；old owner/instance 不能修改新资源；gap 可对账；secret 无泄漏。
