# 旧 GB28181 信令代码盘点（RMV-01）

本仓库不再负责 GB28181 信令（SIP/SDP/XML/MANSCDP/注册/心跳/目录/告警等），只通过结构化媒体 API 接收第三方信令系统协商后的媒体参数。本清单记录仍需退出生产路径并删除的历史信令实现，作为 RMV-02~RMV-05 的输入。

## 1. 盘点范围

- `crates/protocols/gb28181/core`： Sans-I/O 但仍是信令状态机/解析器
- `crates/protocols/gb28181/driver-tokio`： SIP UDP/TCP 监听与报文收发
- `crates/protocols/gb28181/module`： 含 `/devices`、`/invite`、`/bye` 等 REST 信令路由
- `crates/protocols/gb28181/fuzz`： 针对 SIP/REST JSON 的 fuzz target
- `crates/protocols/gb28181/testing/property-tests`： 针对 GB 信令会话的属性测试
- `apps/cheetah-server`： `gb28181` feature gate 与模块注册

## 2. `cheetah-gb28181-core`

| 文件 | 关键公共符号 | 待删除原因 |
| --- | --- | --- |
| `core/src/digest.rs` | `DigestParams`, `compute_md5_response` | SIP Digest 认证 |
| `core/src/message.rs` | `SipMessage`, `StartLine` | SIP 报文解析/序列化 |
| `core/src/sdp.rs` | `GbSdp` | SDP 协商辅助 |
| `core/src/session.rs` | `Gb28181Core`, `GbDevice`, `DialogState`, `Gb28181Command`, `Gb28181Event`, `SipSendAction` | 注册/INVITE/BYE/保活/对话状态机 |
| `core/src/error.rs` | `Gb28181CoreError`, `Gb28181Diagnostic` | 仅服务信令状态机 |
| `core/src/lib.rs` | 上述模块的 `pub mod` / `pub use` | 入口导出 |

依赖：`bytes`, `md5`, `serde`, `serde_json`, `thiserror`。

## 3. `cheetah-gb28181-driver-tokio`

| 文件 | 关键内容 | 待删除原因 |
| --- | --- | --- |
| `driver-tokio/src/lib.rs` | `Gb28181DriverConfig`（`listen_udp`/`listen_tcp` 5060）、`Gb28181DriverHandle`、UDP/TCP socket 绑定、`run_driver_loop`、SIP 报文解析与分发 | 绑定 SIP 端口并驱动信令 I/O |
| `driver-tokio/Cargo.toml` | 依赖 `tokio`（`net`）、`cheetah-gb28181-core` | 信令驱动依赖 |

## 4. `cheetah-gb28181-module`

| 文件 | 关键内容 | 待删除原因 |
| --- | --- | --- |
| `module/src/config.rs` | `Gb28181ModuleConfig`、`listen_addr`、`ControlOwner` 等 | 信令监听与配置 |
| `module/src/module.rs` | `Gb28181Module`、HTTP routes `/devices`, `/invite`, `/bye`、设备注册表 (`devices`)、`InviteSuccess`/`DeviceRegistered` 事件处理 | 提供 REST 信令接口并维护信令状态 |
| `module/src/lib.rs` | `pub use module::{Gb28181Module, Gb28181ModuleFactory}` | 入口 |
| `module/Cargo.toml` | 依赖 `cheetah-gb28181-core`、`cheetah-gb28181-driver-tokio` | 旧信令模块依赖 |

## 5. 测试与 Fuzz

| 文件 | 说明 | 处理方式 |
| --- | --- | --- |
| `testing/property-tests/tests/prop_gb_session.rs` | GB28181 session 属性测试 | 随 `core/session.rs` 删除 |
| `testing/property-tests/src/lib.rs` | property test 辅助 | 保留/迁移至媒体数据面 property tests |
| `fuzz/fuzz_targets/fuzz_sip_message.rs` | SIP 消息 fuzz | 删除 |
| `fuzz/fuzz_targets/fuzz_gb28181_rest_json.rs` | REST JSON fuzz | 删除或迁移为 `RtpSessionApi` JSON fuzz |
| `fuzz/Cargo.toml` | `fuzz_sip_message`/`fuzz_gb28181_rest_json` bin | 清理 |

## 6. 应用装配

| 文件 | 关键内容 | 处理方式 |
| --- | --- | --- |
| `apps/cheetah-server/src/main.rs:33,230` | `#[cfg(feature = "gb28181")] use cheetah_gb28181_module::Gb28181ModuleFactory;` 与 `register_module_factory` | 移除 `gb28181` feature 分支 |
| `apps/cheetah-server/Cargo.toml:22,49` | `gb28181 = ["dep:cheetah-gb28181-module", "rtp"]`、可选依赖 | 删除 feature 与依赖 |

## 7. 建议删除顺序

1. **RMV-02**（已完成）：将调用方从 `cheetah-gb28181-module` 内部 HTTP/JSON 迁移到 `RtpSessionApi`。
2. **RMV-03**：删除 `apps/cheetah-server` 中的 `gb28181` feature/注册/依赖；删除 `cheetah-gb28181-module` 的生产装配与 HTTP 路由；保留 feature-off 编译能力直到 RMV-04。
3. **RMV-04**：删除 `cheetah-gb28181-driver-tokio`、删除 `cheetah-gb28181-core` 的 `message.rs`/`sdp.rs`/`digest.rs`/`session.rs`/`error.rs`；将 `cheetah-gb28181-core` 重构为仅保留 GB 媒体会话状态（SSRC/PT/container/framing）或并入 `cheetah-rtp-core`。
4. **RMV-05**：更新 `SystemArchitecture.md`、`README`、capability matrix 与配置文档，确认生产制品不监听 SIP 端口、不链接信令 parser。

## 8. 保留能力边界

- **保留**：GB28181 媒体数据面（RTP/RTCP、PS/TS/ES、JTT1078、Ehome）应在 `cheetah-rtp-core` / `cheetah-rtp-driver-tokio` / `cheetah-rtp-module` 中继续完善。
- **保留**：ZLM/SMS/ABL 风格的 REST 媒体参数适配应留在 `cheetah-media-module` 或 `cheetah-rtp-module` 的边界 adapter 中，但只映射结构化媒体字段，不处理 SIP/SDP/XML。
- **删除**：SIP transport、REGISTER/Digest、dialog/transaction、SDP 解析、设备目录/心跳/告警、MANSCDP/XML。
