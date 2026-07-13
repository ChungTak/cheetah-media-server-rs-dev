# 04. Native HTTP Adapter

## 1. 定位

native HTTP 是 Cheetah 自有 API，不追求复制任何第三方 JSON。它必须稳定、可版本化、可发现，并能表达异步任务和明确错误。

建议新增 `cheetah-media-http-native` module 或放在 control 的独立路由 module 中；Axum 只在该 adapter 内部使用。

## 2. 通用请求与响应

请求头：

- `Authorization: Bearer ...` 或部署定义的认证方式。
- `X-Request-Id` 可选；服务端没有时生成并在响应中返回。
- `Idempotency-Key` 用于创建任务/代理/RTP session。

成功响应统一为资源或任务对象；异步操作返回 `202 Accepted` 和 task/session id。错误使用 HTTP 状态码与稳定 JSON：

```json
{
  "error": {
    "code": "media.not_found",
    "message": "media stream is not online",
    "retryable": false,
    "request_id": "...",
    "details": {}
  }
}
```

分页统一使用 `items`、`page`、`page_size`、`total`、`next_cursor`；时间使用 RFC 3339 或显式的 Unix milliseconds，不能同一资源混用。

## 3. 路由目录

### 3.1 媒体与 session

- `GET /api/v1/media`：媒体列表。
- `GET /api/v1/media/{vhost}/{app}/{stream}`：媒体详情。
- `GET /api/v1/media/{vhost}/{app}/{stream}/online`：在线状态。
- `POST /api/v1/media/{vhost}/{app}/{stream}/close`：关闭该流相关会话。
- `GET /api/v1/sessions`：session 列表。
- `POST /api/v1/sessions/{session_id}/kick`：踢出单一 session。
- `POST /api/v1/media/{vhost}/{app}/{stream}/keyframe`：请求关键帧。

### 3.2 录制和文件

- `POST /api/v1/record/tasks`：启动录制。
- `POST /api/v1/record/tasks/{task_id}/stop`：停止录制。
- `GET /api/v1/record/tasks`：任务列表。
- `GET /api/v1/record/files`：文件查询。
- `DELETE /api/v1/record/files/{file_id}`：删除文件。
- `POST /api/v1/record/playback/{file_id}/control`：pause/resume/scale/seek。

### 3.3 快照

- `POST /api/v1/snapshots`：抓图。
- `GET /api/v1/snapshots`：抓图索引查询。
- `DELETE /api/v1/snapshots/directories`：清理快照目录。
- `GET /api/v1/files/{file_id}/download`：受权限保护的文件下载。

### 3.4 代理与 RTP

- `POST /api/v1/proxies/pull`、`GET /api/v1/proxies/pull`、`DELETE /api/v1/proxies/pull/{id}`。
- `POST /api/v1/proxies/push`、`GET /api/v1/proxies/push`、`DELETE /api/v1/proxies/push/{id}`。
- `POST /api/v1/proxies/ffmpeg`、`DELETE /api/v1/proxies/{id}`。
- `POST /api/v1/rtp/receivers`、`POST /api/v1/rtp/receivers/{id}/connect`、`DELETE /api/v1/rtp/receivers/{id}`。
- `POST /api/v1/rtp/senders`、`GET /api/v1/rtp/sessions`、`DELETE /api/v1/rtp/sessions/{id}`。
- `PATCH /api/v1/rtp/sessions/{id}`：SSRC、检查、暂停等可变属性。

## 4. 权限和审计

每个 route 声明所需 capability 和 resource scope。读媒体、踢流、删文件、启动代理、打开 RTP、下载文件、播放控制分别授权；不可使用“拥有 API token 即全部权限”的隐式规则。高风险命令写入审计事件，审计字段不得包含密码、secret 或完整 token。

## 5. 兼容与版本

native API 只在 `/api/v1` 中发布破坏性变更；新增字段默认向后兼容；枚举未知值不得导致旧客户端崩溃。实现不得把 native route 作为 ZLM route 的内部别名，二者都应直接调用 domain port。

