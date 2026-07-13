# 02. 媒体领域端口与模型

## 1. crate 设计

新增 `crates/sdk/cheetah-media-api/`，package 名称 `cheetah-media-api`。该 crate 只包含：

- `ids`：媒体身份、会话身份、任务身份和幂等键。
- `model`：媒体、轨道、播放器、发布者、代理、录制、快照、RTP 会话。
- `command`：操作参数。
- `error`：稳定错误码和错误上下文。
- `event`：内部领域事件。
- `port`：异步 runtime-neutral trait。
- `capability`：能力声明和版本。

所有跨边界模型实现 `Serialize`、`Deserialize`；枚举使用稳定的字符串 `serde` 表示，未知枚举值应能被 adapter 映射为 `Unknown` 或明确错误。

## 2. 媒体身份

```rust
pub struct MediaKey {
    pub vhost: VhostName,
    pub app: AppName,
    pub stream: StreamName,
    pub schema: Option<MediaSchema>,
}
```

约束：

- `vhost`、`app`、`stream` 必须非空，长度和字符集在构造时校验。
- `schema` 用于区分 `rtsp`、`rtmp`、`http-flv`、`hls`、`webrtc`、`ts`、`fmp4` 等访问视图；默认查询可以不带 schema。
- schema 不是 engine 中的唯一流身份。相同 vhost/app/stream 的不同输出视图应关联到同一媒体资源。
- 与当前 `StreamKey` 的默认映射是 `namespace=app`、`path=stream`；非默认 vhost 必须使用集中、可逆的 namespace 编码或扩展 bridge，不能在各 adapter 中自行拼接。
- 请求中同时出现 `vhost/app/stream` 与 `url` 时，服务端应解析并校验二者一致，避免路径注入和跨流误操作。

## 3. 核心只读模型

`StreamInfo` 至少包含：

- `key`、`origin`、`online`、`regist`、`created_at`、`last_activity_at`。
- `readers`、`publishers`、`bytes_in`、`bytes_out`、`duration_ms`。
- `tracks: Vec<TrackSummary>`，包括 media type、codec、clock rate、channels、width、height、bitrate 和参数集是否可用。
- `urls: Vec<MediaUrl>`，每个 URL 带 schema、地址、可用状态和过期时间。
- `metadata` 使用受限的键值结构，不允许把协议原始 JSON 作为任意 opaque 状态写入核心。

`SessionInfo` 至少包含 session id、kind（publisher/player/proxy/rtp sender/rtp receiver）、media key、remote/local endpoint、protocol、started_at、last_seen_at、bytes、state 和 close reason。

## 4. 端口分组

实现时可拆成多个 trait，再由 facade 组合；外部调用方依赖最小 trait。

```rust
pub trait MediaControlApi: Send + Sync {
    async fn get_media_list(&self, query: MediaQuery) -> Result<Page<StreamInfo>>;
    async fn get_media(&self, key: &MediaKey) -> Result<StreamInfo>;
    async fn is_media_online(&self, key: &MediaKey) -> Result<OnlineState>;
    async fn list_sessions(&self, query: SessionQuery) -> Result<Page<SessionInfo>>;
    async fn kick_session(&self, id: &SessionId, reason: CloseReason) -> Result<()>;
    async fn kick_stream(&self, key: &MediaKey, reason: CloseReason) -> Result<CloseReport>;
    async fn request_keyframe(&self, key: &MediaKey) -> Result<()>;
}

pub trait PublishSubscribeApi: Send + Sync {
    async fn acquire_publisher(&self, request: PublishRequest) -> Result<PublisherHandle>;
    async fn open_subscriber(&self, request: SubscribeRequest) -> Result<SubscriberHandle>;
    async fn close_handle(&self, id: &SessionId, reason: CloseReason) -> Result<()>;
}

pub trait RecordApi: Send + Sync {
    async fn start_record(&self, request: StartRecordRequest) -> Result<RecordTask>;
    async fn stop_record(&self, request: StopRecordRequest) -> Result<RecordTask>;
    async fn query_record_tasks(&self, query: RecordTaskQuery) -> Result<Page<RecordTask>>;
    async fn query_record_files(&self, query: RecordFileQuery) -> Result<Page<RecordFile>>;
    async fn delete_record_file(&self, request: DeleteRecordRequest) -> Result<()>;
    async fn control_record_playback(&self, command: RecordPlaybackCommand) -> Result<()>;
}

pub trait SnapshotApi: Send + Sync {
    async fn take_snapshot(&self, request: SnapshotRequest) -> Result<SnapshotHandle>;
    async fn query_snapshots(&self, query: SnapshotQuery) -> Result<Page<SnapshotInfo>>;
    async fn delete_snapshot_directory(&self, request: DeleteSnapshotRequest) -> Result<()>;
}

pub trait ProxyApi: Send + Sync {
    async fn create_pull_proxy(&self, request: PullProxyRequest) -> Result<ProxyInfo>;
    async fn delete_pull_proxy(&self, id: &ProxyId) -> Result<()>;
    async fn list_pull_proxies(&self, query: ProxyQuery) -> Result<Page<ProxyInfo>>;
    async fn create_push_proxy(&self, request: PushProxyRequest) -> Result<ProxyInfo>;
    async fn delete_push_proxy(&self, id: &ProxyId) -> Result<()>;
    async fn create_ffmpeg_proxy(&self, request: FfmpegProxyRequest) -> Result<ProxyInfo>;
}

pub trait RtpApi: Send + Sync {
    async fn open_rtp_receiver(&self, request: RtpReceiverRequest) -> Result<RtpSession>;
    async fn connect_rtp_receiver(&self, request: RtpConnectRequest) -> Result<RtpSession>;
    async fn open_rtp_sender(&self, request: RtpSenderRequest) -> Result<RtpSession>;
    async fn stop_rtp_session(&self, id: &RtpSessionId) -> Result<()>;
    async fn list_rtp_sessions(&self, query: RtpQuery) -> Result<Page<RtpSession>>;
    async fn update_rtp_session(&self, request: UpdateRtpRequest) -> Result<RtpSession>;
}
```

最终 facade 应组合以下能力：query、session control、publish/subscribe、proxy、record、snapshot、RTP、stream output、capability。任何未实现的 trait 方法返回 `MediaError::Unsupported { capability }`。

## 5. 参数模型最低要求

- `MediaQuery`：vhost/app/stream/schema/origin/online/page/page_size/order。
- `PublishRequest`：media key、protocol、origin、remote endpoint、lease、auth context、metadata。
- `SubscribeRequest`：media key、output schema、subscriber kind、start policy、auth context。
- `StartRecordRequest`：media key、format（mp4/ts/flv/hls/fmp4）、template、segment duration、max segments、storage policy、idempotency key。
- `RecordPlaybackCommand`：task/file、pause、resume、scale、seek；scale/seek 必须携带 value。
- `SnapshotRequest`：media key、timeout、format、quality、storage policy、capture policy。
- `PullProxyRequest`：source URL、destination media key、retry policy、heartbeat、timeout、transcode/filter policy、record policy。
- `PushProxyRequest`：source media key、destination URL、protocol options、retry policy。
- `RtpReceiverRequest`：media key、port allocation、IP、SSRC、RTCP、TCP mode、payload type、codec hints、reuse-port、timeout policy。
- `RtpSenderRequest`：media key、destination endpoint、SSRC/payload type、passive/active/talk mode、transport options。

## 6. 错误模型

领域错误至少包含：`InvalidArgument`、`Unauthenticated`、`PermissionDenied`、`NotFound`、`AlreadyExists`、`Conflict`、`Busy`、`Timeout`、`Unavailable`、`Unsupported`、`StorageFailed`、`ProtocolFailed`、`Internal`。每个错误带稳定 `code`、安全可展示 `message`、可选 `retryable`、`details` 和 request/correlation id。

adapter 负责把领域错误转换为自身协议；领域层不携带 HTTP 状态码，不返回“HTTP 200 但业务失败”的特殊语义。

