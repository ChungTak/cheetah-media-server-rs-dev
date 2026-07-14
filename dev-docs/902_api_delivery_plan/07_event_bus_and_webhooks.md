# 07 · Media Event Bus 与出站 Webhook

## 1. 方向定义

- native/ZLM `/api/*` 是调用 Cheetah 的入站控制 API。
- `on_publish`、`on_play`、`on_record_mp4` 等是 Cheetah 向配置目标发送的出站 webhook。
- 内部 `MediaEvent` 是 typed domain event，不等于任何 webhook JSON。
- 本项目不需要挂载 `/index/hook/on_publish` 一类入站 route，除非未来明确提供 webhook relay capability。

## 2. Media Event Bus

在 engine 增加有界 `MediaEventBusApi`，与现有 SystemEvent bus 并存或通过明确 bridge 集成。接口必须支持：

- publish typed MediaEvent。
- subscribe 返回可取消 `MediaEventSubscription`。
- 每个 subscriber 独立有界队列。
- lag 通知和 dropped count。
- 按资源维持 sequence；跨资源不承诺全局顺序。
- module stop/restart 自动取消其订阅。

修正 `MediaFacade::subscribe_events`：不能丢弃 sender 后返回成功。建议改为返回 subscription ID/handle，并增加 unsubscribe；若保持现有签名，则 registry 必须持有 sender 到显式关闭。

## 3. 事件发布点

| 事件 | 发布时机 | 生产者 |
| --- | --- | --- |
| StreamPublished | publisher lease 建立并具备有效 track | engine/协议 module |
| StreamUnpublished | publisher 关闭 | engine |
| StreamOnlineChanged | online 状态真正变化 | engine |
| SessionOpened/Closed | directory 注册/注销 | session directory |
| RecordStarted/Progress/Completed | task/segment/file 状态变化 | record module |
| SnapshotCompleted | 文件已安全注册 | snapshot module |
| RtpSessionTimeout | timeout 判定完成 | RTP orchestrator |
| ProxyStateChanged | proxy 状态改变 | proxy module |
| ServerLifecycle | start/keepalive/stop | application/engine |

禁止 adapter 自己推测并重复发布业务事件。

## 4. Webhook Dispatcher

新增独立 dispatcher，消费 MediaEvent 后按 profile 翻译。每个目标配置：URL、event allowlist、secret/signature、timeout、最大 body、retry policy、concurrency、fail-open/fail-close decision policy。

可靠性：

- 媒体热路径只做有界 enqueue，不等待 HTTP。
- 每个目标独立队列和熔断，慢目标不拖累其他目标。
- 仅对幂等通知有限重试；鉴权决策类回调不做跨 deadline 的后台重试。
- event ID 写入请求，接收方可去重。
- 默认拒绝 loopback、link-local、metadata service 和未允许内网目标，防止 SSRF。

## 5. ZLM Hook 映射

必须实现并测试：

`on_publish`、`on_play`、`on_flow_report`、`on_rtsp_realm`、`on_rtsp_auth`、`on_stream_changed`、`on_stream_not_found`、`on_record_mp4`、`on_record_ts`、`on_shell_login`、`on_stream_none_reader`、`on_http_access`、`on_server_started`、`on_server_exited`、`on_server_keepalive`、`on_send_rtp_stopped`、`on_rtp_server_timeout`。

其中：

- publish/play/auth/access 属于同步决策 hook，必须有短 deadline 和配置的失败策略。
- record/server/flow/timeout 属于异步通知 hook。
- RTSP realm/auth 由 RTSP auth provider 生成，不把密码写入 event bus。
- none-reader/not-found 的 close 决策只能影响请求对应资源。

字段、时间单位、布尔格式和响应结构写入 golden fixtures，不从内部 struct 自动序列化冒充兼容格式。

## 6. Native 事件接口

本轮至少提供配置型 webhook；可选提供受权 SSE/stream endpoint，但不得用无界响应队列。Rust SDK 直接使用 event subscription handle。

## 7. 任务与验收

| ID | 任务 | DoD |
| --- | --- | --- |
| S6-T1 | 有界 event bus | lag、取消、顺序可测 |
| S6-T2 | 全生产发布点 | 事件不重复、不遗漏终态 |
| S6-T3 | dispatcher | timeout/retry/circuit breaker |
| S6-T4 | ZLM decision hooks | allow/deny/timeout 策略 |
| S6-T5 | ZLM notification hooks | 字段 golden |
| S6-T6 | SSRF/secret | 内网拒绝、日志脱敏 |

```bash
cargo test -p cheetah-engine media_event
cargo test -p cheetah-media-module webhook
cargo test -p cheetah-record-module event
cargo test -p cheetah-rtp-module event
```

