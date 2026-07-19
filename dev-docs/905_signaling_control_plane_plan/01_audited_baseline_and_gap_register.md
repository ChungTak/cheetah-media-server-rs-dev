# 01 · 审计基线与差距登记

## 1. 审计方法

只把 production provider、真实数据面、持久恢复和可复现测试视为完成。trait、DTO、
fake provider、HTTP/gRPC 2xx、session 存活或目录中存在模板均不构成完成证据。

审计同时检查源码、Cargo feature、CI、提交历史、904 文档和 signaling 的 Proto/计划。
后续实现若偏离本表，必须先更新本表并说明证据，不得用“基本完成”关闭任务。

## 2. 904 当前事实

| Task | 当前证据 | 结论 |
| --- | --- | --- |
| UP/DEP/API/RUN/CAP | avcodec 固定 revision、可选 features、Processing API、spawn_blocking、preflight 已存在 | 代码完成 |
| IMG/SNAP/AUD/VID/OSD | avcodec provider、快照迁移、真实编解码单测已存在 | 功能主体完成，发布 E2E 缺失 |
| JOB/ABR/MIX/SUB/HLS-SUB | Job、派生流、混音、宫格、CEA/WebVTT、HLS 接入已存在 | 功能主体完成 |
| INT-RTMP/WEBRTC/PROXY/HLS | 协议 helper 与派生任务接入已存在 | 无同候选制品真实五链路证据 |
| SEC/OBS/FAULT | admission、边界、幂等、metrics、preflight、故障注入已存在 | 代码完成 |
| MIG | 旧执行边界已大幅清理，但 workspace/test 仍直接依赖 `image` | 未严格关闭 |
| PERF/REL | S6 基础 lane 已存在 | 缺完整 C0–C6、benchmark、24h soak、SBOM/license |
| Evidence | 只有 `15_release_evidence_template.md` | BLOCKED |

### 2.1 904 任务证据台账

| 904 Task | 实际证据 | 审计状态 |
| --- | --- | --- |
| UP-01 | processing manifest 固定 avcodec `0.2.0` revision `c6a40f4428c39990f4bdb3bed0a1dcfea0838e3f`，MP3 adapter 已接入 | Code PASS |
| DEP-01 | avcodec 仅在 processing module 中作为 optional 顶层依赖；默认 feature 为空 | Code PASS；SBOM/license 待证据 |
| API-01 | `cheetah-media-api` 已有 processing/image/subtitle typed contracts | Code PASS |
| RUN-01 | runtime-api 与 Tokio adapter 已有 `spawn_blocking` 及测试 | Code PASS |
| CAP-01 | processing preflight、capability gating、module registration 已存在 | Code PASS |
| IMG-01 | `provider/image.rs` 覆盖 encoded/frame input、算子、JPEG 输出和 PNG Unsupported | Code PASS |
| SNAP-01 | snapshot module 已迁移到 `ImageProcessApi` | Code PASS；三 codec live E2E 待证据 |
| AUD-01/02/03 | `provider/audio.rs` 有 G711/AAC/Opus/MP3、重采样和 roundtrip tests | Code PASS；浏览器链路待证据 |
| VID-01/OSD-01 | `provider/video.rs` 与 image overlay 已有真实 avcodec tests | Code PASS；visual/release matrix 待证据 |
| JOB-01 | processing registry、状态、共享指纹、deadline/idempotency、cleanup 已存在 | Code PASS |
| ABR-01 | `provider/abr.rs` 与 HLS master variant 消费已存在 | Code PASS；真实 player 切档待证据 |
| MIX-01/02 | audio mixer、video mosaic worker/preflight/decodable output tests 已存在 | Code PASS；性能边界待证据 |
| SUB-01/HLS-SUB-01 | codec CEA/WebVTT parser、HLS VTT muxer/playlist/subscriber 已存在 | Code PASS；真实字幕播放待证据 |
| INT-RTMP/WEBRTC/PROXY/HLS | 各 module 已通过 processing helper 创建/回收派生 Job | Code PASS；五链路候选制品证据缺失 |
| MIG-01 | 旧 FFmpeg executor 边界已删除，仍有测试用第三方 `image` 直接依赖 | BLOCKED |
| SEC-01 | admission、owner isolation、FileHandle、deadline/idempotency、bounds tests 已存在 | Code PASS |
| OBS-01 | structured logs、metrics、preflight health、resource leak report 已存在 | Code PASS |
| PERF-01 | 无 processing 专用固定基线和 24h artifact | BLOCKED |
| REL-01 | CI 只有基础 S6，未生成 C0–C6/SBOM/license/final evidence | BLOCKED |
| DOC-01 | SystemArchitecture 与 processing operations guide 已更新 | Code PASS |

`Code PASS` 只表示对应实现和局部测试存在，不等同于 904 发布 PASS。任何缺少 release
artifact 的行仍受 CL904-02..05 约束。

### 2.2 必须承接的 904 残留

| ID | 缺口 | 关闭证据 |
| --- | --- | --- |
| CL904-01 | 根 workspace 与多个测试 crate 仍直接依赖 `image` | `rg`/`cargo tree` 无直接依赖 |
| CL904-02 | S6 未覆盖 904 定义的 profile/feature 全矩阵 | C0–C6 CI 日志 |
| CL904-03 | 五条协议链路没有统一真实数据面报告 | 独立 decoder/browser/player artifact |
| CL904-04 | 无固定硬件基线和 24 小时混合负载 | benchmark + soak artifact |
| CL904-05 | 无 SBOM/license/最终 release evidence | 签署的 PASS 报告 |

## 3. 媒体仓已有控制面基础

- `cheetah-media-api` 已提供 typed Rust traits：query/session、RTP、proxy、record、snapshot、
  playback、output URL、capability 与 bounded event bus。
- `MediaRequestContext` 只有 request/correlation/principal/source/trace/deadline/idempotency，
  缺 tenant、owner、target instance、operation、MediaSession/Binding 等字段。
- `MediaErrorCode` 缺 StaleOwner、RateLimited、Cancelled、VersionMismatch、CursorExpired；
  `MediaError` 没有副作用 outcome。
- 多数 query 仍使用 `page + page_size`，`Page<T>` 同时出现 page 和 cursor，不能作为稳定对账。
- 进程内 event bus 有 bounded subscriber 和 lag callback，但无持久 retention、resume cursor、
  gap、跨进程至少一次投递。
- SDK 的幂等仓库是 in-memory、按 principal scope、用 `Any` 缓存结果，不能跨进程恢复。
- Record provider 等路径仍可能用 idempotency key 派生资源 ID，不能作为统一生产语义。
- Proxy 已有 DNS pinning/SSRF 基础，但 `PullProxyRequest` 只有 `source_url`，没有 credential
  handle；Snapshot 没有受限外部 fetch。
- 仓库没有 tonic/prost gRPC server、媒体节点 registry client 或 durable control store。

## 4. Signaling 合同当前事实

signaling 基线已有 `cheetah-signal-contracts`、MediaClusterRegistry server、media client 与
simulator，但当前 Proto 仍以以下旧接口为主：

- `MediaControl.Execute(CommandEnvelope)` generic 命令；
- `MediaQuery.Query` 与有限 `ListSessions`；
- 单一 `NegotiateRtp`、`Proxy`、`Record`、`Snapshot` RPC；
- event stream 只有简单 filter，没有 resume/gap/retention；
- mutation context 分散在 Envelope/MediaCommand，未覆盖需求中的全部字段；
- error 没有 `NOT_APPLIED | APPLIED | UNKNOWN`；
- registry response 没有完整 lease ID、TTL、heartbeat interval 与 accepted version。

因此 `CT-01` 必须等待 signaling 发布满足 003 计划的固定 tag/descriptor。媒体仓不得以当前
generic Proto 假装完成 typed adapter。

## 5. 新增差距登记

| ID | 差距 | 固定交付 |
| --- | --- | --- |
| CT-01 | 无可消费的最终 typed contract | 固定 tag/revision、descriptor SHA、compat tests |
| ARCH-01 | 无媒体集群控制面与 gRPC adapter crate | 两个独立 system crate、单向依赖 |
| CTX-01 | mutation context 不完整 | 强类型 context 与统一验证器 |
| FENCE-01 | owner/instance 无资源级 fencing | 原子 epoch/generation guard |
| ERR-01 | error 无副作用结果 | 稳定 code + EffectOutcome |
| STORE-01 | 幂等/资源/event 均不持久 | SQLite WAL 生产 store |
| GRPC-01 | 无 typed gRPC server | 十类服务 mapper 与 server |
| NODE-01 | 无 registry client/lease/drain | 节点生命周期 supervisor |
| EVT-01 | 无可重放 event stream | retention、cursor、gap、at-least-once |
| QRY-01 | 无稳定对账查询 | opaque cursor、统一 filters、typed cleanup |
| CRED-01 | 无 credential handle/SecretExchange | 短租约凭据 provider |
| FETCH-01 | 无 ONVIF SnapshotUri fetch | 受限 HTTP(S) fetch、受控存储 |
| CAP-01 | 无跨能力原子容量模型 | permit、load、heartbeat 一致 |
| SEC-01 | 无生产 gRPC mTLS | 双向身份、轮换、scope/audit |
| MIG-01 | 无旧 GB listener 唯一 owner 切换 | 显式配置、灰度、回滚 |

## 6. 差距关闭规则

每个差距必须同时具备：

1. public contract 与分层检查；
2. production provider/adapter 可调用；
3. 成功、拒绝、取消、过期、重启和资源清理测试；
4. fake 与真实 provider 共用 contract；
5. CI 命令、候选制品和 artifact 写入发布证据。

外部合同未发布、真实 provider 未注册或 capability/preflight 不可用时，结论只能是
`BLOCKED`/`Unavailable`，不得以 mapper 草稿或 simulator 成功标记完成。
