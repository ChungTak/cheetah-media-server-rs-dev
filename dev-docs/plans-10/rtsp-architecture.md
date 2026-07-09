# RTSP 协议完善 — 整体架构演进

## 现有架构回顾

```
┌─────────────────────────────────────────────────────────────┐
│  cheetah-rtsp-module (Engine 接入层)                         │
│  ├── 会话管理 (Publish/Play)                                │
│  ├── 后台任务 (Pull/Push/Relay)                             │
│  ├── 认证执行                                               │
│  ├── 组播注册                                               │
│  └── 配置 + 生命周期                                        │
├─────────────────────────────────────────────────────────────┤
│  cheetah-rtsp-driver-tokio (I/O 驱动层)                     │
│  ├── TCP Listener + Accept                                  │
│  ├── Per-connection read/write loop                         │
│  ├── HTTP Tunnel (GET/POST pairing)                         │
│  ├── UDP 端口分配 + NAT 探测                                │
│  └── Client TCP 连接管理                                    │
├─────────────────────────────────────────────────────────────┤
│  cheetah-rtsp-core (Sans-I/O 状态机)                        │
│  ├── RTSP 消息解析/编码                                     │
│  ├── SDP 解析/构建                                          │
│  ├── RTP/RTCP 头解析                                        │
│  ├── Transport 头解析                                       │
│  ├── Basic/Digest 认证逻辑                                  │
│  ├── Interleaved 帧编解码                                   │
│  └── CoreInput/CoreOutput 事件模型                          │
└─────────────────────────────────────────────────────────────┘
```

## 架构演进目标

本次完善不改变三段式架构，而是在各层内部扩展能力：

```
┌─────────────────────────────────────────────────────────────┐
│  module 层新增                                               │
│  ├── 静音音频生成（调用 cheetah-codec MuteAudioMaker）       │
│  ├── Direct Proxy 零解码转发模式                             │
│  ├── 断连续推（source keep-alive + ownership 延迟释放）      │
│  ├── Scale/Speed 业务逻辑                                   │
│  └── RTCP-FB 关键帧请求调度                                 │
├─────────────────────────────────────────────────────────────┤
│  driver 层新增                                               │
│  ├── TLS Acceptor (rustls) → RTSPS                          │
│  ├── TLS Client Connector → rtsps:// 拉流                   │
│  ├── RTP 重排序缓冲区（timer 驱动）                          │
│  ├── RTCP-FB 包收发                                         │
│  └── UDP NAT 穿透增强                                       │
├─────────────────────────────────────────────────────────────┤
│  core 层新增                                                 │
│  ├── SHA-256 Digest 算法                                    │
│  ├── RTCP-FB 包模型（NACK/PLI/FIR）                         │
│  ├── RTP seq 回绕/重置检测逻辑                               │
│  ├── RTCP-XR 包模型                                         │
│  └── 非标 SDP/Transport 兼容解析                            │
└─────────────────────────────────────────────────────────────┘
```

## 关键设计决策

### 1. TLS 放置位置

TLS 终止放在 **driver 层**，与 ZLMediaKit 的 `SessionWithSSL` 模板包装思路一致：

- `listener.rs` 在 accept 后根据配置决定是否包装 `tokio-rustls` TLS stream
- core 层完全不感知 TLS，输入输出仍是字节流
- 配置模型新增 `tls` 段（cert_path, key_path, optional client_ca）

```
Client ──TLS──→ [rustls TlsAcceptor] ──plaintext──→ driver read/write loop ──→ core
```

### 2. 静音音频生成位置

放在 **module 层**，由 `cheetah-codec` 提供 `MuteAudioMaker` 工具：

- module 检测到流只有视频轨时，启用静音音频注入
- 视频帧时间戳驱动 AAC 静音帧生成
- 配置项 `enable_mute_audio: bool`（默认 true）
- 不影响 core 和 driver

### 3. RTP 重排序缓冲区位置

放在 **driver 层**（timer 驱动）：

- core 提供排序判断逻辑（seq 比较、回绕检测）
- driver 持有实际缓冲区，用 timer 驱动超时释放
- 可配置缓冲区大小和超时时间
- 仅 UDP 传输启用，TCP 不需要

### 4. Direct Proxy 模式

放在 **module 层**：

- 当源和目标都是 RTSP 且编解码器匹配时，跳过 depacketize→AVFrame→packetize 路径
- 直接将 RTP 包从 subscriber queue 转发到目标连接
- 节省 CPU 但失去 GOP 对齐和编解码器转换能力
- 配置项 `enable_direct_proxy: bool`

### 5. 非标兼容集中管理

在 core 层新增 `compat` 模块：

```
core/src/compat/
├── mod.rs          # 兼容层统一入口
├── sdp_quirks.rs   # SDP 非标解析（缺失采样率、异常 fmtp）
├── url_quirks.rs   # URL 后缀剥离、路径规范化
├── transport_quirks.rs  # Transport 头非标格式
└── seq_quirks.rs   # RTP seq 重置检测
```

所有兼容逻辑显式命名，不散落在主路径代码中。

## 依赖关系

```
cheetah-rtsp-core
  └── (无新外部依赖，sha2 crate 用于 SHA-256)

cheetah-rtsp-driver-tokio
  └── tokio-rustls (TLS)
  └── rustls-pemfile (证书加载)

cheetah-rtsp-module
  └── cheetah-codec (MuteAudioMaker, 新增)

cheetah-codec
  └── 新增 MuteAudioMaker (AAC silent frame generator)
```

## 配置模型演进

```yaml
modules:
  rtsp:
    enabled: true
    listen: 0.0.0.0:554
    # --- Phase-01 新增 ---
    tls:
      enabled: false
      listen: 0.0.0.0:322     # RTSPS 默认端口
      cert_path: ""
      key_path: ""
    auth:
      digest_algorithms: [md5, sha-256]  # 新增 SHA-256
      nonce_ttl_secs: 300
      nonce_replay_window: 32
    # --- Phase-02 新增 ---
    enable_mute_audio: true
    reorder_buffer:
      enabled: true
      max_packets: 64
      timeout_ms: 100
    rtcp_feedback:
      enable_nack: true
      enable_pli: true
      nack_buffer_size: 512
    # --- Phase-03 新增 ---
    compat:
      strip_sdp_suffix: true
      heartbeat_mode: auto        # auto | rtcp | get_parameter | both
      continue_push_ms: 10000
      default_video_clock_rate: 90000
    # --- Phase-04 新增 ---
    enable_direct_proxy: true
    udp:
      nat_probe_timeout_ms: 5000
      port_range: [30000, 35000]
      randomize_ports: true
```

## 测试策略

| 层 | 新增测试类型 | 覆盖目标 |
|----|-------------|----------|
| core | 单元测试 | SHA-256 digest 计算、RTCP-FB 编解码、seq 回绕检测、compat 解析 |
| core | 属性测试 | 任意 seq 序列的排序正确性、任意 SDP 的 quirks 容错 |
| core | fuzz | RTCP-FB 包解析、非标 SDP、非标 Transport 头 |
| driver | 集成测试 | TLS 握手、重排序缓冲区超时释放、NACK 重传 |
| module | 端到端测试 | RTSPS 推拉流、静音音频注入验证、Direct Proxy 转发 |
| module | 互操作测试 | FFmpeg/VLC/GStreamer 对接 RTSPS、断连续推恢复 |
