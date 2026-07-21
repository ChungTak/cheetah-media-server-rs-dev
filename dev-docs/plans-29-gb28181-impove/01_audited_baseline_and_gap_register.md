# 01 · 审计基线与差距登记

## 1. 审计方法

本轮以代码装配、生产实现、测试和发布证据为完成依据，不以 trait、fake、TODO 注释或单元测试
存在作为生产完成依据。审计覆盖 905 文档、media API/control plane/gRPC adapter/server assembly、
GB28181/RTP/codec crate、CI、脚本和三个参考项目。

## 2. 905 完成度

| 范围 | 当前事实 | 状态 | 解除条件 |
| --- | --- | --- | --- |
| 904 closeout | direct `image` 清理检查通过；C0–C6、五条 E2E、24h、SBOM/evidence 未签署 | BLOCKED | CL904-02..05 同候选制品 PASS |
| Domain/context/error | `cheetah-media-api` 已有 mutation context、IDs、outcome、cursor/event/node contracts | CODE PASS | 纳入真实 adapter contract test |
| Store/control algorithms | SQLite、幂等、资源、事件、恢复、容量与 admin 算法存在 | CODE PASS | 应用装配、重启和黑盒验证 |
| Typed media RPC | adapter 只注册 health/reflection，未暴露可用媒体服务 | BLOCKED | 固定 media API 与 GRPC-01..10 |
| Registry/credentials/fetch | 只有 trait/fake/default Unsupported，无生产 client/provider | BLOCKED | 生产实现、mTLS 与 failure matrix |
| Server assembly | health listener 可启动，但 supervisor/facade/event/recovery 未完整接线 | BLOCKED | feature-on 黑盒启动与节点接管 |
| CI/release | 现有 workflow 无 905 contract、双架构、SBOM、24h 与签名 evidence | BLOCKED | REL gate 全部落地 |

结论：905 不能标记完成。后续提交必须分别更新“代码已存在”“生产已装配”“发布已验证”三列，
禁止用“基本完成”合并不同成熟度。

## 3. 当前 GB/RTP 已有能力

- crate 已按 GB/RTP 的 core、driver-tokio、module 分层，并有独立 property-tests。
- 支持 UDP/TCP active/passive、RFC4571 两字节与 `$` 四字节 framing、PS/TS/ES/raw payload、
  基础 RTCP、SSRC stream fallback、对讲、端口池、有界队列和部分 Ehome2/JT1078 类型。
- PS 已有 PSM 映射、重组上限和基础 H.264/H.265/AAC/G.711 路径。
- RTP 流程测试已覆盖 UDP ingest/egress、talkback、端口释放和真实 RTP packet 基础路径。
- 当前部分 GB core/property tests 实际验证 SIP/SDP roundtrip；它们只记录历史基线，RMV-04 后应删除
  或改写为 GB media session/SSRC/PT/container/framing 的纯媒体属性测试，不能计入最终媒体 DoD。

## 4. 关键差距

| ID | 差距 | 风险 | 目标章节 |
| --- | --- | --- | --- |
| GAP-API-01 | GB module 通过内部 HTTP/JSON 编排 RTP | 无类型、跨层、错误语义丢失 | 04、07 |
| GAP-ADM-01 | receiver 可能先分配 RTP 资源再鉴权 | Deny 后资源泄漏，违反硬约束 | 07 |
| GAP-LIFE-01 | 外部 open/connect 失败缺少统一回滚，stop 吞错 | 幽灵会话、端口与任务泄漏 | 07 |
| GAP-BOUND-01 | 仓库仍包含 SIP listener/parser/session/auth/SDP 逻辑 | 职责越界、形成第二套信令实现 | 08 |
| GAP-EXT-01 | 第三方系统缺少稳定的 typed media open/update/stop/query API | 继续依赖内部 REST/JSON 或信令 DTO | 04、08 |
| GAP-NEG-01 | 媒体接口未完整表达协商后的 transport/PT/codec/SSRC/time range | 回放、TCP、厂商媒体失败 | 04、08、09 |
| GAP-PS-01 | 无 PSM 时 codec fallback 固定，真实异常 PS 样例不足 | H.265/AAC 误判、花屏 | 05 |
| GAP-RTP-01 | 重排、来源重绑定、RTCP 生命周期与非标准 PT 尚不完整 | 丢包、串流、NAT 不稳 | 05、06 |
| GAP-JTT-01 | v2019 parser 存在，但 assembler/packetizer 主路径仍偏 v2013 | 宣称与真实能力不一致 | 05 |
| GAP-EH-01 | Ehome2 路径有限，Ehome5 未形成可信生产支持 | 错误能力声明 | 05 |
| GAP-SCALE-01 | 多个核心模块超过 800 行 | 难审查、兼容分支扩散 | 05、06、07 |
| GAP-TEST-01 | GB module 测试偏配置/路由，缺 typed media API/REST E2E | 回归发现过晚 | 11 |
| GAP-DOC-01 | 架构文档仍把 SIP/SDP/auth 描述为本项目能力 | 误导边界与发布 | 02 |

## 5. 已验证基线

以下命令在审计提交通过，后续 evidence 需记录完整输出与提交号：

```bash
cargo test -p cheetah-media-api -p cheetah-media-control-plane -p cheetah-media-grpc-adapter
cargo test -p cheetah-gb28181-core
cargo test -p cheetah-gb28181-driver-tokio
cargo test -p cheetah-gb28181-module
cargo test -p cheetah-gb28181-property-tests
cargo test -p cheetah-rtp-core
cargo test -p cheetah-rtp-driver-tokio
cargo test -p cheetah-rtp-module
cargo test -p cheetah-rtp-property-tests
cargo clippy -p cheetah-gb28181-core -p cheetah-gb28181-driver-tokio \
  -p cheetah-gb28181-module --tests -- -D warnings
cargo clippy -p cheetah-rtp-core -p cheetah-rtp-driver-tokio \
  -p cheetah-rtp-module --tests -- -D warnings
./dev-scripts/check_runtime_boundaries.sh
```

初次并行执行多个 Cargo 测试发生过共享 target artifact 竞争；串行复跑全部通过。CI 应按独立
target dir 并行，或对共享 target 的 build/test 进行调度，避免把基础设施竞争误判为产品失败。

## 6. 当前审计固定点

| 项目 | 当前值 | 说明 |
| --- | --- | --- |
| 审计基线提交 | `d6f3534979c8a7099949115f62c5f9234f57afdc` | 本计划开始执行时的 `main` HEAD |
| GB28181 core | `crates/protocols/gb28181/core` | 仍含历史 SIP/SDP/digest/message 代码，已标记为待移除 |
| GB28181 driver | `crates/protocols/gb28181/driver-tokio` | 当前以 RTP/RTCP 媒体 I/O 为主 |
| GB28181 module | `crates/protocols/gb28181/module` | 仍保留旧 REST 入口，需迁移到 typed media API |
| 参考实现路径 | `vendor-ref/ZLMediaKit`（缺失） | 工作区未挂载 |
| 参考实现路径 | `vendor-ref/simple-media-server`（缺失） | 工作区未挂载 |
| 参考实现路径 | `ABLMediaServer-src-2026-07-02/ABLMediaServer`（缺失） | 工作区未挂载 |

### 6.1 媒体能力现状清单

| 能力 | 状态 | 证据/位置 |
| --- | --- | --- |
| RTP/RTCP Sans-I/O core | CODE PASS | `crates/protocols/rtp/core` |
| RTP/RTCP tokio driver | CODE PASS | `crates/protocols/rtp/driver-tokio` |
| RTP module engine 接入 | CODE PASS | `crates/protocols/rtp/module` |
| GB28181 core Sans-I/O 媒体状态机 | PARTIAL | 旧 SIP/SDP 代码仍占位，需按 RMV-04 清理 |
| GB28181 driver-tokio | PARTIAL | 媒体 I/O 存在，SIP 遗留待清理 |
| GB28181 module | PARTIAL | 仍通过内部 HTTP/JSON 编排，API-01/02/03 后迁移 |
| PS/TS/ES 共享编解码 | CODE PASS | `cheetah-codec` 提供基础视图 |
| admission/fence/capacity | CODE PASS | 算法存在，待按 ADM-01/02 接入 GB 模块 |
| typed media API/RtpSessionApi | NOT_STARTED | P1 API-01 交付 |
| 第三方控制接口 | NOT_STARTED | P4 EXT-01..07 交付 |
| GB28181 信令解析/监听 | REMOVED_BY_DESIGN | 本项目不实现，由第三方信令系统负责 |

### 6.2 905 依赖门禁当前结论

依据 `02_905_closeout_and_dependency_gates.md` 逐项复核，当前状态如下：

| Gate | 当前状态 | 解除条件 |
| --- | --- | --- |
| CL904-02..05 | BLOCKED | 904 同候选证据签署 |
| media API revision | BLOCKED | 固定 RtpSessionApi/schema 并迁移调用方 |
| adapter compatibility | BLOCKED | 完成 compatibility suite |
| GRPC-01..10 | BLOCKED | 全 typed service + mapper |
| NODE/EVT/REC | BLOCKED | 生产 registry/event/recovery 装配 |
| CRED/FETCH/SEC | BLOCKED | mTLS、scope、rotation 与 leak test |
| REL-GB | BLOCKED | CI、双架构、SBOM、24h、签名证据齐全 |

> 注：以上 BLOCKED 状态不影响 P0/P1/P2 GB 媒体数据面子任务的独立开发；各 gate 解除条件在后续任务中逐步满足。
