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
