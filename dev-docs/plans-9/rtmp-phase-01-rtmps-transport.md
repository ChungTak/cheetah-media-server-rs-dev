# RTMP Phase 01 — RTMPS 传输层

- 状态：已完成
- 范围：在 driver 层集成 TLS，支持 RTMPS 服务端和客户端
- 完成标准：OBS/FFmpeg 可通过 `rtmps://` 推拉流，TLS 握手成功率 100%，性能回归 < 10%

## 目标

为 RTMP 协议增加 TLS 传输层支持，使服务端可同时监听 RTMP (TCP) 和 RTMPS (TLS) 端口，客户端可连接 RTMPS 远程服务器进行推流/拉流。

## 设计约束

- TLS 逻辑完全在 driver 层，不影响 core 的 Sans-I/O 约束。
- 使用 `tokio-rustls`（基于 rustls），不引入 OpenSSL 系统依赖。
- 服务端和客户端复用同一套 TLS 配置抽象。
- 连接处理器通过泛型或 trait 对象统一处理 TCP 和 TLS 连接，避免代码重复。

## 任务分解

### 1.1 rustls 集成到 driver

**目标**：在 `cheetah-rtmp-driver-tokio` 中引入 TLS 依赖并建立抽象。

**实现**：
- 在 `Cargo.toml` 中添加 `tokio-rustls` 和 `rustls-pemfile` 依赖（精确版本）。
- 定义 `TlsConfig` 结构体：
  ```rust
  pub struct TlsConfig {
      pub cert_chain: Vec<CertificateDer<'static>>,
      pub private_key: PrivateKeyDer<'static>,
  }
  ```
- 提供 `TlsConfig::from_pem_files(cert_path, key_path)` 加载方法。
- 定义统一连接类型 `RtmpStream`：
  ```rust
  pub enum RtmpStream {
      Tcp(TcpStream),
      Tls(TlsStream<TcpStream>),
  }
  ```
- 为 `RtmpStream` 实现 `AsyncRead + AsyncWrite`（委托到内部类型）。

**测试**：单元测试验证 PEM 文件加载、`RtmpStream` trait 实现。

### 1.2 TLS 服务端 acceptor

**目标**：服务端可在独立端口接受 RTMPS 连接。

**实现**：
- 在 server driver 中增加可选的 `TlsAcceptor`。
- 启动时根据配置决定是否创建 TLS 监听器。
- TLS 握手超时设置为 5 秒（防止慢速攻击）。
- TLS 握手失败记录日志并关闭连接，不影响其他连接。
- 握手成功后将 `TlsStream` 包装为 `RtmpStream::Tls` 进入统一处理流程。

**实现细节**：
```rust
// server.rs 扩展
async fn accept_tls_connection(
    tcp_stream: TcpStream,
    tls_acceptor: &TlsAcceptor,
    timeout: Duration,
) -> Result<TlsStream<TcpStream>, TlsAcceptError> {
    tokio::time::timeout(timeout, tls_acceptor.accept(tcp_stream))
        .await
        .map_err(|_| TlsAcceptError::Timeout)?
        .map_err(TlsAcceptError::Handshake)
}
```

**测试**：集成测试使用自签名证书验证 TLS 握手。

### 1.3 TLS 客户端 connector

**目标**：客户端驱动可连接 `rtmps://` 远程服务器。

**实现**：
- 在 client driver 中根据 `RtmpUrl.tls` 标志决定是否使用 TLS。
- 创建 `TlsConnector`，配置 SNI（从 URL host 提取）。
- 支持可选的自定义 CA 证书（用于内网部署）。
- 默认使用系统根证书（通过 `rustls-native-certs` 或 `webpki-roots`）。
- 连接失败时的错误信息包含 TLS 相关上下文。

**实现细节**：
```rust
// client.rs 扩展
async fn connect_tls(
    tcp_stream: TcpStream,
    server_name: ServerName<'static>,
    connector: &TlsConnector,
) -> Result<TlsStream<TcpStream>, ConnectError> {
    connector.connect(server_name, tcp_stream)
        .await
        .map_err(ConnectError::TlsHandshake)
}
```

**测试**：集成测试验证客户端连接到本地 RTMPS 服务端。

### 1.4 配置模型扩展

**目标**：在 module 配置中增加 TLS 相关字段。

**实现**：
- 扩展 `RtmpModuleConfig`：
  ```rust
  pub struct RtmpTlsConfig {
      pub enabled: bool,
      pub listen: SocketAddr,       // 默认 0.0.0.0:1936
      pub cert_path: PathBuf,
      pub key_path: PathBuf,
      pub handshake_timeout_ms: u64, // 默认 5000
  }
  ```
- 配置验证：`enabled=true` 时必须提供有效的证书和密钥路径。
- 支持环境变量覆盖：`M7S_MODULE__rtmp__tls__enabled=true`。
- 配置变更返回 `ModuleRestartRequired`。

**测试**：配置解析和验证的单元测试。

### 1.5 RTMPS 集成测试

**目标**：端到端验证 RTMPS 推拉流。

**测试场景**：
1. 自签名证书生成（测试 fixture）。
2. RTMPS 服务端启动并监听。
3. RTMPS 客户端推流（模拟 publish）。
4. RTMPS 客户端拉流（模拟 play）。
5. RTMP 和 RTMPS 同时工作互不干扰。
6. 证书过期/无效时的错误处理。
7. TLS 握手超时测试。

**验证命令**：
```bash
# 生成测试证书
openssl req -x509 -newkey rsa:2048 -keyout test_key.pem -out test_cert.pem -days 1 -nodes -subj '/CN=localhost'

# FFmpeg 推流到 RTMPS
ffmpeg -re -i test.flv -c copy -f flv rtmps://localhost:1936/live/test

# FFplay 拉流
ffplay rtmps://localhost:1936/live/test
```

## 性能考量

- TLS 握手是一次性开销，对长连接影响可忽略。
- 数据传输加密开销：rustls 使用 AES-GCM 硬件加速，预期吞吐下降 < 5%。
- 不在 TLS 层做额外缓冲，复用现有 driver 的读写缓冲策略。
- 监控指标：TLS 握手延迟、握手失败率。

## 回滚策略

- TLS 功能通过配置开关控制，`tls.enabled=false` 时完全不加载 TLS 代码路径。
- 如果 rustls 出现兼容性问题，可通过 feature flag 切换到 `native-tls` 后端。
