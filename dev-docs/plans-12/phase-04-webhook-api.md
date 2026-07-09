# Phase 04 — HTTP Webhook 事件 + 管理 API

- **状态**: 未开始
- **范围**: Webhook 事件框架、流管理 API、按需拉流
- **完成标准**: 核心事件可通过 HTTP POST 回调通知外部系统；可通过 API 动态管理拉流/推流任务

---

## 4.1 Webhook 事件框架

### 事件类型（按优先级）

| 事件 | 触发时机 | 用途 |
|------|----------|------|
| `on_publish` | 流发布成功 | 鉴权/计费 |
| `on_play` | 客户端开始播放 | 鉴权/计费 |
| `on_stream_not_found` | 请求的流不存在 | 按需拉流 |
| `on_stream_none_reader` | 流无人观看超时 | 资源回收 |
| `on_stream_disconnect` | 推流断开 | 告警 |
| `on_record_segment` | 录制分片完成 | 存储管理 |
| `on_server_started` | 服务启动 | 服务发现 |
| `on_server_keepalive` | 定期心跳 | 健康检查 |

### 实现设计

- 通过 `cheetah-sdk` 的 `EventBus` 发布事件
- 新增 `cheetah-webhook-module` 订阅事件并 HTTP POST 到配置的 URL
- 支持重试、超时、并发限制
- 支持 `secret` 签名验证

### 配置

```yaml
modules:
  webhook:
    enabled: true
    endpoints:
      - url: http://localhost:8080/webhook
        secret: my_secret
        events: [on_publish, on_play, on_stream_not_found]
        timeout_ms: 3000
        max_retries: 3
```

---

## 4.2 流管理 API

### 端点

| 方法 | 路径 | 功能 |
|------|------|------|
| POST | `/api/proxy/pull/add` | 添加拉流任务 |
| POST | `/api/proxy/pull/del` | 删除拉流任务 |
| POST | `/api/proxy/push/add` | 添加推流任务 |
| POST | `/api/proxy/push/del` | 删除推流任务 |
| GET | `/api/streams` | 列出所有活跃流 |
| POST | `/api/streams/close` | 强制关闭流 |
| GET | `/api/server/config` | 获取当前配置 |

### 实现位置

通过 `cheetah-control` 的 HTTP 路由注册，各模块通过 `ModuleHttpService` trait 暴露 API。

---

## 4.3 按需拉流（on_stream_not_found）

### 流程

1. 播放器请求不存在的流
2. 模块发送 `on_stream_not_found` Webhook
3. 外部系统调用 `/api/proxy/pull/add` 创建拉流任务
4. 拉流建立后，播放器自动获得数据（通过 `play_wait_source_timeout_ms` 等待）

### 替代方案（内置）

配置静态映射规则，当流不存在时自动从配置的源拉取：

```yaml
modules:
  rtsp:
    on_demand_pull:
      - pattern: "camera/*"
        source_template: "rtsp://nvr.local/{stream}"
```
