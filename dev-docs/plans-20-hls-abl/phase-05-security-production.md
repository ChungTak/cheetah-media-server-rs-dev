# Phase 05 — 安全防护与生产加固

- **状态**: 未开始
- **目标**: 对标 ABLMediaServer 的安全检查和生产级特性，实现请求校验、HTTPS/TLS、HLS 播放统计、非标兼容加固
- **影响 crate**: `cheetah-hls-driver-tokio`（安全检查、TLS）、`cheetah-hls-module`（统计、监控）
- **参考源**: `NetServerHLS.cpp` (ProcessNetData / 安全检查)、`ABLMediaServer.cpp` (SSL 绑定)

---

## 1. 请求安全校验

### ABL 实现分析

ABLMediaServer 在 `ProcessNetData()` 中执行多层安全检查：

```c
// 1. 数据长度检查
if (netDataCacheLength > string_length_4096 || strstr((char*)netDataCache, "%") != NULL) {
    // 立即断开
    pDisconnectBaseNetFifo.push(&nClient, sizeof(nClient));
    return -1;
}

// 2. HTTP 方法检查
if (!(memcmp(netDataCache, "GET ", 4) == 0 || memcmp(netDataCache, "HEAD ", 5) == 0)) {
    // 非法请求，断开
}

// 3. 文件类型检查
// 仅允许 .m3u8, .ts, .mp4
```

### 实现方案

**driver 层请求校验（`server.rs`）：**

```rust
/// 请求安全校验配置
pub struct HlsSecurityConfig {
    /// 最大请求头长度（字节）
    pub max_request_size: usize,        // 默认 4096
    /// 是否拒绝含 % 编码的 URL（防注入）
    pub reject_percent_encoding: bool,  // 默认 true
    /// 允许的 HTTP 方法
    pub allowed_methods: Vec<HttpMethod>, // 默认 [GET, HEAD, OPTIONS]
    /// 允许的文件扩展名
    pub allowed_extensions: Vec<String>, // 默认 [.m3u8, .ts, .m4s, .mp4]
    /// 最大 URL 路径长度
    pub max_path_length: usize,         // 默认 512
}

fn validate_request(req: &RawRequest, config: &HlsSecurityConfig) -> Result<(), SecurityError> {
    // 1. 请求大小检查
    if req.raw_bytes.len() > config.max_request_size {
        return Err(SecurityError::RequestTooLarge);
    }
    
    // 2. % 编码检查（防止路径遍历攻击）
    if config.reject_percent_encoding && req.path.contains('%') {
        return Err(SecurityError::IllegalCharacter);
    }
    
    // 3. 方法检查
    if !config.allowed_methods.contains(&req.method) {
        return Err(SecurityError::MethodNotAllowed);
    }
    
    // 4. 扩展名检查
    if !config.allowed_extensions.iter().any(|ext| req.path.ends_with(ext)) {
        return Err(SecurityError::ForbiddenFileType);
    }
    
    // 5. 路径长度检查
    if req.path.len() > config.max_path_length {
        return Err(SecurityError::PathTooLong);
    }
    
    // 6. 路径遍历检查
    if req.path.contains("..") {
        return Err(SecurityError::PathTraversal);
    }
    
    Ok(())
}
```

**错误响应：**
- `SecurityError::RequestTooLarge` → 直接关闭连接（不响应）
- `SecurityError::IllegalCharacter` → 直接关闭连接
- `SecurityError::MethodNotAllowed` → 405 Method Not Allowed
- `SecurityError::ForbiddenFileType` → 403 Forbidden
- `SecurityError::PathTooLong` → 414 URI Too Long
- `SecurityError::PathTraversal` → 400 Bad Request

**配置项：**
```yaml
hls:
  security:
    max_request_size: 4096
    reject_percent_encoding: true
    max_path_length: 512
    reject_path_traversal: true
```

---

## 2. HTTPS/TLS 支持

### ABL 实现分析

ABLMediaServer 通过端口奇偶判断是否启用 SSL：
- 偶数端口 → HTTP
- 奇数端口 → HTTPS（自动加载 `server.pem` / `privkey.pem` / `cacert.pem`）

### 实现方案

使用 `tokio-rustls` 实现 TLS：

```rust
// driver/tls.rs (新增)

pub struct HlsTlsAcceptor {
    acceptor: TlsAcceptor,
}

impl HlsTlsAcceptor {
    pub fn new(cert_path: &Path, key_path: &Path) -> Result<Self, TlsError> {
        let cert_chain = load_certs(cert_path)?;
        let key = load_private_key(key_path)?;
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)?;
        Ok(Self { acceptor: TlsAcceptor::from(Arc::new(config)) })
    }
    
    pub async fn accept(&self, stream: TcpStream) -> Result<TlsStream<TcpStream>, TlsError> {
        self.acceptor.accept(stream).await.map_err(Into::into)
    }
}
```

**server.rs 改造：**
```rust
pub async fn start_server(config: &HlsConfig) -> HlsServerHandle {
    let listener = TcpListener::bind(&config.listen).await?;
    let tls_acceptor = if let Some(tls_config) = &config.tls {
        Some(HlsTlsAcceptor::new(&tls_config.cert_path, &tls_config.key_path)?)
    } else {
        None
    };
    
    loop {
        let (tcp_stream, addr) = listener.accept().await?;
        if let Some(tls) = &tls_acceptor {
            let tls_stream = tls.accept(tcp_stream).await?;
            spawn(handle_connection(tls_stream, ...));
        } else {
            spawn(handle_connection(tcp_stream, ...));
        }
    }
}
```

**配置项：**
```yaml
hls:
  listen: "0.0.0.0:8088"
  tls:
    enabled: false
    cert_path: "./certs/server.pem"
    key_path: "./certs/privkey.pem"
```

---

## 3. HLS 播放统计

### ABL 实现分析

ABLMediaServer 通过 `getOutList` API 返回 HLS 播放对象统计：
- 统计发送 m3u8 的对象（`nNetServerHLS_SendFileType == NetServerHLS_SendM3u8File`）
- 包含 app, stream, networkType, duration, dst_url, dst_port

### 实现方案

在 module 层维护播放统计，通过 control API 暴露：

```rust
// module/stats.rs (新增)

pub struct HlsStreamStats {
    pub stream_key: StreamKey,
    pub active_sessions: usize,
    pub total_sessions: u64,
    pub total_bytes_sent: u64,
    pub total_segments_served: u64,
    pub uptime_secs: u64,
    pub sessions: Vec<HlsSessionStats>,
}

pub struct HlsSessionStats {
    pub session_id: u64,
    pub client_ip: String,
    pub connected_secs: u64,
    pub bytes_sent: u64,
    pub segments_received: u64,
    pub last_request_secs_ago: u64,
}
```

**通过 SDK ModuleApi 暴露：**
```rust
impl HlsModule {
    pub fn get_stats(&self) -> Vec<HlsStreamStats> {
        // 遍历所有活跃流和 session，汇总统计
    }
}
```

---

## 4. 速率限制

### 实现方案

防止单个客户端过于频繁请求：

```rust
pub struct RateLimiter {
    /// 每个 IP 的请求计数（滑动窗口）
    requests: HashMap<IpAddr, VecDeque<Instant>>,
    /// 窗口大小
    window: Duration,
    /// 窗口内最大请求数
    max_requests: usize,
}

impl RateLimiter {
    pub fn check(&mut self, ip: IpAddr, now: Instant) -> bool {
        let entry = self.requests.entry(ip).or_default();
        // 清理过期记录
        while entry.front().map_or(false, |t| now.duration_since(*t) > self.window) {
            entry.pop_front();
        }
        if entry.len() >= self.max_requests {
            return false; // 限流
        }
        entry.push_back(now);
        true
    }
}
```

**配置项：**
```yaml
hls:
  rate_limit:
    enabled: false
    window_secs: 1
    max_requests_per_window: 50  # 每秒最多 50 个请求/IP
```

---

## 5. 连接数限制

### 实现方案

限制 HLS 服务的总并发连接数：

```rust
pub struct ConnectionLimiter {
    current: AtomicUsize,
    max: usize,
}

impl ConnectionLimiter {
    pub fn try_acquire(&self) -> Option<ConnectionGuard> {
        let prev = self.current.fetch_add(1, Ordering::Relaxed);
        if prev >= self.max {
            self.current.fetch_sub(1, Ordering::Relaxed);
            None
        } else {
            Some(ConnectionGuard { limiter: self })
        }
    }
}
```

**配置项：**
```yaml
hls:
  max_connections: 10000
```

---

## 6. 非标兼容加固

### 基于 ABL 实践的兼容措施

| 措施 | 说明 |
|------|------|
| Content-Type 宽松 | TS 使用 `video/mp2t`，部分播放器需要 `application/octet-stream` |
| Date 头可选 | 某些代理要求 Date 头，添加标准格式 |
| Server 头 | 添加 `Server: Cheetah-HLS/1.0` 标识 |
| 空 playlist 处理 | segment 不足时返回 404 而非空 m3u8（VLC 兼容） |
| ETag 弱匹配 | 使用 `W/"seg_name"` 弱 ETag，兼容代理缓存 |
| CORS 凭证 | `Access-Control-Allow-Credentials: true` |

---

## 验收标准

- [ ] 超过 4096 字节的请求被拒绝（连接关闭）
- [ ] 含 `%` 的 URL 被拒绝
- [ ] 仅 GET/HEAD/OPTIONS 方法被接受
- [ ] 路径遍历（`..`）被拒绝
- [ ] HTTPS 模式下 HLS 流可正常播放
- [ ] 播放统计 API 返回正确的 session 信息
- [ ] 速率限制生效（超限返回 429）
- [ ] 连接数限制生效（超限返回 503）

---

## 测试计划

```bash
# 安全测试
curl -v "http://127.0.0.1:8088/live/../../../etc/passwd"  # 应返回 400
curl -v "http://127.0.0.1:8088/live/test%2e%2e.m3u8"      # 应被拒绝
curl -X POST "http://127.0.0.1:8088/live/test.m3u8"       # 应返回 405

# TLS 测试
curl -k https://127.0.0.1:8089/live/test.m3u8

# 压力测试
ab -n 10000 -c 100 http://127.0.0.1:8088/live/test.m3u8
```
