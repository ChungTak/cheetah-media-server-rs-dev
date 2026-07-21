# 12 · 执行路线与 Agent 交接

## 1. 依赖图

```text
AUD-01/02 -> 905-01..04 -> DOC-01/02
                         -> API-01..04 -> ADM-01/02 -> LIFE-01..05
                         -> CODEC-PS-01..06 -> CODEC-RTP-01..05 -> RTCP-01..05
                         -> RTP-CORE-01..05 -> DRV-01..06
                         -> MOD-01..05 -> TALK-01..06 -> PLAY-01..06
CT-01..03 + 905 GRPC/NODE/EVT/REC -> SIG-01..04 -> MIG-01..05
all implementation -> TST-* -> SEC/OBS/OPS -> REL-01..05
```

CT-01 是外部 blocker，但不阻塞 API 的 Domain 语义、codec、RTP、driver、module lifecycle 和 local
E2E。禁止为绕开 blocker 创建本地 Proto 副本。

## 2. 任务包

| 阶段 | Task | 主要交付 | 完成证据 |
| --- | --- | --- | --- |
| P0 | AUD/905/DOC | 状态表、905 黑盒检查、能力矩阵、文档纠偏 | reviewed audit |
| P1 | API-01 | RtpSessionApi 与 typed request/result | API contract tests |
| P1 | API-02 | workspace fake/literal/adapter 迁移 | no internal HTTP call |
| P1 | API-03 | typed error、effect outcome、resource ref 与 generation | exhaustive mapper tests |
| P1 | API-04 | capability/profile/limits 配置和 feature-off 边界 | config + cargo tree tests |
| P1 | ADM-01/02 | 统一 admission/fence/capacity gate | deny/no-effect matrix |
| P1 | LIFE-01..05 | rollback、幂等、publisher、stop、restart | fault/restart tests |
| P2 | CODEC-PS | 增量 PS/PES/PSM、track/probe/limits | fixture/fuzz |
| P2 | CODEC-RTP/RTCP | reorder/timeline/PT/RTCP | vector/property tests |
| P2 | RTP-CORE | 显式状态机与文件拆分 | core tests |
| P2 | DRV | UDP/TCP/framing/source/backpressure | I/O matrix |
| P2 | COMPAT | ABL/ZLM/SMS/JTT/Ehome2 profiles | rule fixtures |
| P3 | MOD-01 | GB→RTP typed port 与 REST adapter | module E2E |
| P3 | MOD-02 | controlled resource/reconcile/event | restart tests |
| P3 | MOD-03 | REST aliases 仅做输入规范化，移除无类型内部编排 | adapter contract tests |
| P3 | MOD-04 | port pool、limits、profile、control owner 配置 | validation/restart tests |
| P3 | MOD-05 | module 文件拆分和错误/停止语义统一 | regression + size review |
| P3 | TALK | 对讲闭环 | device/decoder evidence |
| P3 | PLAY | 回放/下载闭环 | player/download evidence |
| P4 | SIG-01 | 固定 contract mapper/typed service | external suite |
| P4 | SIG-02 | register/lease/drain/event/query 装配 | feature-on black-box |
| P4 | SIG-03 | legacy SIP 加固 | SIP fuzz/dialog matrix |
| P4 | SIG-04 | SDP/Subject/time/TCP answer 校验与 compat | device fixture matrix |
| P4 | MIG-01 | register-only 与 shadow observation | shadow report |
| P4 | MIG-02 | local create drain 与 snapshot/query 对账 | convergence report |
| P4 | MIG-03 | local/signaling unique owner 与 lease fencing | double-owner rejection |
| P4 | MIG-04 | canary mutations 与全量 rollout | canary report |
| P4 | MIG-05 | signaling→local 回滚和资源收敛 | rollback report |
| P5 | TST/SEC/OBS | CI、interop、security、metrics/runbook | all lanes |
| P5 | REL | 双架构、SBOM、性能、24h、签署 | release evidence PASS |

## 3. 固定实施顺序

1. 先写能证明当前缺口的失败测试和 fixture metadata。
2. 公共 API 由单一 owner 修改，迁移 workspace 调用方后再允许下一个公共变更。
3. 资源生命周期先于新增传输/compat，保证新路径天然经过 admission/rollback。
4. 共享 codec 先完成，协议层只接入，不复制时间线/参数集/探测逻辑。
5. core → driver → module → external adapter → black-box 逐层验证。
6. 每个厂商行为单独提交 compat rule、fixture、metric 和 rollback。
7. P4 只消费固定 signaling revision；合同变化先更新 CT compatibility suite。
8. 每个任务更新 01 状态和 13 evidence，不把不同 commit 的结果拼为同一候选 PASS。

## 4. 并行边界

- codec PS 与 API/lifecycle 可并行，但 `AVFrame/TrackInfo` 变更由单一 owner。
- RTP core 与 driver fixture harness 可并行；driver 在 core action 稳定后接线。
- ABL/ZLM/SMS fixture 可并行提取，compat registry schema 由单一 owner。
- talk 与 playback 可在统一 session API 稳定后并行，不得各自创建资源模型。
- signaling adapter 与 media data plane 可并行；P4 merge 依赖 CT-01..03。
- release evidence 由单一 owner 汇总，执行者分别提供不可变 artifact/checksum。

## 5. 每项任务模板

```text
task ID / owner / branch / revision
current failing behavior / fixture provenance
public API/config/capability changes
layer and dependency check
admission/fencing/idempotency/generation rules
buffer/queue/session/time limits
failure injection and cleanup invariant
tests/commands/artifacts
metrics/events/runbook changes
documentation changes
unfinished branch/blocker/next command
rollback point
```

## 6. 提交拆分原则

- 机械文件拆分与行为修改分开提交。
- Domain API、provider 实现、module 接线、compat fixture、CI/evidence 分开但保持可构建。
- 一个提交不同时扩展 SIP business 和媒体数据面；SIP business requirement 转交 signaling。
- 修复真实设备问题必须包含最小 fixture 或可重复 generator。
- 超过约 500 行的新增模块主动拆分；超过 800 行必须说明为何暂不可拆及后续 task。

## 7. 最终 DoD

所有 task 有唯一 owner、revision、限制、测试和 artifact；905 依赖状态真实；GB/RTP data plane 的
标准与具名非标准路径通过互操作；Deny/失败/取消/重启无资源泄漏；local/signaling 无双 owner；
SystemArchitecture/配置/capability matrix 同步；13 中同一候选版本的最终结论为 PASS。
