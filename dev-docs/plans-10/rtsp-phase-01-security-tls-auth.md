# Phase-01: 安全层 — RTSPS + SHA-256 Digest 认证

## 目标

为 RTSP 服务添加 TLS 加密传输（RTSPS）和 SHA-256 Digest 认证算法，满足安全合规要求。

## 任务清单

### 1.1 driver-tokio 添加 TLS Acceptor

**范围**：`cheetah-rtsp-driver-tokio`

**实现方案**：

1. 新增依赖：`tokio-rustls`、`rustls-pemfile`
2. `listener.rs` 修改：
   - 新增独立 TLS listener（监听 RTSPS 端口，默认 322）
   - accept 后用 `TlsAcceptor` 包装为 `TlsStream<TcpStream>`
   - 统一抽象为 `enum TransportStream { Plain(TcpStream), Tls(TlsStream<TcpStream>) }`
   - 后续 read/write loop 对 `TransportStream` 透明操作（`AsyncRead + AsyncWrite`）
3. 证书加载：
   - 从配置路径加载 PEM 格式 cert + key
   - 支持可选 client CA（双向 TLS，预留接口）
   - 启动时验证证书有效性，失败则拒绝启动

**关键约束**：
- core 层不感知 TLS，不引入任何 TLS 依赖
- TLS 握手超时可配置（默认 10s）
- 连接级别标记 `is_tls: bool`，用于 module 层日志和鉴权策略

### 1.2 客户端支持 `rtsps://` 连接

**范围**：`cheetah-rtsp-driver-tokio` client 模块

**实现方案**：

1. URL 解析识别 `rtsps://` scheme
2. 客户端连接时使用 `TlsConnector` 包装 TCP stream
3. 支持配置：
   - `verify_server_cert: bool`（默认 true）
   - `ca_cert_path: Option<String>`（自定义 CA）
   - `skip_hostname_verify: bool`（调试用，默认 false）
4. Pull/Push/Relay 任务的 URL 支持 `rtsps://`

**兼容性**：
- 自签名证书场景需要 `verify_server_cert: false`
- 与 ZLMediaKit RTSPS 服务器互操作验证

### 1.3 配置模型添加 TLS 段

**范围**：`cheetah-rtsp-module` config

**新增配置**：

```yaml
modules:
  rtsp:
    tls:
      enabled: false
      listen: "0.0.0.0:322"
      cert_path: "/path/to/cert.pem"
      key_path: "/path/to/key.pem"
      client_ca_path: ""          # 可选，双向 TLS
      handshake_timeout_ms: 10000
      min_protocol_version: "tls1.2"  # tls1.2 | tls1.3
```

**验证规则**：
- `enabled: true` 时 `cert_path` 和 `key_path` 必须非空且文件存在
- 证书格式必须为 PEM
- 端口不能与 RTSP 明文端口冲突

### 1.4 Digest 认证支持 SHA-256

**范围**：`cheetah-rtsp-core` auth 模块

**实现方案**：

1. 新增依赖：`sha2` crate
2. `auth.rs` 扩展：
   - `DigestAlgorithm` 枚举：`Md5 | Sha256 | Sha256Sess`
   - `WWW-Authenticate` 头生成时包含 `algorithm=SHA-256`
   - 响应验证支持 `algorithm` 字段路由到对应哈希函数
   - 向后兼容：无 `algorithm` 字段时默认 MD5
3. 服务端行为：
   - 配置 `digest_algorithms: [sha-256, md5]`（优先级排序）
   - 401 响应同时发送多个 `WWW-Authenticate` 头（每个算法一个）
   - 客户端选择其支持的最强算法
4. 客户端行为：
   - 解析服务端 `algorithm` 字段
   - 优先使用 SHA-256，回退 MD5

**RFC 7616 关键点**：
- `algorithm=SHA-256` 使用 SHA-256 哈希
- `algorithm=SHA-256-sess` 使用 session 模式（A1 = H(user:realm:pass):nonce:cnonce）
- `userhash=true` 支持（用户名也做哈希，隐私保护）

### 1.5 认证 nonce 防重放增强

**范围**：`cheetah-rtsp-core` + `cheetah-rtsp-module`

**实现方案**：

1. core 层：
   - nonce 生成包含时间戳编码（用于 TTL 判断）
   - 支持 `qop=auth` 的 `nc`（nonce count）验证
   - `stale=true` 响应（nonce 过期但凭据正确时）
2. module 层：
   - nonce 使用计数跟踪（有界 HashMap，LRU 淘汰）
   - 配置 `nonce_ttl_secs`（默认 300）
   - 配置 `nonce_replay_window`（允许的最大 nc 跳跃，默认 32）

## 测试计划

| 测试类型 | 内容 |
|----------|------|
| 单元测试 | SHA-256 digest 计算正确性、nonce TTL 过期判断 |
| 属性测试 | 任意 user/pass/nonce 组合的 digest 往返一致性 |
| 集成测试 | TLS 握手成功/失败/超时、证书过期拒绝 |
| 互操作测试 | FFmpeg `rtsps://` 拉流、VLC RTSPS 播放 |
| fuzz | 畸形 `Authorization` 头解析不崩溃 |

## 完成标准

- [ ] `rtsps://` 推拉流端到端通过
- [ ] SHA-256 Digest 认证与 FFmpeg/VLC 互操作通过
- [ ] nonce 过期返回 `stale=true`，客户端自动重新认证
- [ ] TLS 配置错误时服务拒绝启动并给出明确错误信息
- [ ] 所有新增代码通过 clippy + fmt + test
