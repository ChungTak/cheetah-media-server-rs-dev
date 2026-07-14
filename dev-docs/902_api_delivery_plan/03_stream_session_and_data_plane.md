# 03 · Stream、Session 与 Rust 数据面

## 1. 目标

第三方项目必须能够查询真实流、精确关闭一个 session、创建发布/订阅关系，并在同进程 Rust 集成时收发 `AVFrame + TrackInfo`。HTTP 调用者不直接获得内存 frame channel，而是获得协议 URL 或 RTP session。

## 2. Session Directory

新增 runtime-neutral `MediaSessionDirectoryApi`，由 engine 实现，记录所有对外可管理 session：

```rust
pub struct MediaSessionRecord {
    pub session_id: SessionId,
    pub kind: SessionKind,
    pub media_key: MediaKey,
    pub protocol: String,
    pub state: SessionState,
    pub remote_endpoint: Option<String>,
    pub local_endpoint: Option<String>,
    pub started_at: i64,
    pub last_seen_at: i64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub owner_module: String,
}
```

每个 session 注册关闭回调或 runtime-neutral close handle。SessionId 必须全局唯一，不能继续使用固定 `publisher` 或每流重复的 `player-N`。

注册来源包括协议 publisher/player、domain publisher/subscriber、proxy、RTP sender/receiver 和 VOD player。module stop 必须注销其全部 session。

## 3. 精确 session 控制

- `list_sessions` 直接查询 directory，并按 vhost/app/stream/kind/state/protocol 分页。
- `kick_session` 解析真实 close handle，触发有界关闭并返回终态。
- `kick_stream` 获取该 MediaKey 的 session 快照，关闭发布者和相关 player，返回真实 `closed_sessions`。
- 并发关闭必须幂等；已关闭 session 返回 NotFound 或最终状态，规则在 native/ZLM adapter 中固定。
- close reason 写入 session event 和审计日志。

## 4. Publish/Subscribe control port

`PublishSubscribeApi` 保留可序列化控制句柄：

- `acquire_publisher` 调用现有 publisher lease，第二发布者返回 Conflict。
- `open_subscriber` 创建 engine subscriber，返回 session、MediaKey、schema 和可选 URL。
- `close_handle` 通过 session directory 关闭对应 lease/subscriber。
- 所有 queue capacity、bootstrap、slow-subscriber 策略使用有界配置。

## 5. Rust 数据面

在 `cheetah-sdk` 增加非序列化 `MediaDataPlaneApi`，不放进 HTTP DTO：

```rust
pub trait MediaDataPlaneApi: Send + Sync {
    async fn open_frame_publisher(
        &self,
        ctx: &MediaRequestContext,
        request: PublishRequest,
    ) -> Result<Box<dyn MediaFramePublisher>>;

    async fn open_frame_subscriber(
        &self,
        ctx: &MediaRequestContext,
        request: SubscribeRequest,
    ) -> Result<Box<dyn MediaFrameSubscriber>>;
}
```

`MediaFramePublisher` 接受 `TrackInfo` 和 `Arc<AVFrame>`；`MediaFrameSubscriber` 提供 runtime-neutral 异步 receive、track updates、lag/closed 状态和 cancel。公共接口不能暴露 Tokio channel。

HomeKit 等同进程集成可以订阅 frame 后在自己的 adapter 中完成 packetization/SRTP；外部进程应使用 RTP/WHIP/RTSP 等媒体协议。

## 6. StreamInfo 质量

必须替换当前占位值：

- `created_at` 来自 publisher/session 创建时间，不是查询时刻。
- `last_activity_at` 来自最近帧或 keepalive。
- bytes/duration/readers/publishers 来自真实计数器。
- tracks 来自 engine `TrackInfo`。
- URLs 由 `MediaUrlResolverApi` 生成。
- schema 过滤基于实际启用输出 capability；不能静默忽略 query.schema。

## 7. 任务与测试

| ID | 任务 | 关键测试 |
| --- | --- | --- |
| S2-T1 | session directory + ID | 多流/多协议 ID 唯一 |
| S2-T2 | 协议 module 注册 session | publish/play/close 生命周期 |
| S2-T3 | kick_session/kick_stream | 精确关闭、幂等、reason |
| S2-T4 | publisher lease bridge | 第二发布者 Conflict |
| S2-T5 | subscriber bridge | bootstrap、帧接收、取消 |
| S2-T6 | Rust data plane | AVFrame/TrackInfo 保真 |
| S2-T7 | StreamInfo 指标 | 时间、bytes、URL 非占位 |

验收命令：

```bash
cargo test -p cheetah-engine stream
cargo test -p cheetah-sdk media_data_plane
cargo test -p cheetah-media-api
```

