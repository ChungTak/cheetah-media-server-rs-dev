# Phase 04: RTSP Client 与转发任务

- 状态：计划中
- 范围：新增 outbound RTSP client driver，支持远端 pull、远端 push 和静态转发 job。
- 完成标准：配置 RTSP pull job 后，远端 RTSP 源可写入本地 engine stream；配置 RTSP push job 后，本地 engine stream 可推送到远端 RTSP server；relay job 可把远端源转发到远端目标，且可停止、可重试、有边界。

## 目标文件与模块

重点修改：

```text
crates/protocols/rtsp/driver-tokio/src/lib.rs
crates/protocols/rtsp/driver-tokio/src/server/mod.rs
crates/protocols/rtsp/module/src/config.rs
crates/protocols/rtsp/module/src/module.rs
crates/protocols/rtsp/module/src/session.rs
crates/protocols/rtsp/module/src/media.rs
crates/protocols/rtsp/module/src/sdp.rs
```

建议新增：

```text
crates/protocols/rtsp/driver-tokio/src/client/mod.rs
crates/protocols/rtsp/driver-tokio/src/client/connection.rs
crates/protocols/rtsp/driver-tokio/src/client/command.rs
crates/protocols/rtsp/driver-tokio/src/client/http_tunnel.rs
crates/protocols/rtsp/module/src/module/client_pull.rs
crates/protocols/rtsp/module/src/module/client_push.rs
crates/protocols/rtsp/module/src/module/relay.rs
crates/protocols/rtsp/module/tests/rtsp_pull_job.rs
crates/protocols/rtsp/module/tests/rtsp_push_job.rs
crates/protocols/rtsp/module/tests/rtsp_relay_job.rs
```

## Outbound Client Driver

Driver API 目标：

```rust
pub enum RtspClientMode {
    Pull,
    Push,
}

pub struct RtspClientDriverConfig {
    pub transport_preference: Vec<RtspRtpTransportKind>,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub keepalive_interval_ms: u64,
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub udp_port_range: Option<RtspPortRange>,
    pub http_tunnel: RtspHttpTunnelClientConfig,
}
```

Client event：

```rust
pub enum RtspClientEvent {
    Connected,
    Closed { reason: String },
    StateChanged { state: RtspClientState },
    DescribeSdp { sdp: String },
    TrackSetup { track_id: TrackId, transport: RtspTransportSelection },
    Rtp { track_id: TrackId, packet: RtpPacket },
    Rtcp { track_id: TrackId, packets: Vec<RtcpPacket> },
    AuthChallenge { realm: String, nonce: String },
}
```

Client command：

```rust
pub enum RtspClientCommand {
    Start,
    SendFrame(Arc<AVFrame>),
    RefreshTracks(Vec<TrackInfo>),
    Pause,
    Teardown,
    Shutdown,
}
```

实际类型可调整，但必须保持 driver 不依赖 engine。

## Pull Job

配置：

```rust
pub struct RtspPullJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_stream_key: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub transport_preference: Vec<RtspRtpTransportKind>,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}
```

流程：

```text
supervisor
  -> parse source_url and target_stream_key
  -> start outbound client in Pull mode
  -> OPTIONS
  -> DESCRIBE, handle 401 retry if needed
  -> parse SDP to TrackInfo
  -> acquire publisher lease
  -> SETUP tracks using selected transport
  -> PLAY
  -> RTP/RTCP ingest through same depacketizer path as server publish
  -> sink.push_frame
  -> close/retry/release lease
```

规则：

- 目标 stream 已有 active publisher 时停止该 job，不抢占。
- DESCRIBE 成功但无 supported tracks，按配置停止或退避；默认退避。
- 远端 401 只重试一次同 method，避免认证循环。
- 远端 session timeout 通过 OPTIONS 或 GET_PARAMETER keepalive。
- source disconnect 后释放 lease，按退避重试。

## Push Job

配置：

```rust
pub struct RtspPushJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_stream_key: String,
    pub target_url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub transport_preference: Vec<RtspRtpTransportKind>,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}
```

流程：

```text
supervisor
  -> wait source stream snapshot
  -> subscribe source stream with GOP bootstrap
  -> start outbound client in Push mode
  -> OPTIONS
  -> ANNOUNCE generated SDP
  -> SETUP tracks using selected transport
  -> RECORD
  -> packetize AVFrame to RTP
  -> send RTP/RTCP through selected transport
  -> source end/remote close -> retry
```

规则：

- push job 不获取 publisher lease，因为它只订阅本地 stream。
- source 不存在时等待或退避，不阻塞 module stop。
- track 变化时，如果远端不支持动态更新，重建 RTSP session。
- 推送端默认从关键帧/GOP bootstrap 开始，避免远端以 delta frame 起流。

## Relay Job

Relay 是配置便利层，不引入新的媒体模型：

```rust
pub struct RtspRelayJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_url: String,
    pub local_stream_key: Option<String>,
    pub transport_preference: Vec<RtspRtpTransportKind>,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}
```

执行策略：

- 如果 `local_stream_key` 存在：展开为 pull job + push job，便于本地其他协议订阅。
- 如果 `local_stream_key` 不存在：内部生成隐藏 stream key，但仍通过 engine `AVFrame + TrackInfo` 规范化，不做 RTP packet 盲转发。
- 不做跨连接裸 RTP fast path，避免绕过时间戳归一化、权限和发布租约。

## Transport Preference

默认 outbound 选择：

```text
Pull:  TCP interleaved -> UDP -> HTTP tunnel -> multicast
Push:  TCP interleaved -> UDP -> HTTP tunnel
```

可配置覆盖。对远端 SETUP 返回 461 时尝试下一个 transport；对认证失败、404、unsupported codec 不切换 transport。

## 具体任务

### 4.1 新增 outbound RTSP client driver

- [x] 新增 `driver-tokio/src/client`，复用 `cheetah-rtsp-core` response decoder 和 interleaved parser。（新增 `client/mod.rs`、`client/connection.rs`、`client/command.rs`、`client/auth.rs`，并对外导出 `start_tcp_client` 与 command/event API）
- [x] 实现 TCP direct client：connect、send request、parse response、parse interleaved RTP/RTCP。（实现 `RtspClientHandle` + 事件循环；支持 `SendRequest`、`SendInterleaved`、`Close`）
- [x] 实现 UDP client endpoint：SETUP 前分配 local RTP/RTCP ports，SETUP 后绑定 server_port target 并打洞。（新增 `client/udp.rs`，提供 `allocate_udp_endpoint`、`configure_udp_remote_and_punch`、`spawn_udp_receive_tasks`，并补端口对分配/打洞/收包事件测试）
- [x] 实现 HTTP tunnel client：GET/POST 双连接、cookie、base64 POST、GET 读响应和 interleaved media。（新增 `client/http_tunnel.rs` 与 `start_http_tunnel_client`，实现 GET/POST 开 tunnel 握手、POST base64 写入、GET RTSP response/interleaved 解析、header/status 错误处理）
- [x] 实现 Basic/Digest 401 retry hook。（新增 `authorization_header_from_response`，支持 Basic 与 Digest(MD5) challenge 生成 Authorization header）
- [x] 增加 driver client tests：OPTIONS/DESCRIBE/SETUP/PLAY 状态机、TCP interleaved 收包、UDP 收包、HTTP tunnel 收包、auth retry。（新增 TCP 与 HTTP tunnel 的 `OPTIONS->DESCRIBE->SETUP->PLAY` 状态机测试；已覆盖 TCP interleaved、UDP endpoint/收包、HTTP tunnel 收包、auth retry）

### 4.2 实现 RTSP pull jobs

- [x] 扩展 `RtspModuleConfig`，加入 `pull_jobs` 和校验。（新增 `RtspPullJobConfig`/`RtspPullTransport`，补齐 name/source_url/target_stream_key/transport/backoff/auth 组合校验与单测）
- [x] module start 时为 enabled pull job 启动 supervisor，stop 时取消并等待退出。（新增 `module/client_pull.rs`；`run_event_loop` 接入 pull supervisor 启停与 join 等待；新增 `tests/rtsp_pull_job.rs` 验证 module start/stop 不悬挂）
- [x] pull job DESCRIBE SDP 后生成 tracks，并 acquire publisher lease。（新增 pull client OPTIONS/DESCRIBE 控制面握手，DESCRIBE SDP 解析并 `sink.update_tracks`；成功后保持 lease 存活至会话退出；补 `tests/rtsp_pull_job.rs` 覆盖 tracks 可见与 lease 生命周期，stop 阶段 supervisor abort+join 防悬挂）
- [x] pull job RTP 复用 server publish depacketize/timestamp normalize 路径。（`module/publish.rs` 新增 `ingest_publish_rtp_packet` 公共入口并保留 `ingest_publish_rtp_payload` 包装，pull job 完成 TCP interleaved `SETUP/PLAY` 后将 RTP 喂入同一路径；新增 `tests/rtsp_pull_job.rs::pull_job_tcp_interleaved_rtp_ingest_reuses_publish_pipeline` 回归）
- [x] 实现退避、session timeout keepalive、lease release、目标占用停止。（pull supervisor 改为指数退避并受 `max_retry_backoff_ms` 上限约束；解析 `Session: ...;timeout=N` 后发送 `GET_PARAMETER` keepalive；`acquire_publisher` 冲突改为 stop-job 非重试；`sink.close()` 作为主释放路径并保留 release fallback；新增 `pull_job_sends_keepalive_from_session_timeout` 与 `pull_job_target_occupied_stops_without_retry` 回归）
- [x] 增加测试：远端 synthetic RTSP server -> local engine -> RTMP/RTSP play。（新增 `tests/rtsp_pull_job.rs::pull_job_remote_rtsp_source_restreams_to_local_rtsp_and_rtmp_play`，使用 pull-job 从 synthetic RTSP source 拉流并验证本地 RTSP 播放与 RTMP 播放都能收到视频媒体帧）

### 4.3 实现 RTSP push jobs 和 relay jobs

- [x] 扩展 `RtspModuleConfig`，加入 `push_jobs`、`relay_jobs` 和校验。（新增 `RtspPushTransport`、`RtspPushJobConfig`、`RtspRelayJobConfig`；`RtspModuleConfig` 默认值和校验接入 `push_jobs`/`relay_jobs`；补齐 URL/重试参数/凭据组合/transport 去重/跨 job 名称冲突 等回归单测）
- [x] push job 订阅 source stream，生成 ANNOUNCE SDP。（新增 `module/client_push.rs`：push supervisor 生命周期、等待 source stream tracks、`subscriber_api.subscribe` 建立订阅、outbound client `OPTIONS -> ANNOUNCE` 控制面握手与错误分类重试；新增 `tests/rtsp_push_job.rs` 覆盖 lifecycle 取消与 ANNOUNCE SDP 回归）
- [x] push job SETUP/RECORD 后 packetize AVFrame 并发送 RTP/RTCP。（`module/client_push.rs` 已接入 `ANNOUNCE -> SETUP -> RECORD` 控制面、track/channel 状态、interleaved RTP 发送与 RTCP SR 发送；新增 `tests/rtsp_push_job.rs::push_job_setup_record_then_sends_interleaved_rtp_and_rtcp` 回归）
- [x] source track 变化时按配置重建远端 session。（push 会话增加 source snapshot 轮询与轨道形态对比；检测到 track 变化后退出当前会话并触发 supervisor 重连重建 `ANNOUNCE -> SETUP -> RECORD`；新增 `tests/rtsp_push_job.rs::push_job_rebuilds_session_when_source_tracks_change` 回归）
- [x] relay job 展开为 pull + push，隐藏 stream key 必须可观测且生命周期受 job 管理。（新增 `module/client_relay.rs` relay supervisor；relay 配置展开为 pull/push 子任务，`local_stream_key` 为空时生成并暴露 `__relay/<job-name>` 隐藏 stream key；module 事件循环接入 relay supervisor 启停与 join 回收；新增 `tests/rtsp_relay_job.rs::relay_job_hidden_stream_is_observable_and_forwards_to_remote_target` 覆盖 remote source -> hidden local stream -> remote target）
- [x] 增加测试：local engine -> synthetic remote RTSP server；remote -> local -> remote relay；远端断开重试；module stop 无悬挂任务。（`rtsp_push_job` 已覆盖 local engine -> synthetic remote RTSP server 与 module stop 生命周期；`rtsp_relay_job` 已覆盖 remote -> local(hidden stream) -> remote relay，以及 source 远端断开后 relay 重试重连回归）

## 测试要求

- outbound client driver tests 不依赖 engine。
- pull/push/relay module tests 必须验证 cancellation，避免静态 job 阻塞 stop。
- job 重试测试要使用短 backoff，但断言有最大重试间隔和不会忙循环。
- auth retry tests 覆盖 URL userinfo 与 job username/password 两种来源。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-rtsp-driver-tokio
cargo test -p cheetah-rtsp-driver-tokio client
cargo clippy -p cheetah-rtsp-module --tests
cargo test -p cheetah-rtsp-module rtsp_pull_job
cargo test -p cheetah-rtsp-module rtsp_push_job
cargo test -p cheetah-rtsp-module rtsp_relay_job
```
