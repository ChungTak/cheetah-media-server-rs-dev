# Phase 01: Signaling Transport OME Compat

- **状态**: 已完成
- **目标**: 补齐 OME 风格 WebSocket/WHIP 信令入口与 `direction`/`transport` 候选策略兼容，使 OvenPlayer 风格客户端和历史 OME URL 能稳定接入。

## 实现范围

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| 现有 WHIP/WHEP 路由 | 已有/复用 | 复用 `module/src/http.rs` 现有 HTTP 信令入口 |
| OME URL 别名 | 已完成 | 已补 `/{app}/{stream}?direction=...` 兼容并复用现有 WHIP/WHEP 会话创建 |
| WebSocket 自定义信令 | 已完成 | 已完成 `request_offer`/`request-offer`、`answer`、`candidate`、`stop` 的 JSON schema、decoder、action 映射，`request_offer` session plan，抽象 handler，WebSocket text transport，`offer` response 渲染，独立 WebSocket listener/server loop，`ome_ws_listen` 配置校验，module start 接入，publish/play 媒体桥接，以及 driver `CreateOffer` candidate policy；真实客户端互操作归入 Phase 05 |
| `transport` 策略解析 | 已完成 | 已完成 query 解析、非法值拒绝、每请求 SDP candidate 过滤 |
| `DefaultTransport` / `TcpRelayForce` | 已完成 | 已完成模块配置、缺省 transport、relay-only 覆盖、WHIP/WHEP `Link: rel="ice-server"` 输出与 OME JSON `iceServers`/`ice_servers` 双字段渲染 |

## 参考 OME 行为

OME 同时支持：

- `ws[s]://host/app/stream?direction=send` 自定义信令 publish。
- `http[s]://host/app/stream?direction=whip` WHIP ingest。
- `?transport=udp|tcp|relay|udptcp|all` 控制 candidate 与 relay 信息输出。
- `TcpRelayForce=true` 时强制 relay-only 信令输出。

## 开发任务

### Task 01: URL 与 query 兼容层

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/compat.rs`
  - 修改: `crates/protocols/webrtc/module/src/http.rs`

验收点：

- `/{app}/{stream}?direction=send`
- `/{app}/{stream}?direction=whip`
- `/{app}/{stream}?direction=recv`
- `?transport=udp|tcp|relay|udptcp|all` 完成解析和校验。
- 未知 `direction`/`transport` 返回结构化错误。

### Task 02: WebSocket 自定义信令

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/http.rs`
  - 新增或修改: `crates/protocols/webrtc/module/src/*signaling*`
  - 已新增: `crates/protocols/webrtc/module/src/ome_signaling.rs`

验收点：

- 已完成 `request_offer` 与非标准 `request-offer` alias 解析。
- 已完成 `answer` 中嵌套 SDP 提取与 `ApplyAnswer` action 映射。
- 已完成 `candidate` 数组解析、空 candidate 忽略、remote candidate action 映射。
- 已完成 `stop` action 映射。
- 已完成 `request_offer` session plan：按 `direction` 映射 Publisher/Player，按播放/发布映射 offer sendonly/recvonly，生成 registry session 与 driver `CreateOffer` command。
- 已完成抽象 handler：`request_offer` 可发送 `CreateOffer`、等待 offer、抽取 SDP candidates 并渲染 OME `offer` JSON；`offer.id` 使用服务端会话 id，`answer`/`candidate`/`stop` 按该 id 校验后转发为 driver command。
- 已完成 OME WebSocket text transport：基于 `tokio-tungstenite` 做文本 JSON frame 与 OME message 的编解码适配。
- 已完成独立 OME WebSocket listener/server loop：保留 upgrade URL path/query、限制连接上限、设置握手超时，并接入 module start 的 `ome_ws_listen` 可选配置。
- 已完成 OME WebSocket publish/play 媒体桥接：发布侧复用 engine publish lease 与 simulcast/BWE 策略，播放侧复用 `spawn_play_subscriber`、bootstrap、B-frame 过滤和音频策略。
- 已完成服务端 `offer` response 渲染，包含 SDP、candidates、`iceServers` 与兼容旧字段 `ice_servers`。
- 已完成 driver `CreateOffer` 的 per-session candidate policy，OME WS `request_offer` 可按 `transport`/`TcpRelayForce` 复用同一候选过滤路径。
- 已完成 OME WebSocket driver `CreateOffer`/`ApplyRemoteAnswer`/`AddRemoteCandidate`/`StopSession` 信令编排与断开清理。
- OvenPlayer/OvenRtcTester 真实互操作回归归入 Phase 05。

### Task 03: `transport` 与 relay 响应策略

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/config.rs`
  - 修改: `crates/protocols/webrtc/driver-tokio/src/config.rs`
  - 修改: `crates/protocols/webrtc/module/src/http.rs`

验收点：

- 已完成 `udp|tcp|relay|udptcp|all` 的 query 解析与非法值拒绝。
- 已完成每请求 SDP 输出过滤：`udp` 仅保留 UDP 非 relay candidates，`tcp` 仅保留 TCP 非 relay candidates，`relay` 仅保留 relay candidates，`udptcp` 保留 UDP/TCP 非 relay candidates，`all` 保留全部 candidates。
- 已完成 `DefaultTransport` 配置项与缺省策略覆盖。

### Task 04: `TcpRelayForce` 与 `iceServers` 输出

- **状态**: 已完成
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/config.rs`
  - 修改: `crates/protocols/webrtc/module/src/http.rs`
  - 新增: `crates/protocols/webrtc/module/src/ome_signaling.rs`

验收点：

- 已完成 `TcpRelayForce` 对 OME session candidate 策略的 relay-only 覆盖。
- 已完成 `ome_ice_servers` 配置校验。
- 已完成 relay/all/TcpRelayForce 下 WHIP/WHEP `Link: rel="ice-server"` 下发，并通过 CORS 暴露 `Link`。
- 已完成 OME 自定义信令可复用的 JSON 输出：标准 `iceServers.username` 与旧兼容 `ice_servers.user_name` 双字段。

## 测试计划

```powershell
cargo test -p cheetah-webrtc-module ome
cargo test -p cheetah-webrtc-module whip
cargo clippy -p cheetah-webrtc-module
```
