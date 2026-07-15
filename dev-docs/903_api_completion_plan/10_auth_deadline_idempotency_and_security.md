# 10 · 鉴权、deadline、幂等与安全

## 1. 资源级授权

扩展 `Principal`：

```rust
struct MediaResourceGrant {
    selector: MediaResourceSelector,
    scopes: Vec<MediaScope>,
}

struct MediaResourceSelector {
    vhost: Pattern,
    app: Pattern,
    stream: Pattern,
}
```

`resource_grants` 使用 serde default；ServerAdmin 仍可全局访问。Pattern 首期只支持 exact 和 `*` 整段通配，不接受正则，避免不同 adapter 解释不一致。`authorizes(scope, key)` 同时检查全局 scope 与资源 grant；无 key 的 server 操作只能使用全局 scope。

list 查询在 provider/目录层做授权过滤后再计算 total 和 cursor，不能先分页再过滤。跨租户 get/delete 统一返回 NotFound；审计仍记录真实 PermissionDenied。

## 2. Deadline

提供统一 helper 计算 remaining duration、检查过期和创建 cancellation child。adapter 调 provider 前检查一次，provider 在分配租约/端口/临时文件/进程前再次检查，driver/connect/encode/wait 使用剩余时间。

DeadlineExceeded 的完成条件包括回滚本次副作用；不能仅停止等待。已创建的长期资源在响应 deadline 到达时按幂等状态判定：若创建已原子提交则可由同 key 重取，否则必须清理。

## 3. 幂等

记录键为 principal identity + operation + idempotency key，值包含 canonical request SHA-256、resource id、terminal creation result、expiry。规则：

- 同键同指纹且已成功：返回原资源，不重复执行。
- 同键同指纹且进行中：等待同一结果或返回带 resource id 的 Conflict。
- 同键不同指纹：Conflict。
- 失败且无副作用：允许按配置重试；部分提交必须先恢复或返回原状态。

覆盖 RTP receiver/sender、pull/push/FFmpeg proxy、snapshot、record start、playback open。敏感字段先规范化再 hash，但不写入日志。

## 4. 其他安全约束

- mTLS identity header 只在请求来自配置的可信反向代理且连接认证成功时接受。
- HMAC key 带 key id，支持当前键签名和上一键限时验签；日志不输出签名或 secret。
- 外部 URL 按 [08](08_proxy_connector_and_ffmpeg_execution.md) 进行 SSRF 校验。
- 文件只通过 owner-bound handle 访问，下载和删除分别要求 FileRead/FileDelete。
- 请求体、分页、Webhook 响应、stderr、事件队列、媒体队列均有上限。

## 5. 任务与验收

`SEC-01` 资源 grants；`SEC-02` deadline 贯穿；`SEC-03` 幂等 repository；`SEC-04` mTLS/HMAC；`SEC-05` 安全审计。

必须有跨 vhost/app 正反测试、list 不泄漏 total、同键不同请求冲突、超时无孤儿资源、恶意代理头、签名轮换、路径逃逸及 DNS 重绑定测试。

