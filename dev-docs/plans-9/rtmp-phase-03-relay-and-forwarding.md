# RTMP Phase 03 — Relay 转发与 Push/Pull 增强

- 状态：已完成
- 范围：实现 Relay 转发任务、增强现有 Push/Pull 能力、建立跨协议转发基础
- 完成标准：Relay 任务可配置启动，支持 RTMP/RTMPS 源和目标，故障自动恢复，资源无泄漏

## 目标

1. 实现 Relay 转发任务（远程 A → 本地 → 远程 B）。
2. 增强现有 Pull/Push 任务的健壮性和可观测性。
3. 建立跨协议转发的基础设施（RTMP→RTSP、RTMP→HTTP-FLV 已通过 StreamManager 天然支持，此处确保 Relay 场景正确工作）。

## 设计

### Relay 任务模型

Relay = Pull + Push 的原子组合：

```
Remote Source ──Pull──▶ StreamManager ──Push──▶ Remote Target
     (A)                  (local)                   (B)
```

- Relay 任务作为单一配置单元管理。
- 内部拆分为 Pull 子任务和 Push 子任务。
- Pull 子任务成功建立流后，Push 子任务才启动。
- 任一子任务失败，整个 Relay 任务进入重试。
- Relay 任务共享同一个 StreamKey。

### 生命周期状态机

```
┌─────────┐    配置启用    ┌──────────┐   Pull成功   ┌──────────┐
│  Idle    │──────────────▶│ Pulling  │────────────▶│ Relaying │
└─────────┘               └──────────┘             └──────────┘
     ▲                         │                        │
     │         失败/超时        │       失败/断开         │
     │◀────────────────────────┘◀───────────────────────┘
     │                                                   
     │              退避等待                              
     └──────────────────────────────────────────────────
```

## 任务分解

### 3.1 Relay 任务模型

**目标**：定义 Relay 任务的配置、状态和管理接口。

**配置模型**：
```rust
pub struct RtmpRelayJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,          // rtmp:// 或 rtmps://
    pub target_url: String,          // rtmp:// 或 rtmps://
    pub stream_key: Option<String>,  // 本地 StreamKey，默认从 source_url 提取
    pub retry_backoff_ms: u64,       // 初始重试间隔，默认 1000
    pub max_retry_backoff_ms: u64,   // 最大重试间隔，默认 30000
    pub pull_timeout_ms: u64,        // Pull 连接超时，默认 10000
    pub push_timeout_ms: u64,        // Push 连接超时，默认 10000
}
```

**状态管理**：
```rust
pub enum RelayJobState {
    Idle,
    Pulling { since: Instant },
    Relaying { since: Instant, pull_id: ConnectionId, push_id: ConnectionId },
    RetryWait { next_attempt: Instant, attempt_count: u32 },
    Stopped,
}
```

**module 层接口**：
- `start_relay_job(config) -> Result<JobId>`
- `stop_relay_job(job_id) -> Result<()>`
- `get_relay_job_status(job_id) -> RelayJobState`

### 3.2 Relay 驱动实现

**目标**：实现 Relay 任务的后台 supervisor。

**实现**：
- Relay supervisor 作为 module 层的后台任务运行。
- 使用 `CancellationToken` 管理生命周期（遵守 module 约束，不使用 `tokio::select!`）。
- Pull 阶段：
  - 使用现有 client driver 的 Play 模式连接 source_url。
  - 等待首帧到达确认 Pull 成功。
  - 将流发布到本地 StreamManager。
- Push 阶段：
  - 订阅本地 StreamManager 中的流。
  - 使用现有 client driver 的 Publish 模式连接 target_url。
  - 转发所有帧到远程。
- 故障处理：
  - Pull 断开：停止 Push，整体重试。
  - Push 断开：仅重试 Push（Pull 保持）。
  - 两端都断开：整体重试。

**资源管理**：
- Relay 任务停止时，确保 Pull 和 Push 连接都被关闭。
- StreamManager 中的发布租约被释放。
- 所有 spawn 的子任务通过 CancellationToken 级联取消。

### 3.3 Pull/Push 增强

**目标**：增强现有 Pull/Push 任务的健壮性和可观测性。

**增强项**：

1. **连接健康检测**：
   - 心跳超时检测（无数据超过 N 秒视为断开）。
   - 参考 simple-media-server 的 `_readClock` / `_rtmpClock` 机制。
   - 配置项：`health_check_interval_ms`（默认 10000）、`no_data_timeout_ms`（默认 30000）。

2. **重连策略增强**：
   - 当前：固定指数退避。
   - 增加：最大重试次数限制（可选，0=无限）。
   - 增加：重试成功后重置退避计数器。
   - 增加：连接成功但短时间内断开（< 5s）视为不稳定，加速退避。

3. **状态上报**：
   - Pull/Push 任务暴露当前状态（Connected/Reconnecting/Failed）。
   - 暴露统计信息：连接时长、重试次数、最后错误、字节吞吐。
   - 通过 module 的 `EngineContext` 上报，供 control API 查询。

4. **RTMPS 支持**：
   - Pull/Push 任务的 URL 支持 `rtmps://` 协议。
   - 复用 Phase 01 实现的 TLS 客户端 connector。

5. **Chunk Size 优化**：
   - 参考 simple-media-server 客户端设置 chunk size = 5,000,000。
   - 大 chunk size 减少分片开销，适合高码率推流。
   - 配置项：`client_chunk_size`（默认 60000，可调大）。

### 3.4 跨协议转发基础

**目标**：确保 RTMP Relay/Pull 的流可被其他协议模块正确消费。

**验证场景**：
1. RTMP Pull → RTSP Play（通过 StreamManager 自动路由）。
2. RTMP Pull → HTTP-FLV Play。
3. RTSP Push → RTMP Play（反向，确认 StreamManager 双向工作）。

**需确认的边界**：
- Pull 任务写入 StreamManager 的 `AVFrame` 格式是否满足 RTSP/HTTP-FLV 模块的消费要求。
- 时间戳基准是否统一（RTMP 使用 ms，RTSP 使用 90kHz clock）。
- 参数集（SPS/PPS/VPS）是否通过 `TrackInfo` 正确传递。
- GOP 缓存是否对跨协议订阅者正确工作。

**cheetah-codec 确认项**：
- `AVFrame` 的 timebase 字段是否被正确设置。
- 跨协议时间戳转换是否在 codec 层统一处理。

## 配置示例

```yaml
modules:
  rtmp:
    enabled: true
    listen: 0.0.0.0:1935
    # Pull 任务
    pull_jobs:
      - name: pull_from_origin
        enabled: true
        source_url: rtmp://origin.example.com/live/main
        target_stream_key: live/main
        retry_backoff_ms: 1000
        max_retry_backoff_ms: 30000
        health_check_interval_ms: 10000
        no_data_timeout_ms: 30000
    # Push 任务
    push_jobs:
      - name: push_to_cdn
        enabled: true
        source_stream_key: live/main
        target_url: rtmps://cdn.example.com/live/main
        retry_backoff_ms: 1000
        max_retry_backoff_ms: 30000
    # Relay 任务
    relay_jobs:
      - name: relay_origin_to_cdn
        enabled: true
        source_url: rtmp://origin.example.com/live/main
        target_url: rtmps://cdn.example.com/live/main
        retry_backoff_ms: 1000
        max_retry_backoff_ms: 30000
```

## 测试计划

1. **单元测试**：Relay 状态机转换、重试逻辑、超时计算。
2. **集成测试**：
   - Relay 任务启动 → Pull 成功 → Push 成功 → 数据流通。
   - Pull 断开 → 自动重试 → 恢复。
   - Push 断开 → 仅 Push 重试 → 恢复。
   - 配置停止 → 资源释放验证。
3. **压力测试**：多个 Relay 任务并发运行，验证资源隔离。
4. **互操作测试**：与 simple-media-server 互推互拉。
