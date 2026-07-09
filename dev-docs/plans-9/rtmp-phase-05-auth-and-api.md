# RTMP Phase 05 — 鉴权钩子与管理 API

- 状态：已完成
- 范围：实现 publish/play 鉴权钩子、REST 管理 API、统计监控
- 完成标准：鉴权钩子可拦截推拉流请求，REST API 可管理 Pull/Push/Relay 任务，统计数据可查询

## 目标

1. 实现基于 HTTP 回调的鉴权钩子框架。
2. 为 RTMP 模块提供 REST 管理 API。
3. 暴露连接和流的统计信息。

## 设计约束

- 鉴权钩子通过 `cheetah-sdk` 的 HTTP 模块契约实现，不在 RTMP module 中直接绑定 HTTP 框架。
- REST API 通过 control 服务暴露，RTMP module 只注册路由处理器。
- 统计信息通过 `EngineContext` 上报，不在 module 中维护全局状态。

## 任务分解

### 5.1 鉴权钩子框架

**目标**：建立通用的 RTMP 事件钩子机制，首先实现 publish/play 鉴权。

**参考**：simple-media-server 的 `HookManager`，支持 publish/play auth、player tracking、stream heartbeat。

**设计**：

```rust
/// 钩子事件类型
pub enum RtmpHookEvent {
    OnPublish(PublishHookPayload),
    OnPlay(PlayHookPayload),
    OnUnpublish(UnpublishHookPayload),
    OnStop(StopHookPayload),
}

/// Publish 鉴权请求
pub struct PublishHookPayload {
    pub app: String,
    pub stream: String,
    pub client_ip: IpAddr,
    pub tc_url: String,
    pub params: HashMap<String, String>,  // URL query params (token etc.)
}

/// 鉴权响应
pub enum HookResponse {
    Allow,
    Deny { code: String, description: String },
}

/// 钩子配置
pub struct RtmpAuthConfig {
    pub enabled: bool,
    pub hook_url: String,           // HTTP POST 回调地址
    pub timeout_ms: u64,            // 回调超时，默认 3000
    pub on_timeout: TimeoutPolicy,  // 超时策略：Allow / Deny
}

pub enum TimeoutPolicy {
    Allow,  // 超时放行（可用性优先）
    Deny,   // 超时拒绝（安全优先）
}
```

**工作流程**：
1. 客户端发送 publish/play 命令。
2. Core 状态机生成 `Output::PublishRequest` / `Output::PlayRequest`。
3. Module 层拦截请求，构建 `HookPayload`。
4. 通过 SDK HTTP 抽象发送 POST 请求到 `hook_url`。
5. 等待响应（带超时）。
6. 根据响应决定 accept 或 reject。
7. 将决策回传给 core 状态机。

**HTTP 回调格式**：
```json
// POST /api/rtmp/auth
// Request:
{
  "event": "on_publish",
  "app": "live",
  "stream": "test",
  "client_ip": "192.168.1.100",
  "tc_url": "rtmp://localhost:1935/live",
  "params": {"token": "abc123"}
}

// Response (200 = allow):
{"code": 0, "message": "ok"}

// Response (403 = deny):
{"code": 403, "message": "unauthorized"}
```

**module 层实现**：
- 鉴权逻辑在 module 层，不在 core 或 driver 中。
- 使用 SDK 提供的 HTTP client 抽象（不直接依赖 reqwest）。
- 鉴权结果缓存（可选）：同一 token 在 TTL 内不重复回调。

### 5.2 Publish/Play 鉴权

**目标**：将鉴权钩子集成到现有的 publish/play 流程中。

**Publish 鉴权流程**：
```
Client                    Module                    Hook Server
  │                         │                          │
  │── publish(app/stream) ──▶│                          │
  │                         │── POST on_publish ───────▶│
  │                         │◀── 200 {code:0} ─────────│
  │◀── onStatus(Start) ────│                          │
  │                         │                          │
  │── publish(app/stream) ──▶│                          │
  │                         │── POST on_publish ───────▶│
  │                         │◀── 403 {code:403} ───────│
  │◀── onStatus(Failed) ───│                          │
```

**Play 鉴权流程**：同上，事件类型为 `on_play`。

**URL 参数提取**：
- RTMP URL 中的 query string 作为鉴权参数传递。
- 例如：`rtmp://host/live/test?token=abc&user=admin`
- 提取 `params: {"token": "abc", "user": "admin"}`。

**cheetah-rtmp-core 改动**：
- URL 解析增加 query string 提取（已有 `RtmpUrl`，需确认 query 支持）。
- 无其他 core 改动（鉴权决策在 module 层）。

**cheetah-rtmp-module 改动**：
- Ingest 流程中 publish 请求增加鉴权拦截点。
- Egress 流程中 play 请求增加鉴权拦截点。
- 鉴权失败时发送 `NetStream.Publish.Denied` / `NetStream.Play.Failed`。

### 5.3 REST 管理 API

**目标**：通过 control 服务暴露 RTMP 模块的管理接口。

**参考**：simple-media-server 的 `RtmpApi`。

**API 设计**：

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/v1/rtmp/connections` | 列出所有活跃连接 |
| GET | `/api/v1/rtmp/connections/:id` | 查询单个连接详情 |
| DELETE | `/api/v1/rtmp/connections/:id` | 强制断开连接 |
| GET | `/api/v1/rtmp/streams` | 列出所有活跃流 |
| GET | `/api/v1/rtmp/streams/:key` | 查询单个流详情 |
| POST | `/api/v1/rtmp/pull/start` | 启动 Pull 任务 |
| POST | `/api/v1/rtmp/pull/stop` | 停止 Pull 任务 |
| GET | `/api/v1/rtmp/pull/list` | 列出 Pull 任务 |
| POST | `/api/v1/rtmp/push/start` | 启动 Push 任务 |
| POST | `/api/v1/rtmp/push/stop` | 停止 Push 任务 |
| GET | `/api/v1/rtmp/push/list` | 列出 Push 任务 |
| POST | `/api/v1/rtmp/relay/start` | 启动 Relay 任务 |
| POST | `/api/v1/rtmp/relay/stop` | 停止 Relay 任务 |
| GET | `/api/v1/rtmp/relay/list` | 列出 Relay 任务 |

**响应格式**：
```json
// GET /api/v1/rtmp/connections
{
  "connections": [
    {
      "id": "conn-001",
      "client_ip": "192.168.1.100:54321",
      "state": "publishing",
      "stream_key": "live/test",
      "connected_at": "2026-05-13T08:00:00Z",
      "bytes_in": 1048576,
      "bytes_out": 0,
      "codec_video": "H.264",
      "codec_audio": "AAC"
    }
  ]
}
```

```json
// POST /api/v1/rtmp/pull/start
// Request:
{
  "name": "dynamic_pull_1",
  "source_url": "rtmp://origin.example.com/live/main",
  "target_stream_key": "live/main"
}
// Response:
{"job_id": "pull-001", "status": "started"}
```

**module 层实现**：
- 通过 `cheetah-sdk` 的 HTTP module 契约注册路由。
- 路由处理器访问 module 内部状态（连接列表、任务列表）。
- 动态启动的 Pull/Push/Relay 任务与配置文件中的任务共用同一管理机制。

### 5.4 统计与监控

**目标**：暴露 RTMP 模块的运行时统计信息。

**统计指标**：

| 指标 | 类型 | 说明 |
|------|------|------|
| `rtmp_connections_active` | Gauge | 当前活跃连接数 |
| `rtmp_connections_total` | Counter | 累计连接数 |
| `rtmp_publishers_active` | Gauge | 当前发布者数 |
| `rtmp_subscribers_active` | Gauge | 当前订阅者数 |
| `rtmp_bytes_in_total` | Counter | 累计接收字节 |
| `rtmp_bytes_out_total` | Counter | 累计发送字节 |
| `rtmp_handshake_failures` | Counter | 握手失败次数 |
| `rtmp_auth_denials` | Counter | 鉴权拒绝次数 |
| `rtmp_pull_jobs_active` | Gauge | 活跃 Pull 任务数 |
| `rtmp_push_jobs_active` | Gauge | 活跃 Push 任务数 |
| `rtmp_relay_jobs_active` | Gauge | 活跃 Relay 任务数 |

**实现方式**：
- 统计数据通过 `EngineContext` 的 metrics 接口上报。
- 不在 RTMP module 中引入 Prometheus 依赖（由 control 层统一导出）。
- 每个连接维护自己的计数器，汇总在 module 层。

**API 暴露**：
```
GET /api/v1/rtmp/stats
```

```json
{
  "connections_active": 5,
  "connections_total": 1234,
  "publishers_active": 2,
  "subscribers_active": 3,
  "bytes_in_total": 10485760,
  "bytes_out_total": 52428800,
  "uptime_seconds": 3600
}
```

## 配置示例

```yaml
modules:
  rtmp:
    enabled: true
    listen: 0.0.0.0:1935
    auth:
      enabled: true
      hook_url: http://localhost:8080/api/rtmp/auth
      timeout_ms: 3000
      on_timeout: allow  # allow | deny
      cache_ttl_ms: 60000  # 鉴权结果缓存时间，0=不缓存
```

## 测试计划

1. **鉴权测试**：
   - Hook 返回 allow → publish/play 成功。
   - Hook 返回 deny → publish/play 被拒绝，客户端收到正确错误码。
   - Hook 超时 → 根据 `on_timeout` 策略处理。
   - Hook 不可达 → 根据 `on_timeout` 策略处理。
   - URL 参数正确传递到 hook payload。

2. **API 测试**：
   - 连接列表查询。
   - 动态启动/停止 Pull/Push/Relay 任务。
   - 强制断开连接。
   - 并发 API 请求不导致死锁。

3. **统计测试**：
   - 连接建立/断开时计数器正确更新。
   - 字节计数准确。
   - 多连接并发时统计无竞争。
