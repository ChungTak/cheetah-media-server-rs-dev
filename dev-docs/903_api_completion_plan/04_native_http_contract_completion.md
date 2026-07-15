# 04 · Native HTTP 契约完善

## 1. 统一 wire 规则

成功响应使用 `{ "request_id": "...", "data": ... }`，分页 data 包含 `items/next_cursor/total`。错误使用 `{ "request_id", "error": { "code", "message", "details" } }`；message 不泄漏路径、凭据和下游响应正文。

HTTP 映射：InvalidArgument=400、Unauthenticated=401、PermissionDenied=403、NotFound=404、Conflict=409、ResourceExhausted=429、Unsupported=501、Unavailable=503、DeadlineExceeded=504、Internal=500。创建返回 201，删除幂等返回 204。

请求头：`Authorization`、`Idempotency-Key`、`X-Request-Deadline-Ms`、`X-Correlation-Id`。deadline 是绝对 epoch 毫秒，非法或已过期请求在调用 provider 前失败。

## 2. RTP 路由

| Method | Path | Scope | 行为 |
| --- | --- | --- | --- |
| POST | `/api/v1/rtp/receivers` | media.publish | 创建 passive receiver |
| POST | `/api/v1/rtp/receivers/{id}/connect` | media.publish | 建立 TCP active receiver |
| POST | `/api/v1/rtp/senders` | media.consume | 创建 sender/talk |
| GET | `/api/v1/rtp/sessions` | media.read | 过滤和分页 |
| GET | `/api/v1/rtp/sessions/{id}` | media.read | 单会话 |
| PATCH | `/api/v1/rtp/sessions/{id}` | media.control | expected_generation + patch |
| DELETE | `/api/v1/rtp/sessions/{id}` | media.control | 停止并回收资源 |

旧 `POST .../{id}/stop` 保留一个小版本，仅委托 DELETE 语义并返回 deprecation header。路由先按原始 segment 匹配再百分号解码，解码后执行 ID 校验。

## 3. 新增路由

Playback：`POST/GET /api/v1/playback/sessions`、`GET/PATCH/DELETE /api/v1/playback/sessions/{id}`。旧 record playback control 路由仅作 shim。

Webhook：`POST/GET /api/v1/webhooks`、`GET/PATCH/DELETE /api/v1/webhooks/{id}`、`POST /api/v1/webhooks/{id}/test`。secret 创建后只返回一次，后续仅返回 `secret_configured`。

能力详情和 URL 路由按 [03](03_capability_and_url_honesty.md)；快照批量删除响应按 [06](06_snapshot_image_and_file_lifecycle.md)。

## 4. Adapter 实施规则

固定顺序：解析长度限制 → request id → credentials → authentication → DTO 校验 → 资源授权 → deadline → idempotency → provider。审计在成功与失败路径都记录，但不得记录 token、Webhook secret 或完整外部 URL 凭据。

adapter 不轮询内部字段推测成功；异步创建返回当前资源，客户端通过 GET 观察状态。所有 provider 缺失均稳定映射 503，不 panic。

## 5. 验收

- `HTTP-01`：补齐 RTP 标准路由和旧路由兼容测试。
- `HTTP-02`：补齐 playback/Webhook/capability details 路由。
- `HTTP-03`：以独立客户端进程验证认证、编码 ID、分页、错误和重启。
- OpenAPI 或等价 schema fixture 必须覆盖所有 DTO，golden 测试阻止字段漂移。

