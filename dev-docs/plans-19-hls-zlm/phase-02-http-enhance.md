# Phase 02 — HTTP 服务增强 + Cookie 高级策略

- **状态**: 未开始
- **范围**: HTTP Range 请求、条件请求 (ETag/304)、OPTIONS 预检完善、Cookie 多设备限制与挤占策略
- **完成标准**: Range 请求可部分下载 segment，ETag 304 减少带宽，Cookie 限制设备数生效

---

## 2.1 HTTP Range 请求支持

**ZLMediaKit 参考**: `HttpFileManager` 解析 `Range: bytes=start-end`，返回 206 Partial Content。

**实现方案**:

在 driver 层 HTTP 响应中支持 Range：

```rust
// cheetah-hls-driver-tokio/src/server.rs

struct RangeRequest {
    start: u64,
    end: Option<u64>,  // None = to end of file
}

fn parse_range_header(header: &str) -> Option<RangeRequest> {
    // Parse "bytes=0-1023" or "bytes=1024-"
}

fn build_partial_response(
    data: &[u8],
    range: &RangeRequest,
    content_type: &str,
) -> HttpResponse {
    // Status: 206 Partial Content
    // Content-Range: bytes {start}-{end}/{total}
    // Content-Length: {end - start + 1}
}
```

**适用场景**:
- 大 segment 文件的断点续传
- 播放器 seek 到 segment 中间位置
- CDN 回源时的分片拉取

**响应头**:
```
HTTP/1.1 206 Partial Content
Content-Range: bytes 0-1023/4096
Content-Length: 1024
Accept-Ranges: bytes
```

---

## 2.2 HTTP 条件请求 (ETag / If-None-Match)

**ZLMediaKit 参考**: `HttpSession` 对静态文件生成 ETag，匹配时返回 304。

**实现方案**:

```rust
// 为每个 segment 生成 ETag（基于 segment seq + stream key hash）
fn segment_etag(stream_key: &str, seq: u64) -> String {
    format!("\"{:x}-{}\"", hash(stream_key), seq)
}

// 为 m3u8 生成 ETag（基于 media_sequence）
fn playlist_etag(stream_key: &str, media_sequence: u64) -> String {
    format!("\"{:x}-m{}\"", hash(stream_key), media_sequence)
}

// 请求处理中：
if let Some(if_none_match) = request_headers.get("If-None-Match") {
    if if_none_match == &current_etag {
        return Response::not_modified_304(current_etag);
    }
}
```

**响应头**:
```
HTTP/1.1 200 OK
ETag: "a1b2c3-5"
Cache-Control: no-cache
```

**304 响应**:
```
HTTP/1.1 304 Not Modified
ETag: "a1b2c3-5"
```

**策略**:
- `.m3u8`: `Cache-Control: no-cache`（允许缓存但必须验证）
- `.ts/.m4s`: `Cache-Control: max-age=86400`（segment 不变）
- `init.mp4`: `Cache-Control: max-age=86400`

---

## 2.3 OPTIONS 预检完善

**ZLMediaKit 参考**: `HttpSession::Handle_Req_OPTIONS` 返回完整 CORS 预检响应。

**实现方案**:

```rust
fn handle_options_request(origin: Option<&str>) -> HttpResponse {
    Response::builder()
        .status(204)
        .header("Access-Control-Allow-Origin", origin.unwrap_or("*"))
        .header("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS")
        .header("Access-Control-Allow-Headers", "Range, If-None-Match, Cookie")
        .header("Access-Control-Expose-Headers", "Content-Range, ETag, Set-Cookie")
        .header("Access-Control-Max-Age", "86400")
        .header("Access-Control-Allow-Credentials", "true")
        .body_empty()
}
```

**改动点**:
- 当前仅在 GET 响应中添加 CORS 头
- 需要独立处理 OPTIONS 方法
- 需要 `Access-Control-Allow-Credentials: true` 以支持跨域 Cookie

---

## 2.4 Cookie 多设备限制

**ZLMediaKit 参考**: `HttpCookieManager::addCookie(cookie_name, uid, max_elapsed, attach, max_client)` 限制同一 uid 最多 N 个设备。

**实现方案**:

```rust
// cheetah-hls-core/src/session.rs — 扩展 HlsCore

pub struct CookiePolicy {
    /// Maximum concurrent sessions per uid (0 = unlimited).
    pub max_sessions_per_uid: usize,
    /// Eviction strategy when limit exceeded.
    pub eviction: EvictionStrategy,
}

pub enum EvictionStrategy {
    /// Evict the oldest session (first-in-first-out).
    EvictOldest,
    /// Reject new session (deny login).
    RejectNew,
}
```

**流程**:
1. 新 session 创建时，检查同一 uid 的活跃 session 数
2. 若超过 `max_sessions_per_uid`：
   - `EvictOldest`: 强制过期最早的 session，发送 `SessionEvicted` 事件
   - `RejectNew`: 返回 403 Forbidden
3. 被挤占的 session 下次请求时收到 401 + 新 Set-Cookie

**配置**:
```yaml
modules:
  hls:
    cookie_policy:
      max_sessions_per_uid: 3
      eviction: "evict_oldest"
```

---

## 2.5 Cookie 异地挤占登录

**ZLMediaKit 参考**: `HttpCookieManager::getOldestCookie()` 找到最早的 cookie 并删除。

**实现方案**:

```rust
impl HlsCore {
    fn enforce_session_limit(&mut self, uid: &str) -> Vec<HlsCoreOutput> {
        let sessions_for_uid: Vec<_> = self.sessions.values()
            .filter(|s| s.uid.as_deref() == Some(uid))
            .collect();

        if sessions_for_uid.len() >= self.cookie_policy.max_sessions_per_uid {
            // Find oldest by created_at
            let oldest = sessions_for_uid.iter()
                .min_by_key(|s| s.created_at)
                .unwrap();
            let evicted_id = oldest.id;
            // Mark as expired, emit event
            outputs.push(HlsCoreOutput::Event(HlsCoreEvent::SessionEvicted {
                session_id: evicted_id,
                reason: "max_sessions_exceeded",
            }));
        }
        outputs
    }
}
```

**事件通知**:
- 被挤占的 session 在下次请求时返回 `401 Unauthorized`
- 响应体包含 JSON: `{"error": "session_evicted", "reason": "new_device_login"}`
- 同时 Set-Cookie 清除旧 cookie

---

## 2.6 HTTP HEAD 请求支持

**实现方案**:

```rust
// HEAD 请求返回与 GET 相同的 headers，但无 body
fn handle_head_request(/* same params as GET */) -> HttpResponse {
    let mut resp = handle_get_request(/* ... */);
    resp.set_body_empty();
    resp
}
```

**用途**: CDN 探测、播放器预检文件大小。

---

## 验证方法

1. `curl -H "Range: bytes=0-187" .../seg_0.ts` → 验证 206 + 正确数据
2. `curl -H "If-None-Match: \"etag\"" .../index.m3u8` → 验证 304
3. `curl -X OPTIONS .../live/test.m3u8` → 验证完整 CORS 预检响应
4. 同一 uid 超过 max_sessions → 验证最早 session 被挤占
5. 被挤占 session 再次请求 → 验证 401 响应
6. `curl -I .../seg_0.ts` → 验证 HEAD 返回正确 Content-Length
