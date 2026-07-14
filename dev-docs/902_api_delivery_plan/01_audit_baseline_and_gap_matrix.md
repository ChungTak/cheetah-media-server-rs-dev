# 01 · 当前基线、证据与缺口矩阵

## 1. 基线

本计划编写时的审计基线为提交 `aaae083`。开始实现前必须重新执行本章命令；若源码已经变化，先更新矩阵，再修改代码。

当前已确认：

- `cheetah-media-api` 已加入 workspace，领域 crate 的依赖方向基本正确。
- `cheetah-sdk::MediaServices` 已能注册 control、publish/subscribe、record、snapshot、proxy、RTP provider。
- Engine 默认注册 stream provider，并通过 `EngineMediaFacade` 填充 record/snapshot/proxy/RTP stub。
- record module 初始化时会覆盖注册真实 record provider。
- RTP module 初始化时会覆盖注册真实 RTP provider。
- server 总是注册 native/ZLM media module，但默认 feature 只有 RTMP。
- ZLM adapter 当前挂载 23 个 `/index/api/*` 路由；901 明确列出的目录为 64 个。
- `MediaFacade::subscribe_events` 丢弃 sender 后返回成功。

## 2. 开工前复核命令

```bash
git status --short
git log -1 --oneline
rg -n 'unsupported|Unsupported|stub|TODO|todo!|unimplemented!' \
  crates/sdk/cheetah-media-api \
  crates/system/cheetah-engine/src/media_provider \
  crates/system/cheetah-media-module \
  crates/system/cheetah-record-module/src/media_provider.rs \
  crates/protocols/rtp/module/src/media_provider.rs
rg -n 'register_(control|publish_subscribe|record|snapshot|proxy|rtp)' crates apps
rg -o '"/api/[A-Za-z0-9_/-]+"' crates/system/cheetah-media-module/src/zlm.rs | sort -u
```

## 3. 能力矩阵

| ID | 能力 | 当前状态 | 目标 | 优先级 |
| --- | --- | --- | --- | --- |
| GAP-CAP-01 | capability 查询 | 静态 facade 与动态 registry 分离 | 单一事实来源、生命周期感知 | P0 |
| GAP-STR-01 | media query | 可查询，但 URL/统计/时间多为空或零 | 真实状态和输出 URL | P1 |
| GAP-SES-01 | session list | 合成 session ID | 全局唯一真实 session directory | P0 |
| GAP-SES-02 | kick session | Unsupported | 精确关闭并返回原因 | P0 |
| GAP-DP-01 | acquire publisher | Unsupported | engine publisher lease | P0 |
| GAP-DP-02 | open subscriber | Unsupported | engine subscriber + Rust 数据面 | P0 |
| GAP-REC-01 | record CRUD | feature 开启后部分可用 | 幂等、事件、完整查询 | P0 |
| GAP-REC-02 | record playback | Unsupported | VOD provider pause/resume/scale/seek | P1 |
| GAP-SNAP-01 | snapshot | 只有 stub | 真实抓图和安全文件 handle | P0 |
| GAP-FILE-01 | file download | Unsupported | 授权、过期、range、安全路径 | P0 |
| GAP-PROXY-01 | pull/push proxy | 只有 stub | connector-backed provider | P0 |
| GAP-PROXY-02 | FFmpeg proxy | 只有 stub | typed allowlist 调度 | P1 |
| GAP-RTP-01 | receiver 端口 | 忽略请求 IP/port | 实际绑定/分配并回报结果 | P0 |
| GAP-RTP-02 | active connect | Unsupported | active TCP/UDP receiver | P1 |
| GAP-RTP-03 | sender/talk | 创建 client，无生产 egress 证据 | subscriber→packet→socket 闭环 | P0 |
| GAP-RTP-04 | update/check | 大部分 Unsupported | SSRC/check/pause/resume | P1 |
| GAP-EVT-01 | media event bus | 只有类型，订阅假成功 | 有界、可取消、可观测 | P0 |
| GAP-HOOK-01 | webhook | 未实现出站 dispatcher | 全 hook 映射和策略 | P0 |
| GAP-NAT-01 | native routes | 仅部分路由 | 全 domain 能力路由 | P0 |
| GAP-SEC-01 | auth/RBAC | principal 恒为空 | scope、审计、secret | P0 |
| GAP-CFG-01 | adapter 配置 | prefix 固定、apply_config 空操作 | 独立启停、prefix、重启语义 | P1 |
| GAP-ZLM-01 | API 目录 | 23/64 | 64/64 路由和状态 | P0 |
| GAP-ZLM-02 | wire compatibility | 无 golden | 字段、错误、别名锁定 | P0 |
| GAP-SIG-01 | signal contract | fake-only，S0 后测试可编译 | 生产 provider E2E | P0 |
| GAP-TOOL-01 | Rust toolchain | `rust-toolchain.toml` 固定 1.94.1 并已安装 | 固定可用工具链 | P0 |

## 4. “完成”判定规则

一个能力只有同时满足下列条件才标记 Done：

1. domain trait 和 DTO 存在。
2. 生产 provider 接入真实 engine/module，不是 stub 或 fake。
3. native HTTP 或 Rust SDK 至少一个正式入口可调用；计划要求兼容时还包括 ZLM route。
4. 成功、错误、取消、超时、restart 和资源释放有测试。
5. capability 查询与实际行为一致。
6. server 的交付 feature profile 会编译并注册该 provider。

route 存在但返回 Unsupported，只能标记“路由已登记”；fake provider 测试通过，只能标记“接口可表达”。

## 5. 已知测试基线

S0 重新审计后的测试基线：

- Rust 工具链固定为 `1.94.1`（`rust-toolchain.toml`），`cargo fmt` / `cargo clippy` / `cargo test` 可运行。
- `cheetah-sdk` contract tests 编译通过；`FakeMediaProvider` 补齐 `get_rtp_session`；`RtpSenderRequest` 补齐 `codec_hint`。
- `cheetah-media-api`、`cheetah-engine`、`cheetah-record-module`、`cheetah-rtp-module` 共 73 个测试通过；`cheetah-media-module` 新增 7 个单元测试。
- 工作区 `cargo test --workspace --lib` 通过 306 个测试。

S0 必须先恢复 SDK contract tests 编译，再开始功能扩展。

