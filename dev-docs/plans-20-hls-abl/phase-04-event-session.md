# Phase 04 — 事件通知与会话增强

- **状态**: 未开始
- **目标**: 对标 ABLMediaServer 实现录像回放 HLS、Webhook 事件通知（on_play / on_record_ts / on_stream_none_reader）、Cookie 过期策略增强
- **影响 crate**: `cheetah-hls-module`（事件发布、录像编排）、`cheetah-hls-driver-tokio`（Cookie 增强）、`cheetah-hls-core`（会话过期模型）
- **参考源**: `NetServerHLS.cpp` (SendRecordHLS / Cookie)、`MediaStreamSource.cpp` (事件通知)

---

## 1. Webhook 事件通知

### ABL 实现分析

ABLMediaServer 在 HLS 流程中触发以下事件：

| 事件 | 触发时机 | 内容 |
|------|----------|------|
| `on_play` | 首次 m3u8 请求 | app, stream, networkType, key, ip, port, params |
| `on_record_ts` | 每个 segment 切片完成 | app, stream, fileName, duration, startTime, endTime, fileSize |
| `on_stream_none_reader` | 超时无人观看 | app, stream, mediaServerId, networkType |
| `on_play_disconnect` | HLS 会话超时断开 | app, stream, networkType, key, playDuration |

### 本地现状

当前 HLS module 无任何事件通知机制。

### 实现方案

通过 `EngineContext` 的事件系统发布 HLS 事件，不直接依赖 HTTP 框架：

```rust
// module/events.rs (新增)

pub enum HlsEvent {
    Play {
        stream_key: StreamKey,
        session_id: u64,
        client_ip: String,
        params: String,
    },
    PlayDisconnect {
        stream_key: StreamKey,
        session_id: u64,
        play_duration_secs: u64,
    },
    SegmentCreated {
        stream_key: StreamKey,
        segment_name: String,
        duration_ms: u64,
        file_size: usize,
    },
    NoneReader {
        stream_key: StreamKey,
        idle_duration_secs: u64,
    },
}
```

**触发点：**

1. **on_play**：在 `module.rs` 处理首次 m3u8 请求时（session 首次创建）
2. **on_play_disconnect**：在 session 超时清理时
3. **on_record_ts**：在 `muxer.rs` 完成一个 segment 切片时
4. **on_stream_none_reader**：在定期检查中发现流无活跃 session 超过阈值时

**配置项：**
```yaml
hls:
  events:
    on_play: true
    on_play_disconnect: true
    on_segment_created: true
    on_none_reader: true
    none_reader_timeout_secs: 30
```

---

## 2. 录像回放 HLS

### ABL 实现分析

ABLMediaServer 的 `SendRecordHLS()` 实现：
- URL 中包含特殊分隔符 `__ReplayFMP4RecordFile__` 标识录像回放
- 将 URL 路径转换为磁盘文件路径
- m3u8 和 segment 直接从磁盘读取
- 录像回放的 HLS 切片与实况共用同一套内存切片逻辑（通过 `MediaStreamSource` 统一处理）

### 实现方案

录像回放 HLS 分两种模式：

#### 模式 A：引擎回放流 → HLS 切片（推荐）

录像系统将回放流发布到引擎，HLS module 像处理实况流一样订阅并切片：

```
[录像系统] → publish(replay_stream) → [Engine] → subscribe → [HLS Module] → segment → [Client]
```

这种模式无需 HLS module 特殊处理，只需录像系统正确发布回放流。

#### 模式 B：磁盘 m3u8 直读（ABL 方式）

对于已经存在磁盘上的 HLS 录像文件（由磁盘切片模式产生），直接从文件系统读取：

```rust
// module/replay.rs (新增)

pub struct HlsReplayHandler {
    record_root_path: PathBuf,
}

impl HlsReplayHandler {
    /// 判断请求是否为录像回放
    pub fn is_replay_request(path: &str) -> bool {
        // 通过 URL 模式识别，如 /record/{app}/{stream}/index.m3u8
        path.starts_with("/record/")
    }
    
    /// 从磁盘读取 m3u8
    pub async fn read_playlist(&self, app: &str, stream: &str) -> Option<String> {
        let path = self.record_root_path.join(app).join(stream).join("index.m3u8");
        tokio::fs::read_to_string(path).await.ok()
    }
    
    /// 从磁盘读取 segment
    pub async fn read_segment(&self, app: &str, stream: &str, name: &str) -> Option<Bytes> {
        let path = self.record_root_path.join(app).join(stream).join(name);
        tokio::fs::read(path).await.ok().map(Bytes::from)
    }
}
```

**URL 设计：**
```
实况：http://host:8088/{app}/{stream}.m3u8
录像：http://host:8088/record/{app}/{stream}/{start_time}-{end_time}.m3u8
```

---

## 3. Cookie 过期策略增强

### ABL 实现分析

ABLMediaServer Cookie 策略：
- 格式：`Set-Cookie: AB_COOKIE=ABLMediaServer{18位序号};expires={当前时间+2分钟};path=/{app}/{stream}/`
- 2 分钟过期，每次 m3u8 请求刷新
- 用于标识唯一客户端会话

### 本地现状

当前有 Cookie 设置（`HLS_SESSION={id}`）但无 `expires` 属性，无 `path` 限定。

### 实现方案

**driver 层 Cookie 增强：**

```rust
fn build_set_cookie(session_id: u64, app: &str, stream: &str) -> String {
    let expires = Utc::now() + Duration::minutes(2);
    let expires_str = expires.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    format!(
        "Set-Cookie: HLS_SESSION={}; expires={}; path=/{}/{}/; HttpOnly",
        session_id, expires_str, app, stream
    )
}
```

**会话刷新逻辑：**
- 每次 m3u8 请求时更新 session 的 `last_request_time`
- 每次 m3u8 响应时重新设置 Cookie（刷新 expires）
- Session 超时（`session_timeout_secs`）后清理

**配置项：**
```yaml
hls:
  cookie_expire_secs: 120    # Cookie 过期时间（秒）
  cookie_path_scoped: true   # Cookie path 限定到流路径
```

---

## 4. 无人观看检测与自动清理

### ABL 实现分析

ABLMediaServer 的无人观看检测：
- `nLastWatchTime`：每次 m3u8 请求更新
- `noneReaderDuration`（默认 30s）：超过此时间触发 `on_stream_none_reader` 事件
- `maxTimeNoOneWatch`（默认 1 分钟）：超过此时间自动删除媒体源

### 实现方案

在 module 的定期清理任务中增加：

```rust
// module/module.rs 定期任务扩展

async fn check_idle_streams(&mut self) {
    let now = Instant::now();
    for (stream_key, muxer_state) in &self.stream_muxers {
        let idle_secs = now.duration_since(muxer_state.last_request_time).as_secs();
        
        // 触发无人观看事件
        if idle_secs >= self.config.none_reader_timeout_secs && !muxer_state.none_reader_notified {
            self.emit_event(HlsEvent::NoneReader {
                stream_key: stream_key.clone(),
                idle_duration_secs: idle_secs,
            });
            muxer_state.none_reader_notified = true;
        }
        
        // 自动停止切片（hls_demand 模式）
        if idle_secs >= self.config.session_timeout_secs {
            self.stop_muxer(stream_key).await;
        }
    }
}
```

---

## 5. 播放参数透传

### ABL 实现分析

ABLMediaServer 从首次 m3u8 请求的 URL query 中提取参数（如鉴权 token），存储在 `szPlayParams` 中，随事件通知一起发送。

### 实现方案

```rust
// core/request.rs 扩展
pub struct HlsRequest {
    pub path: HlsPath,
    pub method: HttpMethod,
    pub query_params: String,  // 原始 query string
    pub client_ip: Option<String>,
}

// module 层在创建 session 时保存 params
struct SessionState {
    session_id: u64,
    last_request_us: u64,
    bytes_sent: u64,
    play_params: String,      // 首次请求的 query params
    client_ip: String,
    created_at: Instant,
}
```

---

## 验收标准

- [ ] 首次 m3u8 请求触发 on_play 事件（通过 EngineContext 可观测）
- [ ] Session 超时触发 on_play_disconnect 事件
- [ ] 每个 segment 完成触发 on_segment_created 事件
- [ ] 无人观看超过 30 秒触发 on_none_reader 事件
- [ ] Cookie 包含正确的 expires 和 path 属性
- [ ] 录像回放 HLS（模式 A）：回放流可通过 HLS 播放
- [ ] 播放参数正确透传到事件中

---

## 测试计划

```bash
# 单元测试
cargo test -p cheetah-hls-core
cargo test -p cheetah-hls-module

# 事件验证（需要 control API 或日志观测）
RUST_LOG=debug cargo run -p cheetah-server --features hls
# 观察日志中的事件输出
```
