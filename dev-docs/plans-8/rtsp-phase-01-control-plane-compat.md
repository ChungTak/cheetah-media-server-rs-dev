# Phase 01: RTSP 控制面与兼容模型

- 状态：计划中
- 范围：统一 RTSP 控制面解析、Transport 候选选择、Session 生命周期、Basic/Digest 鉴权、SDP 兼容和配置模型。
- 完成标准：不改动媒体发送路径也能先用 core/module 单元测试证明控制面行为稳定，后续 UDP/TCP/HTTP/multicast 都复用同一套选择和校验逻辑。

## 目标文件与模块

重点修改：

```text
crates/protocols/rtsp/core/src/core/method.rs
crates/protocols/rtsp/core/src/core/message.rs
crates/protocols/rtsp/core/src/core/transport.rs
crates/protocols/rtsp/core/src/core/connection.rs
crates/protocols/rtsp/core/src/core/sdp.rs
crates/protocols/rtsp/core/src/core/range.rs
crates/protocols/rtsp/module/src/config.rs
crates/protocols/rtsp/module/src/media.rs
crates/protocols/rtsp/module/src/sdp.rs
crates/protocols/rtsp/module/src/session.rs
crates/protocols/rtsp/module/src/module/request_dispatch.rs
crates/protocols/rtsp/module/src/module/response.rs
crates/protocols/rtsp/module/src/module/session_guard.rs
```

建议新增小模块：

```text
crates/protocols/rtsp/core/src/core/auth.rs
crates/protocols/rtsp/core/src/core/tunnel.rs
crates/protocols/rtsp/module/src/module/auth.rs
crates/protocols/rtsp/module/src/module/transport_selection.rs
crates/protocols/rtsp/module/src/module/session_lifecycle.rs
crates/protocols/rtsp/module/src/sdp_compat.rs
```

## 控制面目标行为

支持方法：

```text
OPTIONS
DESCRIBE
ANNOUNCE
SETUP
PLAY
PAUSE
RECORD
TEARDOWN
GET_PARAMETER
SET_PARAMETER
REDIRECT
GET      # HTTP tunnel only
POST     # HTTP tunnel only
```

`GET`/`POST` 只在 HTTP tunnel path 中作为 tunnel setup 处理，不能作为普通 RTSP method 暴露到 module request dispatcher。

响应规则：

- 所有响应尽量回带 `CSeq`。
- 有 session 的响应回带 `Session: <id>;timeout=<secs>`。
- DESCRIBE 成功回 `Content-Type: application/sdp`、`Content-Base`、`Content-Length`。
- SETUP 失败优先返回 461；session 不匹配返回 454；状态错误返回 455；多轨 aggregate SETUP 不明确返回 459。
- PLAY 成功回 `Range`、`RTP-Info`、`Session`。
- keepalive 允许 OPTIONS、GET_PARAMETER、SET_PARAMETER，默认刷新 idle deadline。

## 配置模型

扩展 `RtspModuleConfig`：

```rust
pub struct RtspModuleConfig {
    pub enabled: bool,
    pub listen: String,
    pub session_timeout_secs: u32,
    pub write_queue_capacity: usize,
    pub subscriber_queue_capacity: usize,
    pub subscriber_backpressure: BackpressurePolicy,
    pub start_from_keyframe: bool,
    pub bootstrap_max_frames: usize,
    pub rtp_mtu: usize,
    pub play_wait_source_timeout_ms: u64,
    pub auth: RtspAuthConfig,
    pub transport: RtspTransportConfig,
    pub sdp_compat: RtspSdpCompatConfig,
    pub alert_thresholds: RtspAlertThresholds,
    pub pull_jobs: Vec<RtspPullJobConfig>,
    pub push_jobs: Vec<RtspPushJobConfig>,
    pub relay_jobs: Vec<RtspRelayJobConfig>,
}
```

新增 auth：

```rust
pub struct RtspAuthConfig {
    pub enabled: bool,
    pub require_publish_auth: bool,
    pub realm: String,
    pub users: Vec<RtspAuthUserConfig>,
    pub allow_basic: bool,
    pub allow_digest: bool,
    pub nonce_ttl_secs: u32,
}
```

新增 transport config：

```rust
pub struct RtspTransportConfig {
    pub allow_udp: bool,
    pub allow_tcp_interleaved: bool,
    pub allow_http_tunnel: bool,
    pub allow_multicast: bool,
    pub allow_third_party_destination: bool,
    pub allow_default_interleaved_channels: bool,
    pub udp_port_range: Option<RtspPortRange>,
    pub multicast: RtspMulticastConfig,
    pub tunnel: RtspHttpTunnelConfig,
}
```

默认值：

- `play_wait_source_timeout_ms = 15_000`
- `auth.enabled = false`
- `auth.require_publish_auth = false`
- `auth.realm = "cheetah"`
- `transport.allow_udp = true`
- `transport.allow_tcp_interleaved = true`
- `transport.allow_http_tunnel = false`
- `transport.allow_multicast = false`
- `transport.allow_third_party_destination = false`
- `transport.allow_default_interleaved_channels = true`

## Transport 候选解析

`RtspTransport::parse_multiple` 已存在，应继续强化并被 module SETUP 复用。目标：

- 支持逗号分隔候选，保持原顺序。
- 支持 `RTP/AVP`、`RTP/AVP/UDP`、`RTP/AVP/TCP`。
- 支持参数：`unicast`、`multicast`、`client_port`、`server_port`、`port`、`interleaved`、`destination`、`source`、`ttl`、`layers`、`mode`、`ssrc`、`append`。
- 支持单值端口/通道自动推断第二值，但必须防止溢出。
- 参数大小写不敏感；响应输出使用规范大小写。
- 对未知参数保留到 compat map 或忽略，不要因为厂商私有参数直接拒绝。

Transport 选择顺序：

1. 过滤 module 配置禁止的 transport。
2. 过滤不符合 session mode 的 transport，例如 publish-side multicast 默认关闭。
3. 过滤第三方 destination，除非配置允许。
4. 如果客户端提供多个候选，优先选择配置优先级最高且可分配资源的候选。
5. 所有候选不可用时返回 461，并记录包含候选摘要的 debug 日志。

## Auth 设计

core 提供纯函数：

```rust
pub enum RtspAuthorization {
    Basic { username: String, password: String },
    Digest(RtspDigestAuthorization),
}

pub struct RtspDigestChallenge {
    pub realm: String,
    pub nonce: String,
    pub algorithm: RtspDigestAlgorithm,
}
```

行为规则：

- Basic 仅在 `allow_basic = true` 时接受。
- Digest 支持 MD5、`response = MD5(HA1:nonce:HA2)` 的基础兼容路径。
- nonce 由 module 通过 runtime random 或 deterministic test generator 生成，core 不访问系统随机源。
- DESCRIBE/PLAY/PULL 默认要求 auth 时必须认证；ANNOUNCE/RECORD 是否认证由 `require_publish_auth` 决定。
- outbound client 从 URL userinfo 或 job config 读取用户名密码，遇到 401 后重发对应 method。

## SDP 兼容策略

补强 server publish ingest：

- 接受 `\n` 或 `\r\n` 行结束。
- 支持 session-level 和 media-level `c=`、`b=`、`a=control:*`。
- 支持 absolute control URI 和相对 control URI。
- 支持 `trackID=0`、`trackID=1`、`streamid=0`、`stream=0`、裸数字 control。
- 支持 H264 `sprop-parameter-sets`、H265/H266 `sprop-vps/sprop-sps/sprop-pps`、AAC `config`、AAC LATM `cpresent/config`、AV1 config。
- 支持静态 payload type：0 PCMU、8 PCMA、14 MP3、5/6/16/17 ADPCM；是否扩展 G722/G723/AC3 取决于 `cheetah-codec` 是否新增 CodecId。
- 支持 MP2P/PS：先在 `cheetah-codec::ps` 补齐 bounded demux，再由 RTSP media compat 转为 AVFrame。

补强 server DESCRIBE egress：

- SDP 输出包含 `v=0`、`o=`、`s=`、`c=`、`t=0 0`、`a=control:*`、必要时 `a=range:npt=now-`。
- `Content-Base` 使用请求 URI 去掉 query 后的 canonical base。
- 每条 track 输出 `m=`、`a=rtpmap`、`a=fmtp`、`a=control:trackID=<n>`。
- 对外 URL 使用原始 host/port，不把内部 bind wildcard 泄漏给客户端。

## 具体任务

### 1.1 统一 RTSP 控制面解析与响应模型

- [ ] 增加 `RtspMethod::{Get, Post}` 或 tunnel-only parser，不让普通 dispatcher 误处理 HTTP tunnel。
- [ ] 为 request/response 增加 header lookup helper，避免 module 到处手写大小写匹配。
- [ ] 把 session 状态校验迁移到 `session_lifecycle.rs`，覆盖 ANNOUNCE/DESCRIBE/SETUP/PLAY/RECORD/PAUSE/TEARDOWN。
- [ ] 修正 PLAY `RTP-Info` 的 `rtptime`，使用当前 track RTP timestamp，而不是固定 `0`。
- [ ] 增加控制面单元测试：session mismatch、missing CSeq、multi Transport、aggregate SETUP、invalid state 不破坏已有 session。

### 1.2 增加 Basic/Digest auth 和 hook 接入点

- [ ] 在 core 新增 Basic/Digest Authorization parser 和 Digest response verifier。
- [ ] 在 module config 增加 auth 配置与校验。
- [ ] 在 DESCRIBE/PLAY/ANNOUNCE/RECORD 前插入 auth gate，publish auth 默认关闭。
- [ ] 生成 401 响应：`WWW-Authenticate: Digest realm="...", nonce="..."` 或 Basic。
- [ ] 增加 outbound client 401 retry 设计，实际发送逻辑在 Phase 04 落地。
- [ ] 增加测试：Basic 成功/失败、Digest 成功/nonce mismatch、publish auth disabled、auth failure 不创建 publisher lease。

### 1.3 强化 SDP 与 Transport 兼容解析

- [ ] 改造 SETUP 使用 `RtspTransport::parse_multiple`，淘汰 module 内字符串 `contains` 选择。
- [ ] 增加 Transport candidate selection 和响应 builder。
- [ ] 增加 SDP compat tests：absolute control、Content-Base relative control、LATM AAC、H265/H266 sprop、payload type fallback、malformed line ignore。
- [ ] 增加配置校验测试：端口范围、auth user、multicast group、tunnel limits、非法 job URL。
- [ ] 对 PS/MP2P 只先固定边界与测试 fixture；如果 `cheetah-codec::ps` 能力不足，记录到 Phase 03 补齐。

## 测试要求

- core 测试只验证纯 parser、auth、Transport、Session、Range、RTP-Info，不访问 engine。
- module 单元测试可构造 fake EngineContext，但不启动真实 socket。
- 兼容测试必须标明强行为断言还是 bounded health 断言。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-rtsp-core
cargo test -p cheetah-rtsp-core transport
cargo test -p cheetah-rtsp-core auth
cargo test -p cheetah-rtsp-core session
cargo clippy -p cheetah-rtsp-module --tests
cargo test -p cheetah-rtsp-module config
cargo test -p cheetah-rtsp-module state_mapping
```
