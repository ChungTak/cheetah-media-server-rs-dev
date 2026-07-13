# 06. 内部事件总线与兼容 webhook

## 1. 内部事件

事件属于 domain，不是 HTTP webhook JSON。建议定义：

```rust
pub enum MediaEvent {
    StreamPublished(StreamPublished),
    StreamUnpublished(StreamUnpublished),
    StreamOnlineChanged(StreamOnlineChanged),
    SessionOpened(SessionOpened),
    SessionClosed(SessionClosed),
    RecordStarted(RecordStarted),
    RecordProgress(RecordProgress),
    RecordCompleted(RecordCompleted),
    SnapshotCompleted(SnapshotCompleted),
    RtpSessionTimeout(RtpSessionTimeout),
    ProxyStateChanged(ProxyStateChanged),
    ServerLifecycle(ServerLifecycle),
}
```

每个事件包含 `event_id`、`occurred_at`、`sequence`（按资源可选）、`media_key`（适用时）、`source`、`correlation_id` 和 typed payload。事件不得携带密码、secret、完整鉴权头或任意协议 request body。

## 2. 事件投递

- 内部 event bus 使用有界订阅队列，支持至少一次投递语义和显式 lag。
- 同一媒体的发布/结束、录制开始/完成、RTP timeout 等事件在单资源范围保持顺序。
- 事件消费者必须幂等；`event_id` 可用于去重。
- bus 只负责内存/进程内分发；持久化、重放和跨节点投递属于后续 capability。
- webhook adapter 不应阻塞媒体热路径；使用独立 dispatcher、deadline、有限重试和熔断。

## 3. ZLM webhook 映射

兼容 adapter 订阅内部事件并按旧名称、字段和响应规则生成回调：

| 内部事件 | 兼容 webhook | 必要字段 |
| --- | --- | --- |
| StreamPublished | `on_publish` | vhost/app/stream/schema/ip/port/id/originType |
| SessionOpened(player) | `on_play` | vhost/app/stream/schema/ip/port/id/originType |
| StreamOnlineChanged | `on_stream_changed` | schema/app/stream/regist |
| StreamUnpublished | `on_stream_changed` | 同上，regist=false |
| Stream not found during play | `on_stream_not_found` | media tuple, `close` decision |
| SessionClosed(last reader) | `on_stream_none_reader` | media tuple, `close` decision |
| RecordCompleted(mp4) | `on_record_mp4` | start_time/file_size/time_len/file_path/file_name/folder/url + media tuple |
| RecordProgress(ts) | `on_record_ts` | media tuple、file path、duration/size |
| RtpSessionTimeout | `on_rtp_server_timeout` | local_port/vhost/app/stream_id/tcp_mode/re_use_port/ssrc |
| ServerLifecycle | `on_server_started`/`on_server_exited`/`on_server_keepalive` | server id/version/status/metrics |

其余兼容名称必须有明确映射或显式 unsupported：`on_flow_report`、`on_rtsp_realm`、`on_rtsp_auth`、`on_shell_login`、`on_http_access`、`on_send_rtp_stopped`。其中 RTSP realm/auth 的 response 由 RTSP adapter/provider 生成，不能由通用媒体事件猜测。

## 4. 兼容 webhook 请求/响应

- `on_publish`、`on_play`：请求中包含 protocol、media tuple、远端地址、session id 和原始参数的受限白名单；响应支持允许/拒绝、code、msg、过期时间。
- `on_rtsp_realm`：响应 realm。
- `on_rtsp_auth`：响应 encrypted/passwd 等认证结果，密码只在必要边界短暂存在，不进入 event bus。
- `on_stream_not_found`、`on_stream_none_reader`：响应是否 close；超时使用配置的 fail-open/fail-close 策略。
- `on_http_access`：请求携带 ip/port/id/path/file_path/is_dir/params/headers 的过滤版，响应可覆盖 err/path/second。
- `on_record_mp4`：必须支持 record completion 的幂等回调；失败只影响 webhook 状态，不回滚已完成文件。

## 5. 安全

webhook URL、secret 和响应策略属于配置 secret，日志只记录 URL host、事件名、状态和 request id。外发 webhook 默认禁止内网 SSRF 目标，具体允许网段由部署配置决定。

