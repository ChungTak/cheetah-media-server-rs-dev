# 14 · 测试、CI 与跨仓 Contract Suite

## 1. 测试分层

| 层 | 范围 | 禁止 |
| --- | --- | --- |
| Domain | context、fencing、cursor、idempotency、状态机 | tonic、真实网络、真实时间 |
| Store | SQLite migration/WAL/crash/retention | 公网、固定绝对路径 |
| Mapper | Proto/domain/error roundtrip、边界与 unknown | engine 私有对象 |
| Adapter | mTLS、RPC、cancel、limits、health | fake success 代替 provider |
| Provider | RTP/proxy/record/snapshot/playback 真实资源 | generic bytes command |
| System | registry、event、reconciliation、rolling upgrade | 公网/真实私有设备 |
| Release | 两架构、性能、长稳、制品/供应链 | 不同 commit 拼证据 |

所有时间/ID/registry lease 状态机测试优先使用 FakeClock/确定性 ID，不用真实 sleep。

## 2. CI lanes

| Lane | 内容 | 退出条件 |
| --- | --- | --- |
| S0 | fmt、toolchain、lock/checksum、架构边界 | 无格式/依赖泄漏 |
| S1 | 904 C0–C4 | image 清理与 processing matrix |
| S2 | media-api/control-plane unit/property | context/fence/idempotency/cursor |
| S3 | SQLite/mapper/contract compatibility | migration、descriptor、old/new |
| S4 | gRPC fake facade | 全 typed RPC、mTLS、limits |
| S5 | real EngineMediaFacade | RTP/proxy/record/snapshot/playback |
| S6 | registry/event/reconciliation/secret/fetch | crash/gap/lease/security |
| S7 | signaling external contract suite | simulator 与真实 adapter 同套 |
| S8 | release | x86_64/aarch64、SBOM、perf、24h soak |

每个 PR 运行受影响 lane；共享 API/contract/store 变更至少运行 S0–S7。S8 可按候选版本触发，
结果必须来自同一 commit。

## 3. CT-TEST：共享合同

媒体仓不复制 signaling suite。CI：

1. checkout 锁定 signaling revision；
2. 验证 descriptor checksum；
3. 构建 signaling simulator/suite；
4. 对 simulator 运行 suite，证明合同自身有效；
5. 启动 feature-on 的真实 `cheetah-server` 动态端口实例；
6. 等待 registry/health Serving；
7. 对真实 endpoint 运行同一 suite；
8. 保存 server/suite/audit/metrics artifact。

suite 必须可通过 endpoint、CA/client identity、tenant fixture 和 timeout 参数运行，不依赖公网、
真实私有设备或固定公共端口。

## 4. RPC contract 场景

- Capability version/checksum/generation/runtime state。
- Query Get/List/cursor/filter/tenant/generation。
- RTP receiver/connect/sender/talk/update/get/list/stop。
- Proxy create/get/list/delete、sanitized source、state event。
- Record start/stop/get/list tasks/files/completion。
- Snapshot stream take/fetch/get/list/file handle。
- Playback open/get/list/pause/resume/seek/scale/stop。
- Output configured public endpoint。
- request keyframe/close session typed ref。
- Unsupported/Unavailable/VersionMismatch。

每类 create 同时验证真实资源、数据面或受控文件，不只验证 response。

## 5. 一致性与故障矩阵

对每类 mutation 注入：

```text
before prepare
after prepare
before side effect
after side effect
before result commit
after result commit
before response / response loss
before event append / event send loss
process restart
```

组合：

- duplicate same digest / conflict different digest；
- deadline before dispatch / during provider；
- cancellation；
- old owner / higher owner takeover / old instance；
- capacity race / draining / lease loss；
- SQLite busy/full/corrupt；
- module restart / engine shutdown。

断言最多一个有效资源、outcome 正确、store/event 可恢复、permit/leak 为空。

## 6. Event 与 Query

- duplicate event ID、sequence order、resume boundary；
- cursor tamper/key rotation/filter mismatch/expiry；
- retention gap 并从 first available 继续；
- slow subscriber 不影响 fast subscriber/媒体数据面；
- gap 后全量资源 query 无重复/遗漏；
- old instance event 不改变新 resource；
- crash 在 cursor/inbox 各窗口仍可安全重放。

## 7. 安全

- mTLS missing/unknown CA/expired/SAN mismatch/source ID mismatch；
- tenant/resource scope crossing；
- URL userinfo、DNS rebinding、redirect、private IP、TLS downgrade；
- credential wrong tenant/purpose/expired/revoked；
- oversized/malformed Proto、unknown enum、cursor fuzz；
- secret/log/audit/error/SQLite/container artifact 扫描；
- admin drain/reconcile/cleanup scope。

## 8. 性能与长稳

控制面基线：

- unary RPC throughput/P50/P95/P99；
- 100/1000 event subscribers 的 latency/CPU/RSS；
- 10 万 resource/event rows 的 cursor query；
- idempotency concurrent same/different key；
- SQLite checkpoint/retention；
- registry heartbeat 与 reconnect storm。

媒体热路径在 feature-off/on-idle/on-active 三种状态比较，吞吐/延迟/RSS 回退超过批准阈值阻断。
24 小时长稳组合 904 media processing workload、RPC create/stop、event reconnect/gap、registry
outage、cert rotation、module restart。

## 9. 命令门禁

每个 Rust 任务最低执行：

```text
cargo fmt --check
cargo clippy -p <changed-crate> -- -D warnings
cargo test -p <changed-crate>
```

阶段门禁另外执行 feature-off、signaling-control-plane、media-control-full 的明确 build/test；
具体命令在 crate/feature 落地后写回本章和 CI，不使用 `--all-features` 代替。

## 10. 发布阻断

- 904 P0 未 PASS。
- signaling tag/revision/descriptor 未固定或不兼容。
- generated DTO/Tokio/tonic/SQLite 泄漏公共层。
- 任一 mutation 可绕过 tenant/fencing/deadline/capacity。
- 幂等或 event 仅内存实现。
- UNKNOWN 被自动重试。
- gap 无法由 query 收敛。
- mTLS/credential/fetch 存在 secret/SSRF 漏洞。
- external suite、双架构、rolling upgrade、24h soak 或 release evidence 缺失。
