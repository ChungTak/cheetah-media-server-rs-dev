# 13 · 发布证据模板

> 每个候选版本复制为 `release_evidence_<version>.md`。空字段、跳过必测 lane、仅口头结论或不同
> commit 拼接的结果不得标记 PASS。

## 1. 候选版本

| 字段 | 值 |
| --- | --- |
| Cheetah commit | |
| external media API revision | |
| adapter schema/descriptor SHA-256 | |
| 904 evidence/profile applicability | |
| 905 evidence/profile applicability | |
| Rust/Cargo/OS/arch | |
| Cargo.lock checksum | |
| configuration/profile checksum | |
| x86_64/aarch64 artifact checksum | |
| container digest | |
| SBOM/license report | |
| 测试时间/执行人 | |

## 2. 可选控制面 Profile

未启用相关 feature 时填写 `N/A` 并附 cargo tree；纯 GB 媒体发布不要求可选 904/905 profile PASS。

| Gate | Evidence | 结果 |
| --- | --- | --- |
| CL904-01..05 | | |
| external media API revision/schema | | |
| typed GRPC-01..10 | | |
| Registry/lease/drain/capacity | | |
| Event/query/reconciliation | | |
| Credential/fetch/mTLS | | |
| server assembly/default feature | | |
| 905 final sign-off | | |

## 3. Capability 与兼容矩阵

| 能力 | strict | gb-common | ZLM | SMS | ABL | Ehome | JTT1078 | Evidence/结果 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| UDP receive/send | | | | | | | | |
| TCP active/passive | | | | | | | | |
| 2-byte/4-byte framing | | | | | | | | |
| PS/TS/ES | | | | | | | | |
| H264/H265/AAC/G711 | | | | | | | | |
| RTP/RTCP | | | | | | | | |
| source binding/NAT rebind | | | | | | | | |
| live/playback/download | | | | | | | | |
| voice talk | | | | | | | | |
| JTT 2013/2019 | | | | | | | | |
| Ehome2/Ehome5 | | | | | | | | |

每格只能填写 `Supported`、`Experimental`、`Unsupported` 或 `N/A`，并链接 fixture。Ehome5 等未通过
真实 wire 与互操作的能力必须保持 Unsupported。

## 4. Admission、fencing 与资源生命周期

| 场景 | 期望 | Evidence | 结果 |
| --- | --- | --- | --- |
| admission Deny | 零端口/socket/task/worker/lease/resource | | |
| expired deadline | NOT_APPLIED，无网络副作用 | | |
| old owner/instance/generation | fenced，无副作用 | | |
| capacity/port exhaustion | typed rejection，无泄漏 | | |
| failure at each create step | rollback 到基线 | | |
| publisher conflict | 单发布者，新 session 清理 | | |
| duplicate/response loss | 一个有效 resource | | |
| concurrent create/stop | generation 收敛 | | |
| stop already stopped | 幂等 outcome | | |
| stop cleanup failure | Failed/Orphaned，不伪成功 | | |
| restart/reconcile | 实际资源与 store 收敛 | | |

附测试前后 resource/port/socket/task/worker/permit/publisher lease 计数。

## 5. Codec 与传输

| 场景 | Artifact | 结果 |
| --- | --- | --- |
| PSM missing/late/repeated/change | | |
| PES zero/split/stuffing/private | | |
| dynamic/unknown PT detection | | |
| H264/H265 parameter sets | | |
| seq/timestamp wrap/reorder/duplicate/loss | | |
| RTCP SR/RR/SDES/BYE/timeout | | |
| TCP fragmentation/coalescing/resync | | |
| source spoof/validated rebind | | |
| queue backpressure/slow peer | | |
| JTT2013/2019/Ehome2 | | |

## 6. 外部媒体接口与信令代码退出

| 场景 | Evidence | 结果 |
| --- | --- | --- |
| open receiver/sender/talk | | |
| two-stage open/connect timeout | | |
| update generation/fencing | | |
| stop/get/list/event | | |
| HTTP/gRPC adapter typed mapper | | |
| raw SIP/SDP/XML input rejection | | |
| no SIP listener/parser/runtime task | | |
| old API caller migration | | |
| controller outage/drain/reconciliation | | |

## 7. 互操作与真实输出

| Suite | Revision/device | Fixture/artifact | Decoded/player result | 结果 |
| --- | --- | --- | --- | --- |
| ABL style | | | | |
| ZLM style | | | | |
| SMS style | | | | |
| GB UDP device | | | | |
| GB TCP active device | | | | |
| GB TCP passive device | | | | |
| H264/H265 | | | | |
| PCMA/PCMU talk | | | | |
| playback/download | | | | |

## 8. 安全与观测

| 场景 | Evidence | 结果 |
| --- | --- | --- |
| API identity/tenant/deadline/rate limit | | |
| cross-tenant/resource scope | | |
| oversized API/RTP/PS/JTT/Ehome | | |
| RTP source injection/rebind rate | | |
| mTLS identity/rotation | | |
| log/event/store/error secret scan | | |
| metrics cardinality | | |
| readiness/lease loss/drain | | |
| orphan/admin audit | | |

## 9. CI、性能与长稳

| Lane/Benchmark | Artifact | Baseline delta | 结果 |
| --- | --- | --- | --- |
| fmt/clippy/unit/property | | | |
| fuzz/sanitizer | | | |
| driver/module E2E | | | |
| external media contract | | | |
| interop/security | | | |
| receive/send/talk throughput | | | |
| packet latency/loss/reorder | | | |
| create/stop contention | | | |
| default feature hot path | | | |

24 小时记录 workload、故障注入、CPU/RSS、queue/resource curves、create/stop/restart 次数、registry
outage、cert rotation 和最终 leak report：

- start/end：
- workload/config：
- failure timeline：
- CPU/RSS/queue/resource artifact：
- final resource leak report：
- 结论：

## 10. 文档、回滚与签署

- SystemArchitecture/README/config/capability matrix：
- migration/upgrade/rollback command 与耗时：
- known Experimental/Unsupported：
- open blocker、owner、解除条件、下一条命令：

| 角色 | 结论 | 姓名/时间 | Evidence |
| --- | --- | --- | --- |
| architecture/API | | | |
| codec/RTP | | | |
| GB/media API | | | |
| security | | | |
| performance/stability | | | |
| release | | | |

最终结论只能是 `PASS` 或 `BLOCKED`。BLOCKED 必须列出 task ID、owner、解除条件与下一条验证命令。
