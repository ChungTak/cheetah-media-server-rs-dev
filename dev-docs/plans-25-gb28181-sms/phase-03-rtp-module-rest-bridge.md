# Phase 03 — RTP Module、REST API 与跨协议桥接

- **状态**: 已完成
- **范围**: 新增 `cheetah-rtp-module`，接入 engine，实现 RTP server/client、REST API、主动/被动模式和 RTSP/RTMP/HLS 跨协议桥接
- **完成标准**: RTP 推流可发布为本地 stream，本地 stream 可转推到 RTP 远端，REST API 与配置可用，`cheetah-server` 可按 feature 启动模块

---

## 3.1 Module Factory 与配置

新增 crate：

```text
crates/protocols/rtp/module
```

module manifest：

- `module_id`: `rtp`
- `display_name`: `RTP Module`
- `config_namespace`: `rtp`
- `routes_prefix`: `/api/v1/rtp`
- capabilities: `Publish`、`Subscribe`、`HttpApi`、`BackgroundJob`

配置结构：

```rust
pub struct RtpModuleConfig {
    pub enabled: bool,
    pub listen_udp: Option<String>,
    pub listen_tcp: Option<String>,
    pub rtcp_listen_udp: Option<String>,
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub max_reassembly_bytes: usize,
    pub max_tracks: usize,
    pub idle_timeout_ms: u64,
    pub default_payload: RtpPayloadMode,
    pub allow_unaligned_payload: bool,
    pub pull_jobs: Vec<RtpClientJobConfig>,
}
```

---

## 3.2 SMS 兼容 REST API

路由：

```text
POST /api/v1/rtp/server/create
POST /api/v1/rtp/server/stop
POST /api/v1/rtp/client/create
POST /api/v1/rtp/client/start
POST /api/v1/rtp/client/stop
```

请求兼容字段：

- `port`
- `rtcpPort`
- `socketType`
- `transportMode`
- `payloadType`
- `ssrc`
- `appName`
- `streamName`
- `peerIp`
- `peerPort`
- `localIp`
- `localPort`
- `senderInfos`
- `receiver`

行为规则：

- `server/create` 可只开监听端口，也可直接绑定 `app/stream/ssrc`
- `client/create` 只创建会话和 socket，不自动 start
- `client/start` 开始主动连接或发送
- `client/stop` 和 `server/stop` 要释放 session 与资源
- 默认流路径兼容 `/live/{ssrc}`

---

## 3.3 RTP -> Engine -> 其他协议

RTP ingress：

```text
RTP packet
  -> rtp-driver/core
  -> cheetah-codec demux
  -> module publisher lease
  -> Engine StreamManager
  -> RTSP/RTMP/HLS module 订阅输出
```

要求：

- 接收后的本地流可被 RTSP、RTMP、HLS、HTTP-FLV、TS 等模块复用
- 多轨音视频 track 全部进入 engine
- codec 不被目标协议支持时，保留本地流并在目标输出侧诊断，不在 RTP ingest 阶段丢主路径

---

## 3.4 本地流 -> RTP 转推

RTP egress：

```text
Engine StreamManager
  -> rtp-module subscriber
  -> cheetah-codec mux/payload encoder
  -> rtp-driver/core
  -> remote UDP/TCP peer
```

要求：

- 支持 `ps`、`ts`、`es` 转推
- 首版不做转码，只做封装和 payload 变换
- `senderInfos` 可描述单路或多路远端
- send-only 与 send-recv 都要支持
- 远端断开后 job 可按配置重试

---

## 3.5 App 与 Workspace 接入

改动点：

- 根 `Cargo.toml` workspace members 加入 RTP crates
- `apps/cheetah-server/Cargo.toml` 增加 feature `rtp`
- `apps/cheetah-server/src/main.rs` 注册 `RtpModuleFactory`
- `config.example.yaml` 增加 `rtp` 示例配置
- `SystemArchitecture.md` 增加 RTP 协议位置和依赖方向

---

## 3.6 Module 测试

测试场景：

1. 模块默认配置合法
2. `server/create` 开启 UDP/TCP/both 成功
3. `client/create` + `start` 主动发送成功
4. RTP-PS 接收后可在 RTSP 播放
5. RTP-TS 接收后可在 HLS 播放
6. 本地 RTMP/RTSP 输入流可转推到 RTP 远端
7. `senderInfos` 多目标不互相拖累
8. 多轨 RTP 流进入 engine 后 track 数正确
9. `server/stop` 和 `client/stop` 正确释放资源

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-rtp-module --tests
cargo test -p cheetah-rtp-module
cargo clippy -p cheetah-server --features rtp
cargo test -p cheetah-server --features rtp
```
