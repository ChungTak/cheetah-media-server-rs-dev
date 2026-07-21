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
| Typed gRPC | adapter 只注册 health/reflection，未消费固定 signaling contract | BLOCKED | CT-01..03 与 GRPC-01..10 |
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

## 4. 关键差距

| ID | 差距 | 风险 | 目标章节 |
| --- | --- | --- | --- |
| GAP-API-01 | GB module 通过内部 HTTP/JSON 编排 RTP | 无类型、跨层、错误语义丢失 | 04、07 |
| GAP-ADM-01 | receiver 可能先分配 RTP 资源再鉴权 | Deny 后资源泄漏，违反硬约束 | 07 |
| GAP-LIFE-01 | SIP 命令失败缺少统一回滚，stop 吞错 | 幽灵会话、端口与任务泄漏 | 07 |
| GAP-SIP-01 | REGISTER/MESSAGE/BYE 之外事务不完整，INVITE 2xx 后不发 ACK | 真实设备互操作失败 | 08 |
| GAP-SIP-02 | Digest 解析/挑战未形成安全验证闭环 | 未授权注册、重放 | 08、10 |
| GAP-XML-01 | 无完整 MANSCDP/XML/SN correlation | 当前 local 信令非生产级 | 08 |
| GAP-SDP-01 | SDP/Subject/time range/TCP setup/answer 校验不足 | 回放、TCP、厂商设备失败 | 08、09 |
| GAP-PARSE-01 | SIP TCP 总缓冲与 Content-Length 缺少硬上限 | OOM/DoS/报文错位 | 08、10 |
| GAP-PS-01 | 无 PSM 时 codec fallback 固定，真实异常 PS 样例不足 | H.265/AAC 误判、花屏 | 05 |
| GAP-RTP-01 | 重排、来源重绑定、RTCP 生命周期与非标准 PT 尚不完整 | 丢包、串流、NAT 不稳 | 05、06 |
| GAP-JTT-01 | v2019 parser 存在，但 assembler/packetizer 主路径仍偏 v2013 | 宣称与真实能力不一致 | 05 |
| GAP-EH-01 | Ehome2 路径有限，Ehome5 未形成可信生产支持 | 错误能力声明 | 05 |
| GAP-SCALE-01 | 多个核心模块超过 800 行 | 难审查、兼容分支扩散 | 05、06、07 |
| GAP-TEST-01 | GB module 测试偏配置/路由，缺真实 SIP/REST/E2E | 回归发现过晚 | 11 |
| GAP-DOC-01 | 架构文档声称的 ACK/SDP/auth/lease 与代码不一致 | 误导实施与发布 | 02 |

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
