# Phase 03 — Module、WHIP/WHEP、推流与播放

- **状态**: 已完成（含 engine 媒体桥）
- **完成位置**: `crates/protocols/webrtc/module/`、`apps/cheetah-server/src/main.rs`、`apps/cheetah-server/Cargo.toml`
- **范围**: 实现 `cheetah-webrtc-module`，提供 SMS-compatible HTTP API、WHIP/WHEP、WebRTC publish/play、engine 桥接、GOP bootstrap 秒开、基础鉴权和生命周期管理
- **完成标准**: 浏览器/客户端可通过 WHIP 或 SMS-style API 推流进入 engine；已有 engine 流可通过 WHEP 或 SMS-style API WebRTC 播放；session 可停止、超时清理、指标可观测
- **落地清单**:
  - `WebRtcModuleFactory` 提供 `cheetah-sdk` 模块清单：`module_id="webrtc"`，`config_namespace="webrtc"`，`routes_prefix="/api/v1/rtc"`，能力 `Publish/Subscribe/HttpApi/BackgroundJob`。
  - 配置层 `WebRtcModuleConfig` 覆盖 listen_udp/listen_tcp/public_ips/codec_profile/simulcast/BWE/RTX/Reorder/handshake/idle/migration TTL/bootstrap_frame_count/bootstrap_max_age_ms/wait_stream_timeout_ms 等参数；通过 `to_driver_config()` 派生 `WebRtcDriverConfig`。
  - HTTP 路由：`POST /publish`、`POST /play`、`POST /whip`、`POST /whep`、`GET /session/list`、`GET|PATCH|DELETE /session/{id}`，以及 Phase 05 的 `pull/push/p2p/echo` 占位接口（返回 501 + 结构化错误，但已挂载到路由表）。
  - WHIP/WHEP 响应 `201 Created` + `Content-Type: application/sdp` + `Location: /api/v1/rtc/session/{id}`，并附带 `Access-Control-Allow-Origin: *` 兼容 SMS。
  - WHIP/WHEP `PATCH /session/{id}` 解析 `application/trickle-ice-sdpfrag` body，按行抽出 `a=candidate:` 并通过 driver `AddRemoteCandidate` 命令喂入 core，缺少候选返回 `400 no_candidates`，未知 session 返回 `404`，已关闭 session 返回 `409`。
  - SMS publish 响应携带 `code:0`、`server`（默认 `"cheetah"`，可配置）、`sessionid`（实际 WebRTC session id）、`sdp`；SMS play 响应 `code:200` + `sdp`，与 SMS 字段保持兼容。
  - codec 政策：`browser` profile 禁止 `preferVideoCodec=h265`、`preferAudioCodec=aac`，命中即返回 `422 Unprocessable Entity` 并附 JSON 诊断；`device`/`passthrough` 放宽。
  - 模块生命周期：`init` 解析配置并申请模块自有 `CancellationToken`；`start` 起 driver、桥接 engine root cancel、spawn driver 事件 worker；`stop` 触发模块取消令牌，driver 与 worker 自动收尾，session 注册表清空，所有 publish/play 桥被关闭。`apply_config` 对参数变更返回 `ModuleRestartRequired`。
  - **Engine 媒体桥（`bridge.rs`）**：
    - **Ingress（推流）**：`WebRtcPublishBridge` 在 publish/WHIP HTTP 处理函数里先 `acquire_publisher` 获取 lease（重复发布返回 `409`），再调用 driver 接受 offer；driver 事件 worker 收到 `WebRtcMediaEvent::Frame` 时按 `WebRtcCodecKind` 映射到 `CodecId`、按 codec 选 `FrameFormat`、按 RTP timestamp 与 codec clock rate 写入 canonical pts/dts，并在第一次见到某个 MID 时调用 `update_tracks` 注册 `TrackInfo`。
    - **Egress（播放）**：play/WHEP HTTP 处理函数在 answer 投递成功后调用 `spawn_play`，启动 `spawn_play_subscriber` 异步循环：用 `BootstrapPolicy::live_tail(bootstrap_frame_count, Some(bootstrap_max_age_ms))` 订阅 engine，按帧映射回 `WebRtcSendFrame` 并通过 driver `SendFrame` 命令进入 core。core 的 `send_frame` 通过 `Rtc::writer(mid)` + `payload_params().find(codec)` 拿到 PT，再调用 `Writer::write` 把帧交给 str0m 打包发送。
    - 同一 session 关闭（HTTP DELETE / driver `SessionClosed` / 模块 stop）时同时清理 publish lease 与 play subscriber 的 cancel token。
  - Driver 事件 worker 把 `KeyframeRequest`(PLI/FIR) 翻译为 `stream_manager_api.request_keyframe()` 调用，让其他协议发布者刷新 IDR；`AnswerReady` 与 `Diagnostic`(AcceptOffer 失败) 通过 `AnswerDispatcher` 路由回挂起的 HTTP 请求；`SessionClosed` 清理注册表、关闭 publish bridge、取消 play subscriber，并对仍在等待的请求返回失败；`MediaTrackAdded` 记录 play 端 MID/kind 映射，供 subscriber 帧路由使用。
  - `apps/cheetah-server` 增加 `webrtc` feature，按既有模块的 cfg 模式注册 `WebRtcModuleFactory`，并未默认启用以避免在不需要 WebRTC 的部署中拉入 str0m 依赖。
  - 集成测试 `tests/module_lifecycle.rs` 通过 `EngineBuilder` 启动模块并直接调用 `ModuleHttpService::handle`，覆盖：session list、WHIP `201` + SDP + Location、SMS publish JSON、`browser` profile 拒绝 H.265、WHIP 引发 engine publisher_active=true 并第二次 WHIP 返回 409、PATCH 未知 session 返回 404、publish bridge 真实 lease engine、play bridge 真实 subscriber engine 共 8 个端到端用例。
  - **Phase 04 后续闭环**：simulcast 多层选择策略已落到 `bridge::SimulcastSelection`（`Highest` / `Lowest` / `Rid(name)` / `Adaptive`），未选中的 RID 在到达 engine 之前就被 drop；`Adaptive` 走 `min(bwe, remb)` 双轨动态降层 + NACK storm 触发强制最低层。详见 phase-04。

## 3.1 Module 文件结构

```text
crates/protocols/webrtc/module/src/
  lib.rs
  config.rs
  module.rs
  http.rs
  api.rs
  compat.rs
  session.rs
  publish.rs
  play.rs
  whip.rs
  whep.rs
  codec_policy.rs
  bootstrap.rs
  jobs.rs
  metrics.rs
```

职责：

- `module.rs`: `WebRtcModuleFactory`、manifest、start/stop/apply_config。
- `http.rs`: `ModuleHttpService` 实现和 route dispatch。
- `api.rs`: SMS-style request/response DTO。
- `compat.rs`: SMS 字段别名、SDP 兼容预处理、response 兼容。
- `session.rs`: module session registry。
- `publish.rs`: WebRTC ingress -> engine publisher。
- `play.rs`: engine subscriber -> WebRTC egress。
- `whip.rs` / `whep.rs`: 标准 HTTP 信令语义。
- `codec_policy.rs`: profile、prefer codec、peer SDP 过滤。
- `bootstrap.rs`: GOP 秒开策略。
- `jobs.rs`: Phase 05 client/P2P job 预留。

## 3.2 Module lifecycle

`WebRtcModule` 字段：

```rust
pub struct WebRtcModule {
    state: ModuleState,
    config: WebRtcModuleConfig,
    ctx: Option<EngineContext>,
    driver_handle: Arc<Mutex<Option<Arc<WebRtcDriverHandle>>>>,
    cancel_token: Option<CancellationToken>,
    sessions: Arc<Mutex<HashMap<WebRtcSessionId, WebRtcModuleSession>>>,
}
```

启动流程：

1. `init` 解析配置并保存 `EngineContext`。
2. `init` 创建 module-scoped cancellation token，确保 HTTP service 捕获到可取消 token。
3. `start` 根据配置启动 driver。
4. `start` spawn driver event worker。
5. `start` spawn session cleanup worker。
6. `stop` cancel module token、关闭 driver、清空 sessions、释放 publisher/subscriber。

配置变更：

- 监听地址、candidate、codec profile、queue、BWE、simulcast、RTX 参数变更返回 `ModuleRestartRequired`。
- 纯观测参数可 `Immediate`，但首版可以全部 restart，保持简单。

## 3.3 HTTP API

Routes：

```text
POST   /play
POST   /publish
POST   /whep
POST   /whip
DELETE /session/{session_id}
PATCH  /session/{session_id}
GET    /session/list
GET    /session/{session_id}
```

由于 module mount prefix 是 `/api/v1/rtc`，完整路径为：

```text
/api/v1/rtc/play
/api/v1/rtc/publish
/api/v1/rtc/whep
/api/v1/rtc/whip
```

SMS-style publish body：

```json
{
  "appName": "live",
  "streamName": "demo",
  "sdp": "v=0...",
  "preferVideoCodec": "h264",
  "preferAudioCodec": "opus",
  "enableDtls": 1
}
```

SMS-style publish response：

```json
{
  "code": 0,
  "server": "cheetah",
  "sessionid": "webrtc-...",
  "sdp": "v=0..."
}
```

SMS-style play body：

```json
{
  "appName": "live",
  "streamName": "demo",
  "enableDtls": 1,
  "sdp": "v=0..."
}
```

SMS-style play response：

```json
{
  "code": 200,
  "sdp": "v=0..."
}
```

WHIP/WHEP：

- Request `Content-Type` 必须接受 `application/sdp`。
- Response status `201 Created`。
- Response `Content-Type: application/sdp`。
- Response `Location: /api/v1/rtc/session/{session_id}`。
- CORS header 按项目 HTTP module 统一策略处理；如无统一策略，首版加 `Access-Control-Allow-Origin: *` 兼容 SMS。

## 3.4 Publish 数据流

```text
POST /api/v1/rtc/publish or /whip
  -> parse app/stream/sdp
  -> acquire publisher lease
  -> create driver session role=Publisher
  -> return local SDP answer
  -> driver event MediaFrame/RtpPacket
  -> cheetah-codec ingress normalization
  -> PublisherSink.update_tracks
  -> PublisherSink.push_frame
```

发布规则：

- 先 acquire publisher lease，再创建可接收媒体的 session，避免绕过单发布者语义。
- 如果 `StreamKey` 已有发布者，返回 `409 Conflict`。
- tracks 未 ready 前可缓存少量 frame，但必须 bounded。
- track ready 后先 `update_tracks` 再 `push_frame`。
- session close 时关闭 sink 并 release lease。

发布缓存：

```yaml
publish_pending_frame_capacity: 64
publish_track_ready_timeout_ms: 5000
```

如果超时仍无 track：

- 关闭 session。
- 返回或记录 `TrackReadyTimeout`。

## 3.5 Play 数据流

```text
POST /api/v1/rtc/play or /whep
  -> parse app/stream/sdp
  -> check stream exists or wait short timeout
  -> subscribe with bootstrap policy
  -> create driver session role=Player
  -> return local SDP answer
  -> subscriber recv AVFrame
  -> cheetah-codec egress view
  -> driver SendFrame
  -> str0m writer / RTP mode
```

播放规则：

- 首版默认只播放 live stream。
- 如果 stream 不存在，SMS-style 可配置等待 `wait_stream_timeout_ms`，默认 3000 ms。
- WHEP 如果 stream 不存在，返回 `404` 或 `503`，按配置选择。
- subscribe queue 必须 bounded。
- 慢 WebRTC peer 不得拖慢 engine publisher；driver queue 满时按策略丢包或关闭 session。

SubscriberOptions：

```rust
SubscriberOptions {
    queue_capacity: config.play_subscriber_queue_capacity,
    bootstrap_policy: BootstrapPolicy::live_tail(
        config.bootstrap_frame_count,
        Some(Duration::from_millis(config.bootstrap_max_age_ms)),
    ),
    ..Default::default()
}
```

## 3.6 GOP 秒开

目标：

- WebRTC 播放端尽快收到可解码关键帧。
- 不在 WebRTC module 内私自实现参数集缓存。

策略：

1. engine ring 提供最近帧。
2. module 订阅时启用 keyframe bootstrap。
3. `cheetah-codec` egress contract 检查 access unit boundary。
4. `ParameterSetCache` 为 H264/H265/H266 keyframe 补发 VPS/SPS/PPS。
5. 如果 ring 中无 keyframe：
   - 向 stream manager 发 `request_keyframe`。
   - 可配置 fallback：等待 keyframe 或发送音频-only。

配置：

```yaml
bootstrap_frame_count: 150
bootstrap_max_age_ms: 5000
bootstrap_wait_keyframe_ms: 2000
bootstrap_fallback_audio_only: true
```

测试：

- 新 WebRTC play session 从最近 keyframe 开始。
- keyframe 缺参数集时 codec 补发。
- 没有 keyframe 时触发 `request_keyframe`。
- discontinuity 后必须等待新的 keyframe 或明确 fallback。

## 3.7 Codec policy

输入字段：

- global `codec_profile`
- per request `preferVideoCodec`
- per request `preferAudioCodec`
- remote SDP capabilities

决策顺序：

1. remote SDP 是否支持。
2. module profile 是否允许。
3. request prefer codec 是否可满足。
4. engine stream tracks 是否存在。
5. 不能满足时返回明确错误，不转码。

错误示例：

```json
{
  "code": 422,
  "error": "unsupported_codec",
  "message": "remote SDP does not accept H265 in browser profile"
}
```

## 3.8 Session registry

`WebRtcModuleSession`：

```rust
pub struct WebRtcModuleSession {
    pub id: WebRtcSessionId,
    pub stream_key: StreamKey,
    pub direction: WebRtcDirection,
    pub api_kind: WebRtcApiKind,
    pub state: WebRtcModuleSessionState,
    pub created_at_micros: u64,
    pub last_activity_at_micros: u64,
    pub publisher: Option<WebRtcPublisherHandle>,
    pub subscriber_cancel: Option<CancellationToken>,
}
```

session states：

- `Created`
- `Signaling`
- `Connecting`
- `Connected`
- `Closing`
- `Closed`
- `Failed`

cleanup：

- handshake timeout。
- idle timeout。
- HTTP DELETE。
- module stop。
- driver close event。

## 3.9 Phase 03 测试要求

命令：

```text
cargo fmt
cargo clippy -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-core
cargo clippy -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-driver-tokio
cargo clippy -p cheetah-webrtc-module --tests
cargo test -p cheetah-webrtc-module
```

module tests：

- `sms_publish_accepts_app_stream_sdp_and_returns_answer`
- `sms_play_accepts_enable_dtls_compat_field`
- `whip_returns_201_sdp_and_location`
- `whep_returns_201_sdp_and_location`
- `publish_rejects_second_publisher_with_conflict`
- `publish_updates_tracks_before_first_frame`
- `play_waits_for_stream_then_subscribes`
- `play_returns_404_when_stream_missing_and_wait_disabled`
- `delete_session_closes_driver_and_releases_resources`
- `module_stop_cancels_http_created_sessions`
- `codec_policy_rejects_h265_for_browser_without_peer_support`
- `bootstrap_uses_keyframe_and_parameter_set_replay`

interop smoke：

- Chrome WHIP publish H264/Opus。
- Chrome WHEP play H264/Opus from RTSP or RTMP source。
- SMS SDP fixture publish/play request。

## 3.10 Phase 03 验收标准

- WebRTC publish/play 主链路打通 engine。
- WHIP/WHEP 返回符合标准的基本 HTTP 语义。
- SMS-style API 字段兼容。
- GOP bootstrap 行为可测。
- module 不直接依赖 `tokio::net`、`tokio::time`、`tokio::sync`。
- 同一 stream 单发布者语义不被绕过。

