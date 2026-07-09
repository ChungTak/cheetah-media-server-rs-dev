# Phase 03 — 运维集成

- **状态**: 未开始
- **范围**: Webhook 事件系统、自动录制、无人观看关流、RTMP 302 重定向
- **完成标准**: 推流/拉流事件可通过 HTTP 回调通知；无人观看流自动释放资源

---

## 3.1 Webhook 事件框架

**问题**: 生产环境需要在推流/拉流/断开时通知业务系统进行鉴权、计费、录制触发等。

**ABLMediaServer 方案**: 
- `on_publish`/`on_play`/`on_disconnect` 等事件通过 HTTP POST 发送 JSON 到配置的 URL
- 事件通过 `pMessageNoticeFifo` 队列异步发送，由 `ABLMedisServerFastDeleteThread` 处理
- 支持 `hook_enable` 全局开关

**本地现状**: `EventBus` trait 已在 SDK 中定义，但无 HTTP webhook 实现。

**实现方案**:

```rust
// crates/system/cheetah-webhook/src/lib.rs — 新 crate
pub struct WebhookModule {
    client: reqwest::Client,
    config: WebhookConfig,
}

impl WebhookModule {
    /// 订阅 EventBus，将流事件转为 HTTP POST
    pub async fn start(&self, event_bus: Arc<dyn EventBus>) {
        let mut rx = event_bus.subscribe();
        while let Some(event) = rx.recv().await {
            match event.kind {
                StreamEventKind::PublishStarted => self.post("on_publish", &event).await,
                StreamEventKind::PlayStarted => self.post("on_play", &event).await,
                StreamEventKind::Disconnected => self.post("on_disconnect", &event).await,
                _ => {}
            }
        }
    }
}
```

**配置**:
```yaml
global:
  webhook:
    enabled: false
    on_publish: "http://localhost:8080/api/hook/on_publish"
    on_play: "http://localhost:8080/api/hook/on_play"
    on_disconnect: "http://localhost:8080/api/hook/on_disconnect"
    timeout_ms: 3000
```

---

## 3.2 推流自动录制

**问题**: 需要在推流开始时自动触发 MP4/FLV 录制，无需额外 API 调用。

**ABLMediaServer 方案**: `pushEnable_mp4` 配置项，推流时自动创建录制任务。

**本地现状**: 无录制能力。

**实现方案**:

录制作为独立模块，监听 `PublishStarted` 事件：

```rust
// 当收到 PublishStarted 事件且 auto_record 配置启用时
// 自动订阅该流并写入 MP4/FLV 文件
fn on_publish_started(stream_key: &StreamKey, config: &RecordConfig) {
    if config.auto_record {
        let subscriber = engine.subscribe(stream_key, options).await;
        spawn_record_task(subscriber, config.output_path, config.format);
    }
}
```

**配置**:
```yaml
global:
  record:
    auto_record: false
    format: mp4        # mp4 或 flv
    output_path: "/data/recordings/{app}/{stream}/{timestamp}.mp4"
    max_duration_secs: 3600
    max_file_size_mb: 2048
```

---

## 3.3 无人观看超时关流

**问题**: 推流源持续推流但无人观看时，浪费服务器资源（内存、CPU、带宽）。

**ABLMediaServer 方案**: `maxTimeNoOneWatch` 配置项，超过指定秒数无订阅者则断开推流源。

**本地现状**: 无此能力。流在发布者主动断开前一直存在。

**实现方案**:

在 Engine 的 StreamManager 中增加空闲检测：

```rust
// 周期性检查（每 10s）
for stream in streams.iter() {
    if stream.subscriber_count == 0 {
        let idle_duration = now - stream.last_subscriber_leave_time;
        if idle_duration > config.max_idle_timeout {
            stream.publisher_sink.close(); // 通知发布者断开
        }
    }
}
```

**配置**:
```yaml
global:
  stream:
    max_no_viewer_timeout_secs: 0  # 0 = 禁用，>0 = 超时秒数
```

---

## 3.4 RTMP 拉流 302 重定向支持

**问题**: 部分 CDN 和源站使用 HTTP 302 重定向机制，RTMP 客户端需要支持 `NetStream.Redirect` 命令。

**ABLMediaServer 方案**: 拉流客户端收到 302 响应后自动重连到新地址。

**本地现状**: RTMP 客户端不处理重定向。

**实现方案**:

在 RTMP client driver 中处理 `NetStream.Redirect`：

```rust
// cheetah-rtmp-driver-tokio client.rs
RtmpEvent::Redirect { new_url } => {
    // 断开当前连接
    // 解析新 URL
    // 重新建立连接到新地址
    // 重试次数限制（最多 3 次）
}
```

**配置**:
```yaml
modules:
  rtmp:
    pull_jobs:
      - name: source
        source_url: "rtmp://cdn.example.com/live/stream"
        follow_redirect: true
        max_redirects: 3
```
