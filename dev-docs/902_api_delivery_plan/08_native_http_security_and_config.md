# 08 · Native HTTP、安全与配置

## 1. Native route 完整集

在 `/api/v1` 下完成：

- media：list/detail/online/close/keyframe/urls。
- sessions：list/detail/kick。
- record：task start/stop/list、file list/delete/download、playback control。
- snapshots：create/list/delete/download。
- proxies：pull/push/ffmpeg create/list/detail/delete。
- RTP：receiver create/connect、sender create、session list/detail/update/stop。
- capabilities：当前 provider、版本、状态和 operation。

动态资源 path 必须通过 route template 匹配，不能让空 route list 无条件接管全部 `/api/v1` fallback。未知 path 返回 404，已知 path 错方法返回 405。

## 2. 认证与授权

新增框架无关 `ControlAuthApi`，HTTP adapter 把 Authorization、mTLS identity 或部署 token 转换为 principal。默认拒绝匿名高风险操作。

权限 scope：

| Scope | 操作 |
| --- | --- |
| `media.read` | 媒体/session/capability 查询 |
| `media.control` | 关键帧、关闭流、踢 session |
| `media.publish` | 创建 publisher/pull proxy/RTP receiver |
| `media.consume` | 获取 URL、subscriber、RTP sender |
| `record.manage` | record start/stop/playback |
| `file.read` | snapshot/record 下载 |
| `file.delete` | 删除文件/目录 |
| `server.admin` | 配置、restart、危险兼容 API |

resource scope 可限制 vhost/app/stream。鉴权失败不泄露资源是否存在。

## 3. Request context

- 没有 `X-Request-Id` 时生成唯一 ID并回写。
- 解析 `X-Correlation-Id`、trace context。
- `Idempotency-Key` 只用于创建任务/session/proxy。
- deadline 由 route 默认值和客户端可接受上限共同决定。
- principal 和 source adapter 传入所有 domain port。

## 4. 审计

踢流、删文件、启动/停止 record、创建 proxy/RTP、修改配置、restart 必须产生审计记录：principal、operation、resource、result、request ID、时间和安全摘要。不得记录 token、secret、密码、完整 SDP 或包含凭据的 URL。

## 5. Adapter 配置

配置模型：

```text
media.native.enabled
media.native.path_prefix             # 默认 /api/v1
media.native.auth.mode
media.native.request_timeout_ms
media.native.max_body_bytes
media.zlm.enabled
media.zlm.path_prefix                # 默认 /index
media.zlm.auth.mode
media.zlm.secret
media.zlm.legacy_http_200
media.zlm.strict_fields
media.webhooks.targets[]
```

factory 在 server 读取启动配置后构造 manifest prefix。prefix 或 enabled 的在线变化返回 EngineRestartRequired；timeout/strict/retry 等安全可热更新字段返回 Immediate。两个 adapter 独立启停。

## 6. Feature 与交付 profile

默认 Cargo feature 只有 RTMP 时，不得声称 record/RTP/snapshot/proxy 可用。提供明确 server profile，例如 `media-control-full`，组合正式交付所需协议和 provider；CI 对默认 profile 与 full profile 分别检查 capability。

route 可以在 provider 未安装时返回 503/Unavailable 或 501/Unsupported，但 capability 响应必须解释原因。不能因为 adapter 路由存在就标记 feature 可用。

## 7. 错误响应

native 使用 HTTP 状态：400 参数、401 未认证、403 无权限、404 不存在、409 冲突、429/503 busy、504 timeout、501 unsupported、500 internal。所有错误包含稳定 domain code、message、retryable、request ID 和经过过滤的 details。

## 8. 测试

- 每条 route 覆盖 success/400/401/403/404/409/501/503/504。
- 同一资源不同 principal 做授权矩阵。
- prefix、enabled 和 restart semantics。
- body 上限、重复 header、非法 percent encoding。
- request ID 生成、透传和日志关联。
- native 与 ZLM 同时启用、分别关闭、prefix 不冲突。

```bash
cargo test -p cheetah-media-module native
cargo test -p cheetah-control module_http
cargo check -p cheetah-server --features media-control-full
```

