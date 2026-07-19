# 16 · 发布证据模板

> 每个候选版本复制为 `release_evidence_<version>.md`。空字段、仅口头结论或不同 commit
> 拼接的结果不得标记 PASS。

## 1. 候选版本

| 字段 | 值 |
| --- | --- |
| Cheetah commit | |
| 904 evidence | |
| signaling contract tag | |
| signaling full revision | |
| descriptor SHA-256 | |
| accepted/min/max contract version | |
| Rust/Cargo | |
| OS/arch | |
| Cargo.lock checksum | |
| SQLite schema version | |
| x86_64 artifact/checksum | |
| aarch64 artifact/checksum | |
| container digest | |
| SBOM | |
| license report | |
| 测试时间/执行人 | |

## 2. Feature 与依赖

| Profile | Build command | Artifact | Cargo tree/link check | 结果 |
| --- | --- | --- | --- | --- |
| default | | | no tonic/contracts/SQLite/avcodec | |
| signaling-control-plane | | | fixed contract/SQLite | |
| media-control-full | | | complete control plane | |
| processing C0–C6 | | | 904 evidence | |

附：

- 无本地 Proto 副本检查：
- contract revision/checksum 编译信息：
- generated DTO/Tokio/tonic/SQLite 公共层泄漏检查：
- image/avcodec 边界检查：

## 3. 904 P0

| ID | Evidence | Artifact | 结果 |
| --- | --- | --- | --- |
| CL904-01 direct image removal | | | |
| CL904-02 C0–C6 | | | |
| CL904-03 five protocol E2E | | | |
| CL904-04 benchmark/24h soak | | | |
| CL904-05 SBOM/license/evidence | | | |

904 最终报告/签署：

## 4. Contract 与 Mapper

| 项目 | Simulator | Fake facade | Real provider | 结果 |
| --- | --- | --- | --- | --- |
| descriptor compatibility | | | n/a | |
| Capability/Query | | | | |
| RTP | | | | |
| Proxy | | | | |
| Record | | | | |
| Snapshot Take/Fetch | | | | |
| Playback | | | | |
| Output/Control | | | | |
| EventStream | | | | |

外部 signaling contract suite command/artifact：

## 5. Fencing、幂等与结果

| 场景 | 期望 | Evidence | 结果 |
| --- | --- | --- | --- |
| missing mutation context | NOT_APPLIED/no resource | | |
| expired deadline | NOT_APPLIED | | |
| old owner | StaleOwner/no effect | | |
| higher owner takeover stop | success; old fenced | | |
| old media instance | StaleOwner/no effect | | |
| generation conflict | NOT_APPLIED | | |
| duplicate same digest | first result replay | | |
| same key different digest | Conflict | | |
| response loss | one effective resource | | |
| crash after side effect | query/reconcile; no duplicate | | |
| UNKNOWN | no automatic retry | | |

## 6. Node、容量与对账

| 场景 | Evidence | 结果 |
| --- | --- | --- |
| register/heartbeat/deregister | | |
| instance replacement/old heartbeat | | |
| lease loss/self-isolation/recovery | | |
| drain query/stop/create rejection | | |
| capacity race/overload/retry hint | | |
| startup reconciliation | | |
| orphan grace/typed cleanup | | |
| shutdown deadline/leak report | | |

## 7. Event 与 Cursor

| 场景 | Evidence | 结果 |
| --- | --- | --- |
| durable append before send | | |
| duplicate/order/resume | | |
| retention gap | | |
| gap → query convergence | | |
| cursor tamper/filter/tenant mismatch | | |
| key rotation/expiry | | |
| slow subscriber isolation | | |
| old instance event ignored | | |

## 8. 安全

| 场景 | Evidence | 结果 |
| --- | --- | --- |
| mTLS CA/SAN/source identity | | |
| cert/CA rotation without media restart | | |
| tenant/resource scope | | |
| credential handle scope/expiry/revoke | | |
| URL userinfo rejection | | |
| DNS rebinding/redirect/private range | | |
| Fetch size/type/TLS/storage limits | | |
| logs/audit/error/store secret scan | | |
| admin scope/audit | | |

## 9. 性能与长稳

| Benchmark | Throughput/FPS | CPU | RSS | P95/P99 | Baseline delta | 结果 |
| --- | --- | --- | --- | --- | --- | --- |
| feature-off media hot path | | | | | | |
| feature-on idle hot path | | | | | | |
| unary RPC | | | | | | |
| event subscribers | | | | | | |
| 100k cursor query | | | | | | |
| idempotency contention | | | | | | |
| SQLite retention/checkpoint | | | | | | |

24 小时：

- workload/start/end：
- registry outage/instance replacement：
- cert rotation：
- event reconnect/gap：
- module restart/create-stop 次数：
- CPU/RSS/store/event/resource curves：
- final leak report：
- artifact：

## 10. 迁移、升级与回滚

- register-only/shadow/canary evidence：
- GB local/signaling unique-owner evidence：
- old/new reader/writer：
- rolling upgrade：
- rollback command/time/result：
- default feature regression：
- configuration/operations/release notes：

## 11. 最终签署

| 角色 | 结论 | 姓名/时间 | Evidence |
| --- | --- | --- | --- |
| 904 closeout | | | |
| contract/API | | | |
| implementation | | | |
| security | | | |
| performance/stability | | | |
| signaling integration | | | |
| release | | | |

最终结论只能为 `PASS` 或 `BLOCKED`。BLOCKED 必须列出 task ID、owner、解除条件和下一条验证命令。
