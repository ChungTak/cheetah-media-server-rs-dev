# 01 · 当前基线、证据与缺口矩阵

## 1. 基线

本文件最初审计基线为提交 `aaae083`，最终交付基线为 `main` 分支合并 S10 后的最新提交。矩阵已于 S10 阶段更新为最终交付状态。

当前已确认（S10 最终状态）：

- `cheetah-media-api` 已加入 workspace，领域 crate 的依赖方向正确。
- `cheetah-sdk::MediaServices` 统一注册 control、publish/subscribe、record、snapshot、proxy、RTP provider，capability 与实际 provider 一致。
- Engine 默认 stream provider；各 feature module 初始化时覆盖注册真实 record/RTP/snapshot/proxy provider。
- `cheetah-server` 默认启用 RTMP，`media-control-full` feature 启用全部已交付模块。
- ZLM adapter 挂载 64 个 `/index/api/*` 路由，L1 真实生产路径、L2–L4  capability 显式返回。
- `MediaFacade::subscribe_events` 通过有界 `MediaEventBusApi` 真实分发事件。

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
| GAP-CAP-01 | capability 查询 | 已完成：`MediaServices` 统一 registry，`EngineMediaFacade` 动态查询 | 单一事实来源、生命周期感知 | P0 |
| GAP-STR-01 | media query | 已完成：`getMediaList`/`isMediaOnline` 返回真实 StreamInfo；输出 URL 由 `MediaUrlResolverApi` 提供 | 真实状态和输出 URL | P1 |
| GAP-SES-01 | session list | 已完成：`SessionDirectory` 全局唯一，支持 list/kick/close | 全局唯一真实 session directory | P0 |
| GAP-SES-02 | kick session | 已完成：native/ZLM `kick_session`/`close_stream` 精确关闭并返回结果 | 精确关闭并返回原因 | P0 |
| GAP-DP-01 | acquire publisher | 已完成：`MediaDataPlaneApi::open_frame_publisher` 提供 engine publisher 租赁 | engine publisher lease | P0 |
| GAP-DP-02 | open subscriber | 已完成：`SubscriberApi::subscribe` 提供真实 engine subscriber + `AVFrame` 数据面 | engine subscriber + Rust 数据面 | P0 |
| GAP-REC-01 | record CRUD | 已完成：record 幂等、有界分页、删除、事件发布 | 幂等、事件、完整查询 | P0 |
| GAP-REC-02 | record playback | 已完成：`RecordApi::control_record_playback` 支持 Pause/Resume/Scale/Seek 并校验状态 | VOD provider pause/resume/scale/seek | P1 |
| GAP-SNAP-01 | snapshot | 已完成：`SnapshotApi` + `cheetah-snapshot-module` 真实截图并返回安全 `FileHandle` | 真实抓图和安全文件 handle | P0 |
| GAP-FILE-01 | file download | 已完成：`MediaFileStoreApi` 注册公开文件，private 下载拒绝越权，支持 Range | 授权、过期、range、安全路径 | P0 |
| GAP-PROXY-01 | pull/push proxy | 已完成：`cheetah-proxy-module` 提供真实 pull/push proxy，带 SSRF 校验和 cancel | connector-backed provider | P0 |
| GAP-PROXY-02 | FFmpeg proxy | 已完成：typed `FFmpegJob` + 参数白名单/黑名单调度 | typed allowlist 调度 | P1 |
| GAP-RTP-01 | receiver 端口 | 已完成：RtpSession 实际 UDP/TCP 端口绑定并返回 ack | 实际绑定/分配并回报结果 | P0 |
| GAP-RTP-02 | active connect | 已完成：`connect_rtp_receiver` 主动 TCP/UDP connect | active TCP/UDP receiver | P1 |
| GAP-RTP-03 | sender/talk | 已完成：RTP sender 从 subscriber 取帧并实际发包；talk 双向音频复用 socket | subscriber→packet→socket 闭环 | P0 |
| GAP-RTP-04 | update/check | 已完成：`update_rtp_session` 支持 pause_check/timeout/SSRC 并发布事件 | SSRC/check/pause/resume | P1 |
| GAP-EVT-01 | media event bus | 已完成：有界 `MediaEventBusApi` + per-subscriber queue + cancel handle | 有界、可取消、可观测 | P0 |
| GAP-HOOK-01 | webhook | 已完成：`cheetah-webhook-dispatcher` 出站 dispatcher、ZLM 决策/通知 hook 映射、retry/熔断/SSRF | 全 hook 映射和策略 | P0 |
| GAP-NAT-01 | native routes | 已完成：22 条 native `/api/v1` 路由，动态 path matching + scope 鉴权 | 全 domain 能力路由 | P0 |
| GAP-SEC-01 | auth/RBAC | 已完成：`ControlAuthApi` + `MediaScope` + `Principal` + request context + audit logging | scope、审计、secret | P0 |
| GAP-CFG-01 | adapter 配置 | 已完成：native/ZLM adapter 配置化 enabled/prefix，支持 live reload | 独立启停、prefix、重启语义 | P1 |
| GAP-ZLM-01 | API 目录 | 已完成：64/64 ZLM `/index/api/*` 路由 catalog，L1–L4 分级 | 64/64 路由和状态 | P0 |
| GAP-ZLM-02 | wire compatibility | 已完成：端点专属 DTO、golden fixture 测试、错误码和字段别名对齐 ZLM | 字段、错误、别名锁定 | P0 |
| GAP-SIG-01 | signal contract | 已完成：四类信令 fake + production contract，全部使用真实 Engine/provider | 生产 provider E2E | P0 |
| GAP-TOOL-01 | Rust toolchain | 已完成：`rust-toolchain.toml` 固定 1.94.1，fmt/clippy/test 均通过 | 固定可用工具链 | P0 |

## 4. “完成”判定规则

一个能力只有同时满足下列条件才标记 Done：

1. domain trait 和 DTO 存在。
2. 生产 provider 接入真实 engine/module，不是 stub 或 fake。
3. native HTTP 或 Rust SDK 至少一个正式入口可调用；计划要求兼容时还包括 ZLM route。
4. 成功、错误、取消、超时、restart 和资源释放有测试。
5. capability 查询与实际行为一致。
6. server 的交付 feature profile 会编译并注册该 provider。

route 存在但返回 Unsupported，只能标记“路由已登记”；fake provider 测试通过，只能标记“接口可表达”。

## 5. 最终交付测试基线

- Rust 工具链固定为 `1.94.1`（`rust-toolchain.toml`），`cargo fmt --check` / `cargo clippy --workspace --tests` / `cargo test --workspace` 均通过。
- `cheetah-sdk` contract tests 编译并全部通过；signal contract 支持 fake 与 production 两套 support。
- 工作区 `cargo test --workspace` 通过全部测试（含新增 capability profiles、media module config、ZLM golden/interop、proxy module 等）。
- `cheetah-server` 默认、`media-control-full` 以及显式 feature 展开组合均编译通过。

