# 09 · 事件、Webhook 与准入

## 1. 两类语义严格分离

`MediaEvent` 是已发生事实，用于异步通知；`AdmissionRequest/Decision` 是副作用前的同步决策。禁止把“已发布/已播放”事件倒用为准入请求。

`MediaAdmissionApi::authorize(ctx, AdmissionRequest) -> Decision` 的 action 固定为 Publish、Play、CreatePullProxy、CreatePushProxy、OpenRtpReceiver、OpenRtpSender。请求包含 principal、资源键、协议、来源地址和必要参数摘要，不包含 token。Decision 为 Allow 或 Deny{stable_code, reason}。

统一接入点位于 engine/provider 的资源登记边界：发布在申请 publisher lease 前，播放在 subscriber 注册前，proxy/RTP 在创建 session 和分配端口前。每个逻辑操作只调用一次；adapter 不重复决策。deny 不得留下租约、端口、任务或幂等成功记录。

## 2. 失败策略

每类 action 配置 `FailClosed` 或 `FailOpen`，默认 publish/proxy/RTP 为 FailClosed，play 为 FailClosed；只有显式配置可放开。deadline 取请求剩余时间与 target timeout 的较小值。失败策略结果必须写审计和 metric，不能伪装成远端 Allow。

## 3. Webhook 管理

`WebhookAdminApi` 管理 profile：id、enabled、mode (`NativeDomain`/`ZlmCompatible`)、target URL、event filter、admission actions、failure policy、timeout、secret reference、generation。更新使用 expected generation；secret 不可读回。

test 操作发送独立 `WebhookTest` envelope，不伪造真实媒体事件；返回 DNS、connect、HTTP、签名验证和 latency 的安全摘要。管理 API 的持久化复用配置/数据库抽象，module restart 后恢复。

## 4. 出站投递

NativeDomain envelope 包含 event_id、event_type、occurred_at、resource、payload、attempt；使用 HMAC-SHA256 签名。兼容 translator 只映射其支持的 hook。无映射时记录 `unsupported_mapping_total{event_type,profile}` 和最后状态，不静默丢弃。

队列有界、按 profile 隔离；重试只针对网络错误、429 和 5xx，指数退避有次数和总时长上限。2xx 视为通知成功；准入响应另按 Decision schema 解析。关闭 subscription/profile 必须停止后续投递。

## 5. 任务与验收

- `EVT-01`：定义 Admission 类型和 MediaServices slot。
- `EVT-02`：接入四个资源登记边界并验证无残留副作用。
- `EVT-03`：WebhookAdmin provider、持久化和 native routes。
- `EVT-04`：两种 translator、签名、队列、重试和 metrics。
- `EVT-05`：真实 HTTP receiver 的通知与准入 E2E。

测试覆盖 allow、deny、timeout 两种策略、重复调用防护、队列满、关闭取消、签名篡改、未知映射，以及 StreamOnlineChanged、SnapshotCompleted、RecordCompleted 的真实到达。

