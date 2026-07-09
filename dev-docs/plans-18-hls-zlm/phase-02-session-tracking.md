# Phase 02 — Cookie 会话追踪 + 按需生成

- **状态**: 未开始
- **范围**: Cookie 会话追踪、按需 HLS 生成、播放统计、模拟长连接
- **完成标准**: 播放器通过 Cookie 被追踪为独立会话，无人观看时停止生成

---

## 2.1 Cookie/Set-Cookie 会话追踪

**ZLMediaKit 参考**: `HttpServerCookie` + `HlsCookieData`，首次请求 m3u8 时 Set-Cookie，后续请求携带 Cookie 刷新 TTL。

**实现方案**:

在 driver 层 HTTP 响应中注入 Cookie：

```rust
// cheetah-hls-driver-tokio/src/server.rs
struct HlsSession {
    id: u64,
    stream_key: String,
    created_at: u64,      // micros
    last_access: u64,     // micros
    bytes_sent: u64,
    segments_served: u64,
}
```

**流程**:
1. 客户端首次请求 `/{app}/{stream}.m3u8`
2. 服务端生成 session_id，响应头 `Set-Cookie: HLS_SESSION={id}; Path=/{app}/{stream}/; HttpOnly`
3. 后续请求解析 `Cookie: HLS_SESSION={id}`，更新 `last_access`
4. 无 Cookie 的请求创建新 session

**改动点**:
- Driver `run_connection`: 解析请求头中的 Cookie
- Driver 响应: 注入 Set-Cookie
- Module: 维护 `SessionMap` 使用 Cookie ID 而非 URL uid 参数
- 兼容: 同时支持 `?uid=` 参数（无 Cookie 客户端回退）

---

## 2.2 按需 HLS 生成（hls_demand）

**ZLMediaKit 参考**: `onReaderChanged()` 检测 reader count，为 0 时停止 muxing。

**实现方案**:

```rust
// module.rs — 在 cleanup_expired_sessions 中
// 当某 stream 的所有 session 过期且 hls_demand=true:
// 1. 向 subscriber task 发送暂停信号
// 2. subscriber 停止调用 push_frame
// 3. 新 session 到来时恢复

pub struct StreamMuxerControl {
    enabled: Arc<AtomicBool>,
}
```

**配置**:
```yaml
modules:
  hls:
    hls_demand: true  # 按需生成，无人观看时暂停
```

**改动点**:
- `StreamMuxer`: 新增 `enabled` 原子标志
- `run_subscriber`: 检查 `enabled` 再调用 `push_frame`
- `cleanup_expired_sessions`: 当 stream 无 session 时设 `enabled=false`
- `handle_core_event`: 新请求到来时设 `enabled=true`

---

## 2.3 播放统计

**ZLMediaKit 参考**: `HlsCookieData::addByteUsage()` 累计字节数和时长。

**实现方案**:

```rust
// 在 handle_core_event 的 SegmentRequested 处理中：
// 发送 segment 后更新 session 统计
session.bytes_sent += segment_data.len() as u64;
session.segments_served += 1;
```

**统计上报**: 通过 `EngineContext::metrics_api` 上报：
- 每 stream 的活跃 session 数
- 每 session 的累计字节数
- 每 stream 的总带宽

---

## 2.4 模拟长连接

**ZLMediaKit 参考**: Cookie TTL 在每次请求时刷新，使 HLS 的多次 HTTP 请求表现为一个逻辑"连接"。

**实现方案**:

- Session TTL = `session_timeout_secs`（默认 10s）
- 每次请求（m3u8 或 .ts）刷新 `last_access`
- 播放器正常轮询 m3u8（每 segment_duration/2）会持续刷新
- 停止播放后 TTL 到期 → session 销毁 → 触发"断开"事件

**事件**:
```rust
enum HlsSessionEvent {
    Connected { session_id, stream_key, peer_addr },
    Disconnected { session_id, bytes_sent, duration_secs },
}
```

---

## 验证方法

1. 浏览器播放 → 检查 Set-Cookie 响应头
2. 停止播放 → 验证 session 在 timeout 后被清理
3. hls_demand=true → 无人观看时验证 CPU 使用下降
4. 统计 API → 验证 bytes_sent 累计正确
