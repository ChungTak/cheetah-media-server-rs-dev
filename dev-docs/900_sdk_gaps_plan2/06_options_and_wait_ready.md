# 06 · R4 + R5：Options 透传与 `wait_ready` 真就绪

> **Agent 用途**：阶段 4 主文档。  
> **R4**：公共 options 真正影响底层。  
> **R5**：`PushHandle::wait_ready()` 等待可写，非 stub。

---

## 1. 目标 / 非目标

| 项 | 目标 |
| --- | --- |
| R4 | HTTP-FLV read_limits / buffer_size / queue；loopback queue；subscriber queue 可配置 |
| R5 | RTMP / WebRTC（及未来）push 在协议 ready 前阻塞 wait_ready |
| 非目标 | 通用重试框架 UI；所有协议 100 个冷门选项 |

---

## 2. R4：Options 透传

### 2.1 现状

```rust
// pull/http_flv.rs（基线）
read_limits: Default::default(),
buffer_size: 64,
// ConnectorPullOptions.subscriber 基本未用于队列
// LoopbackOptions.queue_capacity 未使用
```

### 2.2 proposed 映射表

| 公共选项 | 底层 |
| --- | --- |
| `ConnectorPullOptions.subscriber.queue_capacity` | HTTP-FLV `buffer_size` 或专用 queue 字段；RTSP 队列 |
| `ConnectorPullOptions` 扩展 `http_flv_read_limits: Option<PullReadLimits>` | `HttpFlvSubscriberOptions.read_limits` |
| `ConnectorPullOptions` 扩展 `reconnect` | streaming reconnect |
| `LoopbackOptions.queue_capacity` | 两侧 push/pull 队列与 subscriber options |
| `ConnectorPushOptions.publisher` | 已有则保持；扩展 publish buffer 若有 |

```rust
// proposed options 增量（字段名以实现为准）
pub struct ConnectorPullOptions {
    pub subscriber: cheetah_sdk::SubscriberOptions,
    pub cancel: CancellationToken,
    pub reconnect: Option<ReconnectPolicy>,
    pub http_flv: HttpFlvPullExtras, // read_limits, buffer_size override
    pub rtsp: RtspPullExtras,
}

pub struct HttpFlvPullExtras {
    pub read_limits: Option<PullReadLimits>,
    pub buffer_size: Option<usize>,
}
```

**规则**：

1. `None` → 文档化默认（与现今默认一致，避免静默行为大变）。  
2. `Some` → **必须**传到底层（单测断言或注入 mock）。  
3. `queue_capacity == 0` → `InvalidArgument`。

### 2.3 实现触点

```text
src/options.rs
src/pull/http_flv.rs
src/pull/rtsp.rs          # R1
src/push/rtmp.rs
src/push/webrtc.rs        # R2
src/loopback.rs
```

### 2.4 测试

| ID | 用例 | 期望 |
| --- | --- | --- |
| T-OPT-01 | 自定义 read_limits 过小触发可读错误 | 非默认行为可观测 |
| T-OPT-02 | buffer_size/queue=1 推多帧 | 背压/drop 可观测且不 OOM |
| T-OPT-03 | loopback queue_capacity 生效 | 与固定 64 行为不同或计数断言 |
| T-OPT-04 | queue_capacity=0 | InvalidArgument |

---

## 3. R5：`wait_ready`

### 3.1 现状

```rust
// handles.rs
// TODO: wire protocol-specific readiness signalling.
Ok(())
```

### 3.2 语义（钉死）

```text
wait_ready() 返回 Ok(()) 当且仅当：
  - 推流会话已完成必要握手，且
  - 至少可以接受 update_tracks/push_frame 进入发送路径（或文档定义的“connected”）

返回 Err：
  - 超时（若 options 带 timeout）
  - 握手失败（typed Protocol/Connect）
  - 已 close / cancel → Closed
```

**幂等**：ready 后再次 `wait_ready` → 立即 Ok。

### 3.3 协议就绪信号

| 协议 | 建议就绪条件 |
| --- | --- |
| RTMP | publish `NetStream.Publish.Start` 或等价 onStatus；实现时 rg 既有事件 |
| WebRTC | WHIP answer 完成 + DTLS/ICE connected（或 module 已有 SessionState） |
| fixture WebRTC | harness 标记 ready（可立即） |

```bash
rg -n 'Publish.Start|onStatus|SessionState|Connected|wait_ready' \
  crates/protocols/rtmp crates/protocols/webrtc crates/sdk/cheetah-connector \
  --glob '*.rs' | head -40
```

### 3.4 实现形态

```rust
// proposed
pub struct PushHandle {
    readiness: Readiness, // oneshot/watch/Notify
    protocol: Protocol,
    // ...
}

impl PushHandle {
    pub async fn wait_ready(&self) -> Result<(), ConnectorError> {
        self.readiness.wait().await.map_err(|e| map(self.protocol, e))
    }
}
```

Adapter 在后台任务收到就绪事件时 `readiness.signal()`。

可选：

```rust
pub async fn wait_ready_timeout(&self, dur: Duration) -> Result<(), ConnectorError>;
```

### 3.5 测试

| ID | 用例 | 期望 |
| --- | --- | --- |
| T-RDY-01 | RTMP loopback：wait_ready 后首帧 Accepted | 无 sleep 作为唯一同步 |
| T-RDY-02 | 在 ready 前可选行为 | 文档+测 |
| T-RDY-03 | cancel 期间 wait_ready | Closed/Cancelled |
| T-RDY-04 | 幂等第二次 wait_ready | Ok |
| T-RDY-05 | 失败握手 | Err 非 Ok |

**禁止**：测试里 `tokio::time::sleep(2s).await` 作为 ready 的唯一手段（可作 upper bound timeout，但断言应挂在 wait_ready）。

---

## 4. DoD（阶段 4）

### R4

- [ ] HTTP-FLV 不再无视 read_limits/buffer/queue options  
- [ ] LoopbackOptions.queue_capacity 使用  
- [ ] T-OPT-* 绿  

### R5

- [ ] handles 中 TODO stub 删除  
- [ ] RTMP（及已接线 WebRTC）信号就绪  
- [ ] T-RDY-* 绿；loopback 测改为 wait_ready  

---

## 5. 衔接

- R1/R2 adapter 必须接入同一 readiness/options 模式。  
- R6 loopback 两侧受益于 wait_ready，减少 flaky。  
