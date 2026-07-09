# RTMP Phase 03 — 鲁棒性增强

- **状态**: 未开始
- **范围**: 断连续推、Paced Sender 平滑发送、直接代理模式
- **完成标准**: 断连续推可在配置窗口内恢复，Paced Sender 可平滑突发流量，直接代理模式可零解析转发

---

## 目标

提升 RTMP 服务器在生产环境中的鲁棒性：

1. 发布者短暂断连后可无缝恢复，订阅者无感知
2. 对突发源流进行平滑发送，避免客户端缓冲区溢出
3. 纯转发场景跳过 demux/remux，降低 CPU 开销

---

## 设计约束

- 断连续推在 module 层实现，通过引擎发布租约模型扩展
- Paced Sender 在 driver 层实现，不影响 core 状态机
- 直接代理在 module 层实现，标记流为 passthrough 模式
- 所有新增能力默认禁用，通过配置开启

---

## 任务分解

### 3.1 断连续推（发布保活窗口）

**目标**: 发布者断连后保持流活跃一段时间，允许重连恢复。

**实现**:

1. 配置项：

```yaml
modules:
  rtmp:
    publish_keepalive_ms: 3000  # 0 = 禁用, 默认 0
```

2. Module 层逻辑：

```rust
/// 发布者断连时的处理
fn on_publisher_disconnected(&mut self, stream_key: &StreamKey) {
    let keepalive_ms = self.config.publish_keepalive_ms;
    if keepalive_ms == 0 {
        // 立即释放流（当前行为）
        self.engine.unpublish(stream_key);
        return;
    }

    // 启动保活定时器
    let timer = self.runtime.delay(Duration::from_millis(keepalive_ms));
    self.keepalive_timers.insert(stream_key.clone(), KeepaliveState {
        timer,
        original_session_id: session_id,
    });
    // 不发送 EOF，订阅者继续等待
}

/// 同一 StreamKey 重新 publish
fn on_publish_request(&mut self, stream_key: &StreamKey, session_id: SessionId) {
    if let Some(keepalive) = self.keepalive_timers.remove(stream_key) {
        // 在保活窗口内重连 → 恢复
        keepalive.timer.cancel();
        // 绑定新 session 到已有流，不重建
        self.rebind_publisher(stream_key, session_id);
        return;
    }
    // 正常 publish 流程
    // ...
}

/// 保活定时器超时
fn on_keepalive_timeout(&mut self, stream_key: &StreamKey) {
    self.keepalive_timers.remove(stream_key);
    self.engine.unpublish(stream_key);
    // 通知所有订阅者 EOF
}
```

3. 恢复时的时间戳处理：
   - 重连后的第一帧时间戳可能回跳或跳跃
   - 使用 `cheetah-codec` 的时间戳修正器进行平滑过渡
   - 对订阅者透明，不产生时间戳断裂

4. 边界条件：
   - 保活期间新的不同发布者请求同一 StreamKey → 拒绝（单发布者独占）
   - 保活期间流被管理 API 强制关闭 → 取消保活，立即释放
   - 保活期间服务器关闭 → 正常清理

**测试**:
- 单元测试：保活定时器启动/取消/超时
- 集成测试：推流 → 断连 → 2s 内重连 → 拉流无中断
- 集成测试：推流 → 断连 → 超时 → 拉流收到 EOF
- 集成测试：保活期间另一发布者尝试 publish → 拒绝

---

### 3.2 Paced Sender 平滑发送

**目标**: 对突发源流进行匀速发送，避免下游客户端缓冲区溢出。

**实现**:

1. 配置项：

```yaml
modules:
  rtmp:
    paced_sender_ms: 0  # 0 = 禁用, 建议值 35-100ms
```

2. Driver 层实现：

```rust
/// Paced sender 状态
struct PacedSender {
    interval: Duration,
    buffer: VecDeque<PacedPacket>,
    max_buffer_size: usize,
    timer: Interval,
}

struct PacedPacket {
    data: Bytes,
    enqueue_time: Instant,
}

impl PacedSender {
    fn new(interval_ms: u64) -> Self {
        Self {
            interval: Duration::from_millis(interval_ms),
            buffer: VecDeque::with_capacity(256),
            max_buffer_size: 1024,
            timer: tokio::time::interval(Duration::from_millis(interval_ms)),
        }
    }

    /// 入队数据包
    fn enqueue(&mut self, data: Bytes) {
        if self.buffer.len() >= self.max_buffer_size {
            // 缓冲区满：丢弃最旧的非关键帧
            self.drop_oldest_non_keyframe();
        }
        self.buffer.push_back(PacedPacket {
            data,
            enqueue_time: Instant::now(),
        });
    }

    /// 定时器触发：批量发送
    fn on_tick(&mut self) -> Vec<Bytes> {
        // 每个 tick 发送积累的所有包（匀速化）
        self.buffer.drain(..).map(|p| p.data).collect()
    }
}
```

3. 集成到 egress 发送循环：

```rust
// driver send loop (per connection)
loop {
    tokio::select! {
        frame = rx.recv() => {
            if paced_sender_enabled {
                paced_sender.enqueue(frame);
            } else {
                tcp_write(frame).await;
            }
        }
        _ = paced_sender.timer.tick(), if paced_sender_enabled => {
            let batch = paced_sender.on_tick();
            for data in batch {
                tcp_write(data).await;
            }
        }
    }
}
```

4. 设计要点：
   - Paced Sender 是 per-connection 的，不是全局的
   - 只影响 egress（发送给播放者），不影响 ingest
   - 缓冲区有上界，溢出时丢弃非关键帧

**测试**:
- 单元测试：PacedSender 入队/出队/溢出
- 集成测试：突发 100 帧 → paced sender 平滑输出
- 性能测试：paced sender 开启/关闭对延迟的影响

---

### 3.3 直接代理模式

**目标**: 纯 RTMP→RTMP 转发场景跳过 demux/remux，降低 CPU 开销。

**实现**:

1. 配置项：

```yaml
modules:
  rtmp:
    direct_proxy: false  # 默认禁用
```

2. Module 层实现：

```rust
/// 直接代理流的数据模型
pub struct DirectProxyStream {
    /// 原始 RTMP 包缓冲（不解析为 AVFrame）
    ring_buffer: RingBuffer<RtmpPacket>,
    /// 元数据包（用于新订阅者 bootstrap）
    metadata_packet: Option<RtmpPacket>,
    /// 视频/音频 config 包
    video_config: Option<RtmpPacket>,
    audio_config: Option<RtmpPacket>,
    /// GOP 缓存
    gop_cache: VecDeque<RtmpPacket>,
    max_gop_size: usize,
}
```

3. Publish 路径（直接代理模式）：

```
RTMP Publish → chunk decode → RtmpPacket
    │
    ├── 识别 metadata/config 包 → 缓存
    ├── 识别关键帧 → 更新 GOP 缓存起点
    └── 所有包 → ring_buffer（不解析 payload）
```

4. Play 路径（直接代理模式）：

```
新订阅者:
    1. 发送 metadata_packet
    2. 发送 video_config + audio_config
    3. 发送 GOP 缓存
    4. 订阅 ring_buffer 实时包

ring_buffer 新包:
    → 直接 chunk encode → TCP write
```

5. 限制：
   - 直接代理流**不支持**跨协议转发（HTTP-FLV、RTSP 等）
   - 直接代理流**不支持**录制
   - 直接代理流**不支持**时间戳修正
   - 直接代理流**不支持**静音音频注入
   - 直接代理流**不支持**转码

6. 自动降级：
   - 如果有非 RTMP 订阅者请求直接代理流 → 返回错误或自动切换为正常模式

**测试**:
- 集成测试：直接代理推流 → RTMP 拉流正常
- 集成测试：直接代理流 → HTTP-FLV 拉流返回错误
- 性能测试：直接代理 vs 正常模式 CPU 对比
- 集成测试：GOP 缓存 + bootstrap 正确性
