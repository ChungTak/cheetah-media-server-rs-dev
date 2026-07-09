# RTMP 协议完善 — 架构扩展设计

- **状态**: 未开始
- **范围**: 新增能力的 crate 归属、接口边界、数据流路径
- **完成标准**: 所有新增能力有明确的 crate 归属和接口定义

---

## 1. 整体架构视图

```
┌─────────────────────────────────────────────────────────────────┐
│                        cheetah-server (app)                       │
├─────────────────────────────────────────────────────────────────┤
│  cheetah-rtmp-module    cheetah-http-flv-module   cheetah-record-module  │
├─────────────────────────────────────────────────────────────────┤
│  cheetah-rtmp-driver-tokio   cheetah-http-flv-driver-tokio       │
├─────────────────────────────────────────────────────────────────┤
│  cheetah-rtmp-core           cheetah-http-flv-core               │
├─────────────────────────────────────────────────────────────────┤
│                     cheetah-codec (Foundation)                    │
├─────────────────────────────────────────────────────────────────┤
│  cheetah-sdk    cheetah-engine    cheetah-runtime-api            │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. 新增编解码归属

### 2.1 cheetah-codec 扩展

新增编解码统一在 `cheetah-codec` 中实现：

```
cheetah-codec/src/
├── audio/
│   ├── aac.rs          (已有)
│   ├── opus.rs         (已有)
│   ├── mp3.rs          (已有)
│   ├── g711.rs         (新增: A-law + μ-law 参数解析)
│   └── mod.rs
├── video/
│   ├── h264.rs         (已有)
│   ├── h265.rs         (已有)
│   ├── av1.rs          (已有)
│   ├── vp8.rs          (新增: VP8 参数解析)
│   ├── vp9.rs          (新增: VP9 参数解析)
│   └── mod.rs
└── codec_id.rs         (扩展: 新增 G711Alaw, G711Ulaw, VP8, VP9)
```

### 2.2 编解码能力分级

| 编解码 | 能力级别 | 说明 |
|--------|----------|------|
| H.264/H.265/AV1 | 完整支持 | 参数集解析、NALU 处理、转协议 |
| AAC/Opus/MP3 | 完整支持 | 配置头解析、转协议 |
| G.711/VP8/VP9 | 完整支持 | 参数解析、转协议 |
| 其他编码 | 透传转发 | 不解析、不转协议、仅 RTMP→RTMP 转发 |

---

## 3. 国内扩展兼容层

### 3.1 设计原则

- 国内扩展 codec ID 作为**入口兼容层**，解析后统一转为内部 `CodecId` 枚举
- 出口默认使用 Enhanced RTMP FourCC，可通过配置回退到国内扩展 ID
- 兼容逻辑集中在 `cheetah-rtmp-core` 的 `media.rs` 中

### 3.2 Codec ID 映射表

```rust
// 国内扩展 → 内部统一
const DOMESTIC_VIDEO_H265: u8 = 12;
const DOMESTIC_VIDEO_AV1: u8 = 13;
const DOMESTIC_VIDEO_VP8: u8 = 14;
const DOMESTIC_VIDEO_VP9: u8 = 15;
const DOMESTIC_AUDIO_OPUS: u8 = 13;

// 出口模式枚举
pub enum RtmpCodecMode {
    Enhanced,       // 默认: FourCC
    Domestic,       // 国内扩展 ID
    Auto,           // 根据对端能力自动选择
}
```

---

## 4. 录制模块架构

### 4.1 crate 归属

录制作为独立模块 `cheetah-record-module`，不嵌入 RTMP module：

```
crates/system/record/
├── Cargo.toml          (package: cheetah-record-module)
└── src/
    ├── lib.rs
    ├── module.rs       (Module trait 实现)
    ├── config.rs       (录制配置)
    ├── flv_writer.rs   (FLV 文件写入)
    ├── lifecycle.rs    (录制生命周期: 开始/停止/分片)
    └── api.rs          (HTTP API: 开始/停止/查询录制)
```

### 4.2 数据流

```
Engine StreamManager
       │
       ▼ (subscribe)
cheetah-record-module
       │
       ▼ (AVFrame → FLV tags)
  flv_writer.rs
       │
       ▼ (write)
  File System
```

### 4.3 设计要点

- 录制模块通过 `EngineContext` 订阅流，与 RTMP module 解耦
- FLV tag 生成复用 `cheetah-rtmp-core` 的 `flv.rs` 能力
- 支持按时长/大小自动分片
- 文件 I/O 使用 `spawn_blocking` 避免阻塞事件循环

---

## 5. 断连续推架构

### 5.1 归属

断连续推逻辑在 `cheetah-rtmp-module` 中实现，通过引擎的发布租约模型扩展：

```
Publisher disconnect
       │
       ▼
Module 启动保活定时器 (configurable: publish_keepalive_ms)
       │
       ├── 超时前同一 StreamKey 重新 publish → 恢复，不中断订阅者
       │
       └── 超时 → 正常释放流、通知订阅者 EOF
```

### 5.2 设计要点

- 保活期间流状态保持 `Active`，订阅者不收到 EOF
- 保活期间 GOP 缓存保留，新订阅者仍可获取 bootstrap
- 配置项 `publish_keepalive_ms`，默认 0（禁用）

---

## 6. Paced Sender 架构

### 6.1 归属

Paced Sender 在 `cheetah-rtmp-driver-tokio` 中实现：

```
egress pipeline (module)
       │
       ▼ (AVFrame batch)
driver send loop
       │
       ▼ (pacing timer)
TCP write
```

### 6.2 设计要点

- 配置项 `paced_sender_ms`，默认 0（禁用）
- 启用后按固定间隔匀速发送，避免突发流量导致客户端缓冲区溢出
- 实现为 driver 层的 `tokio::time::interval` 节流

---

## 7. 直接代理模式架构

### 7.1 归属

直接代理在 `cheetah-rtmp-module` 中实现：

```
RTMP Publish (raw chunks)
       │
       ▼ (bypass demux)
Module: 标记为 direct_proxy 流
       │
       ▼ (raw RTMP packets stored in ring buffer)
RTMP Play (raw chunks)
```

### 7.2 设计要点

- 配置项 `direct_proxy: bool`，默认 false
- 启用后跳过 FLV demux → AVFrame → FLV mux 路径
- 直接代理流**不支持**跨协议转发（仅 RTMP→RTMP）
- 直接代理流**不支持**录制、转码、时间戳修正
- 降低 CPU 开销，适用于纯转发场景

---

## 8. HTTP-FLV 增强架构

### 8.1 HTTPS-FLV / WSS-FLV

在 `cheetah-http-flv-driver-tokio` 中扩展 TLS 支持：

```
cheetah-http-flv-driver-tokio/src/
├── server.rs           (已有: TCP listener)
├── tls.rs              (新增: TLS acceptor, 复用 rustls)
└── lib.rs              (新增: start_tls_server)
```

配置模型扩展：

```yaml
modules:
  http_flv:
    listen: 0.0.0.0:8080
    tls:
      enabled: true
      listen: 0.0.0.0:8443
      cert_path: /path/to/cert.pem
      key_path: /path/to/key.pem
```

### 8.2 HTTP-FLV Push（POST 推流）

在 `cheetah-http-flv-core` 中扩展 POST 请求解析：

```
HTTP POST /app/stream.flv
       │
       ▼ (chunked body)
FlvSplitter (demux FLV tags)
       │
       ▼ (RtmpPacket → AVFrame)
Engine publish
```

- 复用已有的 `flv_ingest.rs` 解析逻辑
- 在 driver 层增加 POST 路由识别
- 在 module 层增加 publish 管线

---

## 9. 播放控制架构

### 9.1 服务端 Seek/Pause/Speed

在 `cheetah-rtmp-core` 中扩展命令解析：

```rust
// 新增 CoreInput 变体
pub enum CoreInput {
    // ... 已有
    SeekCommand { stream_id: u32, millis: f64 },
    PauseCommand { stream_id: u32, pause: bool, millis: f64 },
    SpeedCommand { stream_id: u32, speed: f64 },
}

// 新增 CoreOutput 变体
pub enum CoreOutput {
    // ... 已有
    SeekRequested { stream_id: u32, millis: f64 },
    PauseRequested { stream_id: u32, pause: bool },
    SpeedRequested { stream_id: u32, speed: f64 },
}
```

### 9.2 客户端 Seek/Pause/Speed

在 `cheetah-rtmp-core` 客户端状态机中扩展：

```rust
pub enum ClientCommand {
    // ... 已有
    Seek { millis: f64 },
    Pause { pause: bool, millis: f64 },
    Speed { speed: f64 },
}
```

---

## 10. 未知编码透传

### 10.1 设计原则

- 对于不在支持列表中的 codec ID，不尝试解析参数集
- 以 `CodecId::Unknown(u8)` 或 `CodecId::UnknownFourCC([u8; 4])` 表示
- 透传流只能在相同协议间转发（RTMP→RTMP、FLV→FLV）
- 不支持跨协议转换（因为无法生成目标协议所需的参数集）

### 10.2 实现位置

- `cheetah-codec`: 扩展 `CodecId` 枚举增加 Unknown 变体
- `cheetah-rtmp-core`: media 解析时遇到未知 ID 生成 Unknown 帧
- `cheetah-rtmp-module`: Unknown 帧直接转发，不经过 codec 处理管线
