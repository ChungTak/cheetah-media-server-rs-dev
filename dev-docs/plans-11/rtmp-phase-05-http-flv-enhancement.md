# RTMP Phase 05 — HTTP-FLV 增强

- **状态**: 未开始
- **范围**: HTTP-FLV Push（POST 推流）、HTTPS-FLV（TLS）、WSS-FLV（TLS WebSocket）
- **完成标准**: HTTP POST 可推流，HTTPS-FLV/WSS-FLV 可正常播放，TLS 握手验证通过

---

## 目标

扩展 HTTP-FLV 生态能力：

1. 支持通过 HTTP POST 推送 FLV 流（HTTP-FLV Push）
2. 支持 HTTPS-FLV 加密播放
3. 支持 WSS-FLV（WebSocket over TLS）加密播放

---

## 设计约束

- TLS 支持在 `cheetah-http-flv-driver-tokio` 中实现，复用 `rustls`
- HTTP-FLV Push 在 core 层扩展请求解析，module 层增加 publish 管线
- 与 RTMP TLS 实现保持一致的配置模型和证书加载方式
- 不引入额外 HTTP 框架依赖，继续使用手写 HTTP 解析

---

## 任务分解

### 5.1 HTTP-FLV Push（POST 推流）

**目标**: 支持通过 HTTP POST 方式推送 FLV 流到服务器。

**实现**:

1. `cheetah-http-flv-core` 请求解析扩展：

```rust
/// HTTP 请求类型
pub enum HttpFlvRequest {
    /// GET 请求 → 播放（已有）
    Play {
        app: String,
        stream: String,
        enhanced: bool,
    },
    /// POST 请求 → 推流（新增）
    Publish {
        app: String,
        stream: String,
    },
    /// OPTIONS 请求 → CORS（已有）
    Cors,
    /// WebSocket Upgrade → WS-FLV 播放（已有）
    WebSocketPlay {
        app: String,
        stream: String,
        enhanced: bool,
    },
}

/// 解析 HTTP 请求
pub fn parse_request(method: &str, path: &str, headers: &Headers) -> Option<HttpFlvRequest> {
    match method {
        "GET" => parse_play_request(path, headers),
        "POST" => parse_publish_request(path),
        "OPTIONS" => Some(HttpFlvRequest::Cors),
        _ => None,
    }
}
```

2. `cheetah-http-flv-core` FLV ingest 状态机：

```rust
/// HTTP-FLV Push 状态机
pub struct HttpFlvIngestState {
    splitter: FlvSplitter,
    state: IngestState,
}

enum IngestState {
    /// 等待 FLV header (9 bytes)
    WaitingHeader,
    /// 等待 PreviousTagSize0 (4 bytes)
    WaitingFirstPrevTagSize,
    /// 正常解析 FLV tags
    Parsing,
}

pub enum IngestOutput {
    /// 需要更多数据
    NeedMore,
    /// 解析出一个媒体帧
    Frame(MediaFrame),
    /// FLV header 解析完成
    HeaderParsed { has_video: bool, has_audio: bool },
    /// 解析错误
    Error(IngestError),
}

impl HttpFlvIngestState {
    /// 输入 HTTP body 数据
    pub fn feed(&mut self, data: &[u8]) -> Vec<IngestOutput> {
        // 复用已有的 flv_ingest.rs 解析逻辑
        // ...
    }
}
```

3. `cheetah-http-flv-driver-tokio` POST 处理：

```rust
async fn handle_connection(stream: TcpStream) {
    let request = parse_http_request(&stream).await?;

    match request {
        HttpFlvRequest::Play { .. } => handle_play(stream, request).await,
        HttpFlvRequest::Publish { app, stream_name } => {
            // 发送 200 OK 响应
            send_response(&stream, 200, "OK").await?;
            // 持续读取 body，解析 FLV tags
            handle_publish(stream, app, stream_name).await
        }
        // ...
    }
}

async fn handle_publish(mut tcp: TcpStream, app: String, stream_name: String) {
    let mut ingest = HttpFlvIngestState::new();
    let mut buf = BytesMut::with_capacity(65536);

    loop {
        let n = tcp.read_buf(&mut buf).await?;
        if n == 0 { break; } // 连接关闭

        let outputs = ingest.feed(&buf);
        buf.clear();

        for output in outputs {
            match output {
                IngestOutput::Frame(frame) => {
                    // 发送到 module 层
                    event_tx.send(HttpFlvEvent::MediaFrame { frame }).await?;
                }
                IngestOutput::Error(e) => {
                    // 记录错误，关闭连接
                    break;
                }
                _ => {}
            }
        }
    }
}
```

4. `cheetah-http-flv-module` publish 管线：

```rust
fn handle_publish_event(&mut self, app: &str, stream: &str, frame: MediaFrame) {
    let stream_key = StreamKey::new(app, stream);

    // 复用 RTMP module 的 ingest 逻辑：
    // FLV tag → codec 参数解析 → AVFrame + TrackInfo → engine publish
    self.ingest_pipeline.process_frame(&stream_key, frame);
}
```

5. 配置项：

```yaml
modules:
  http_flv:
    enable_push: true  # 是否允许 HTTP POST 推流, 默认 false
    push_auth_token: ""  # 推流鉴权 token, 空 = 不鉴权
```

**测试**:
- 单元测试：POST 请求解析
- 单元测试：FLV ingest 状态机（header → tags → frames）
- 集成测试：`curl -X POST -T file.flv http://host:8080/live/test.flv` → 拉流验证
- 集成测试：推流鉴权验证

---

### 5.2 HTTPS-FLV（TLS 加密）

**目标**: 支持 HTTPS 方式播放 FLV 流。

**实现**:

1. `cheetah-http-flv-driver-tokio` TLS 扩展：

```rust
/// 启动 HTTPS-FLV 服务器
pub async fn start_tls_server(
    config: HttpFlvTlsConfig,
    event_tx: mpsc::Sender<HttpFlvEvent>,
) -> Result<()> {
    let tls_config = load_tls_config(&config.cert_path, &config.key_path)?;
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));
    let listener = TcpListener::bind(&config.listen).await?;

    loop {
        let (tcp_stream, addr) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let event_tx = event_tx.clone();

        tokio::spawn(async move {
            let tls_stream = match tokio::time::timeout(
                Duration::from_secs(config.handshake_timeout_secs),
                acceptor.accept(tcp_stream),
            ).await {
                Ok(Ok(stream)) => stream,
                _ => return, // TLS 握手失败或超时
            };

            handle_connection(tls_stream, addr, event_tx).await;
        });
    }
}
```

2. 配置模型：

```yaml
modules:
  http_flv:
    listen: 0.0.0.0:8080
    tls:
      enabled: true
      listen: 0.0.0.0:8443
      cert_path: /path/to/cert.pem
      key_path: /path/to/key.pem
      handshake_timeout_secs: 5
```

3. 设计要点：
   - HTTP 和 HTTPS 可同时监听不同端口
   - TLS 配置结构与 RTMPS 保持一致
   - 复用 `rustls`，不引入 OpenSSL
   - `handle_connection` 对 `TcpStream` 和 `TlsStream` 使用泛型或 trait object

4. 泛型连接处理：

```rust
/// 统一连接处理（支持 TCP 和 TLS）
async fn handle_connection<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    addr: SocketAddr,
    event_tx: mpsc::Sender<HttpFlvEvent>,
) {
    // 解析 HTTP 请求、处理 play/publish/websocket
    // ...
}
```

**测试**:
- 集成测试：HTTPS-FLV 播放（curl + ffplay）
- 集成测试：TLS 证书加载失败 → 优雅降级
- 集成测试：TLS 握手超时处理

---

### 5.3 WSS-FLV（TLS WebSocket）

**目标**: 支持 WebSocket over TLS 方式播放 FLV 流。

**实现**:

1. WSS-FLV 自动支持：
   - 由于 5.2 中 `handle_connection` 已泛型化，WebSocket upgrade 在 TLS 连接上自动工作
   - WSS 连接通过 HTTPS 端口进入，HTTP 解析后检测 `Upgrade: websocket` 头
   - 无需额外代码，只需确认 WebSocket 握手在 TLS 流上正常工作

2. URL 格式：

```
HTTPS-FLV: https://host:8443/{app}/{stream}.flv
WSS-FLV:   wss://host:8443/{app}/{stream}.flv
```

3. 验证点：
   - WebSocket 握手（`Sec-WebSocket-Key` → `Sec-WebSocket-Accept`）在 TLS 流上正确完成
   - FLV 数据通过 WebSocket binary frame 发送
   - 客户端 close frame 正确处理

**测试**:
- 集成测试：WSS-FLV 播放（浏览器模拟或 wscat）
- 集成测试：WSS 连接异常断开 → 资源正确释放
- 集成测试：同时 HTTP + HTTPS + WS + WSS 四种方式播放同一流
