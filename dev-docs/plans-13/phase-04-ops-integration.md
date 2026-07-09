# Phase 04 — 运维集成

- **状态**: 未开始
- **范围**: Webhook 事件系统、推流自动录制、无人观看超时关流
- **完成标准**: 核心事件可通过 HTTP POST 回调；推流可自动触发录制；无人观看流自动释放

---

## 4.1 Webhook 事件框架

**ABLMediaServer 事件列表**:
- `on_publish` — 推流开始
- `on_play` — 播放开始
- `on_play_disconnect` — 播放断开
- `on_stream_disconnect` — 推流断开
- `on_stream_arrive` — 流到达（首帧）
- `on_stream_not_arrive` — 拉流失败
- `on_stream_none_reader` — 无人观看超时
- `on_stream_not_found` — 请求的流不存在

**本地实现方案**:

新增 `cheetah-webhook-module`：
- 订阅引擎 `EventBus` 事件
- 按配置的 URL 列表发送 HTTP POST
- 支持 `secret` HMAC 签名
- 支持重试（指数退避）
- 支持事件过滤（只订阅感兴趣的事件类型）

**配置**:
```yaml
modules:
  webhook:
    enabled: true
    endpoints:
      - url: http://localhost:8080/hook
        secret: my_secret
        events: [on_publish, on_play, on_stream_none_reader]
        timeout_ms: 3000
        max_retries: 3
```

---

## 4.2 推流自动录制

**ABLMediaServer 方案**: `pushEnable_mp4=1` 时，RTMP 推流自动触发 MP4 录制。

**本地实现方案**:

在 RTMP module 的 publish 接受逻辑中，检查配置并通知录制模块：

```yaml
modules:
  rtmp:
    auto_record:
      enabled: false
      format: flv        # flv | mp4 | ts
      max_duration_secs: 3600
      stream_pattern: "*"  # 匹配哪些流自动录制
```

**实现位置**: `cheetah-rtmp-module` publish 接受后，通过 EventBus 发送 `StreamPublished` 事件，录制模块监听并自动启动录制。

---

## 4.3 无人观看超时关流

**ABLMediaServer 方案**: `maxTimeNoOneWatch` 配置，超时后关闭流。

**本地实现方案**:

在引擎层跟踪订阅者数量：
- 订阅者数降为 0 时启动计时器
- 超时后发送 `on_stream_none_reader` 事件
- 若配置了自动关流，释放发布者租约

```yaml
global:
  stream_lifecycle:
    no_reader_timeout_ms: 30000  # 0 = 禁用
    no_reader_action: close      # close | webhook_only
```

**实现位置**: `cheetah-engine` 流管理层。
