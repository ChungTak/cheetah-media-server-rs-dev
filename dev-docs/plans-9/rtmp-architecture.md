# RTMP 协议完善架构设计

- 状态：已完成
- 范围：整体架构设计、crate 边界调整、数据流模型、TLS 集成方案
- 完成标准：架构设计评审通过，各阶段实现有明确的 crate 归属和接口定义

## 1. 现有架构概览

```
┌─────────────────────────────────────────────────────────┐
│                  cheetah-rtmp-module                      │
│  (引擎接入、会话管理、Ingest/Egress、Pull/Push Jobs)      │
└────────────────────────┬────────────────────────────────┘
                         │ 调用
┌────────────────────────▼────────────────────────────────┐
│               cheetah-rtmp-driver-tokio                   │
│  (TCP 监听、连接管理、读写循环、客户端连接、背压控制)       │
└────────────────────────┬────────────────────────────────┘
                         │ 驱动
┌────────────────────────▼────────────────────────────────┐
│                  cheetah-rtmp-core                        │
│  (Sans-I/O 状态机、握手、Chunk、AMF、消息、Enhanced RTMP)  │
└─────────────────────────────────────────────────────────┘
                         │ 依赖
┌────────────────────────▼────────────────────────────────┐
│                    cheetah-codec                          │
│  (AVFrame、TrackInfo、时间戳归一化、参数集缓存)            │
└─────────────────────────────────────────────────────────┘
```

## 2. 架构扩展点

### 2.1 RTMPS 传输层（Phase 01）

TLS 集成在 driver 层，不影响 core 的 Sans-I/O 约束。

```
┌─────────────────────────────────────────┐
│         cheetah-rtmp-driver-tokio         │
│                                           │
│  ┌─────────────┐   ┌─────────────────┐  │
│  │ TcpListener  │   │ TlsAcceptor     │  │
│  │ (port 1935)  │   │ (port 1936)     │  │
│  └──────┬───────┘   └───────┬─────────┘  │
│         │                    │            │
│         ▼                    ▼            │
│  ┌──────────────────────────────────┐    │
│  │   统一 AsyncRead + AsyncWrite     │    │
│  │   (tokio::io trait objects)       │    │
│  └──────────────┬───────────────────┘    │
│                 │                         │
│                 ▼                         │
│  ┌──────────────────────────────────┐    │
│  │   Connection Handler (共用)       │    │
│  └──────────────────────────────────┘    │
└─────────────────────────────────────────┘
```

设计要点：
- 使用 `tokio-rustls` 提供 TLS 能力，不引入 OpenSSL 系统依赖。
- 服务端和客户端共用 TLS 配置抽象。
- 连接处理器通过 `AsyncRead + AsyncWrite` trait 对象统一处理 TCP 和 TLS 连接。
- TLS 配置（证书路径、密钥路径、ALPN）放在 module 配置中，由 driver 消费。

### 2.2 编解码分层（Phase 02）

编解码能力分为两层：

```
┌─────────────────────────────────────────────────────┐
│                 cheetah-rtmp-core                     │
│                                                       │
│  ┌─────────────────────────────────────────────┐    │
│  │  RTMP 封装/解封装 (Mux/Demux)                │    │
│  │  - 识别 codec ID / FourCC                    │    │
│  │  - 提取 sequence header / config record      │    │
│  │  - 构建 RTMP audio/video 消息                │    │
│  └──────────────────────┬──────────────────────┘    │
└─────────────────────────┼───────────────────────────┘
                          │ AVFrame + TrackInfo
┌─────────────────────────▼───────────────────────────┐
│                   cheetah-codec                       │
│                                                       │
│  ┌─────────────────────────────────────────────┐    │
│  │  编解码处理 (Codec Processing)               │    │
│  │  - AVCC/HVCC/AV1CodecConfig 解析             │    │
│  │  - 参数集缓存与补发                          │    │
│  │  - 时间戳归一化                              │    │
│  │  - Access Unit 拼装                          │    │
│  └─────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────┘
```

编解码支持分级：
- **完整支持**（可转协议）：H.264、H.265、AAC、G.711、Opus、MP3、VP8、VP9、AV1
- **透传支持**（仅 RTMP→RTMP 转发）：其他所有编码，保持原始 RTMP 消息不解析

### 2.3 Relay 数据流（Phase 03）

```
Remote RTMP Server A                    Remote RTMP Server B
       │                                        ▲
       │ Pull                                   │ Push
       ▼                                        │
┌──────────────────────────────────────────────────────┐
│                  cheetah-rtmp-module                   │
│                                                        │
│  ┌──────────┐   ┌──────────┐   ┌──────────────────┐ │
│  │ PullJob   │   │ RelayJob  │   │ PushJob          │ │
│  │ (远程→本地)│   │ (远程→远程)│   │ (本地→远程)      │ │
│  └─────┬─────┘   └─────┬─────┘   └────────┬────────┘ │
│        │               │                   │          │
│        ▼               ▼                   ▼          │
│  ┌──────────────────────────────────────────────┐    │
│  │           StreamManager (Engine)              │    │
│  │  - 发布租约管理                               │    │
│  │  - 订阅者分发                                 │    │
│  │  - GOP 缓存                                   │    │
│  └──────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────┘
```

Relay 任务 = Pull + Push 的组合，共享同一个 StreamKey：
- Pull 端从远程拉流写入本地 StreamManager
- Push 端从本地 StreamManager 订阅并推送到远程
- Relay 任务作为原子单元管理（启动/停止/重试）

### 2.4 兼容性层设计（Phase 04）

```
┌─────────────────────────────────────────────────────┐
│                 cheetah-rtmp-core                     │
│                                                       │
│  ┌─────────────────────────────────────────────┐    │
│  │  compat 模块                                 │    │
│  │  - QuirksRegistry (厂商特征检测)             │    │
│  │  - CommandCompat (FCPublish/releaseStream)   │    │
│  │  - HandshakeCompat (复杂握手可选支持)        │    │
│  │  - ChunkCompat (非标准 chunk size 容忍)     │    │
│  └─────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────┘
```

兼容性处理原则：
- 入口宽容：接受非标准输入，自动检测厂商特征。
- 内部规范：所有数据经过规范化后进入引擎。
- 出口稳定：输出严格符合规范，可预测。
- 集中管理：所有 quirks 在 `compat` 模块中显式注册，不散落在各处。

## 3. 配置模型扩展

```yaml
modules:
  rtmp:
    enabled: true
    listen: 0.0.0.0:1935
    # Phase 01: RTMPS
    tls:
      enabled: false
      listen: 0.0.0.0:1936
      cert_path: /path/to/cert.pem
      key_path: /path/to/key.pem
    # Phase 03: Relay
    relay_jobs:
      - name: relay_to_cdn
        enabled: true
        source_url: rtmp://source.example.com/live/stream
        target_url: rtmp://cdn.example.com/live/stream
        retry_backoff_ms: 1000
        max_retry_backoff_ms: 30000
    # Phase 04: 兼容性
    compat:
      enable_complex_handshake: false
      max_chunk_size: 10_000_000
      fcpublish_as_publish: true
    # Phase 05: 鉴权
    auth:
      enabled: false
      hook_url: http://localhost:8080/api/rtmp/auth
      timeout_ms: 3000
```

## 4. 依赖变更计划

| 阶段 | crate | 新增依赖 | 说明 |
|------|-------|---------|------|
| Phase 01 | cheetah-rtmp-driver-tokio | `tokio-rustls`, `rustls-pemfile` | TLS 传输 |
| Phase 01 | cheetah-rtmp-module | 无 | 配置模型扩展 |
| Phase 02 | cheetah-codec | 无（已有能力扩展） | VP8/G711/MP3 路径 |
| Phase 04 | cheetah-rtmp-core | `hmac`, `sha2`（可选 feature） | 复杂握手 |
| Phase 05 | cheetah-rtmp-module | `reqwest`（通过 SDK 抽象） | HTTP 鉴权回调 |

## 5. 不实现的功能（明确排除）

| 功能 | 排除原因 |
|------|---------|
| RTMPT (HTTP 隧道) | 已过时，现代客户端不使用 |
| 共享对象 (Shared Object) | 使用场景极少，可通过 WebSocket 替代 |
| AMF3 命令消息 | simple-media-server 也不支持，实际客户端不使用 |
| 录制/DVR | 属于独立模块职责，不在 RTMP 协议层实现 |
| 自适应码率 | 属于转码模块职责 |

## 6. 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| TLS 性能开销 | 高并发场景吞吐下降 | 使用 rustls 的 zero-copy API，连接池复用 |
| 复杂握手兼容性 | 部分老客户端可能需要 | 作为可选 feature，默认关闭 |
| Relay 任务资源泄漏 | 长时间运行的任务可能泄漏 | 统一生命周期管理，CancellationToken |
| 编解码路径不完整 | 某些编码无法转协议 | 明确分级：完整支持 vs 透传支持 |
