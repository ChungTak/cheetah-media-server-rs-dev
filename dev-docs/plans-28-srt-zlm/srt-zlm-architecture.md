# SRT ZLM 兼容 — 目标架构（实现规格）

智能体实现时以本文 + [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md) 为准。

---

## 1. 三段式边界（不可破坏）

| Crate | 路径 | 允许 | 禁止 |
|-------|------|------|------|
| `cheetah-srt-core` | `crates/protocols/srt/core` | 纯解析、配置类型、版本编解码、FEC 纯函数、错误类型 | Tokio、socket、`Instant::now`、engine、HTTP |
| `cheetah-srt-driver-tokio` | `crates/protocols/srt/driver-tokio` | UDP、timer、`shiguredo_srt::SrtConnection`、stats、连接表 | 业务 stream 租约、TS demux |
| `cheetah-srt-module` | `crates/protocols/srt/module` | classify、auth、publish/play、TS bridge、jobs、metrics | `tokio::net/time/sync` 公共暴露；自研 NACK |

协议状态机：`shiguredo_srt`。  
媒体：`cheetah_codec::{MpegTsDemuxer, MpegTsMuxer}` + engine `PublisherSink` / `SubscriberSource`。

依赖方向：

```text
module → driver-tokio → core → shiguredo_srt
module → cheetah-codec
module → cheetah-sdk
```

---

## 2. 数据流（必须保持）

### 2.1 Listener 推流

```text
Caller (OBS/ffmpeg)
  → UDP → driver (SrtConnection)
  → Connected { stream_id }
  → module classify+auth
  → acquire_publisher(StreamKey)
  → Payload → MpegTsDemuxer
  → TrackInfo + AVFrame → PublisherSink
```

### 2.2 Listener 拉流

```text
Caller (ffplay/VLC)
  → Connected { stream_id }
  → classify+auth → StreamKey
  → subscribe engine
  → MpegTsMuxer → SendPayload → driver → UDP
```

### 2.3 与「TS 直通 ring」的差异

兼容规范允许拉流直接吐 TS 包。Cheetah **必须** 走 engine 统一帧，以便 RTMP/RTSP/HLS/WebRTC 互转。对客户端仍是 MPEG-TS over SRT，行为兼容。

---

## 3. Stream ID 与 StreamKey 目标模型

### 3.1 建议类型（Phase 01 落地）

在 `cheetah-srt-core` 扩展（可替换现有 `ParsedSrtStreamId` 字段，保持函数名 `parse_srt_stream_id` 或新增 `parse_zlm_stream_id` 再封装）：

```rust
// 目标形状（名称可微调，语义不可变）
pub struct ParsedSrtStreamId {
    pub vhost: String,
    pub app: String,
    pub stream: String,
    /// app/stream 规范化后的资源串
    pub resource: String,
    pub mode: Option<SrtStreamMode>, // None = 未声明 m
    pub user: Option<String>,
    pub session: Option<String>,
    /// 除 h、r 外全部 key（必须含 m，若存在）
    pub auth_params: BTreeMap<String, String>,
}

pub struct StreamIdParseOptions {
    pub default_vhost: String,
    pub strict_prefix: bool,     // true: 必须 #!::
    pub strict_resource: bool,   // true: r 必须两段
    pub allow_bare_key: bool,    // true: 允许无前缀兼容旧客户端
}
```

解析算法：**完整复制** [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md) §2.3。

### 3.2 StreamKey 映射

`cheetah_sdk::StreamKey { namespace, path }` 无独立 vhost 维。

```rust
// stream_key_vhost_mode = "app_only" （默认）
StreamKey::new(app, stream)

// stream_key_vhost_mode = "vhost_prefix"
StreamKey::new(format!("{vhost}/{app}"), stream)  // 或规范化分隔
```

`vhost` 始终进入：

- 日志字段  
- metrics label（若导出）  
- `SrtAuthContext`  

### 3.3 模式决议

```text
if parsed.mode == Some(Publish) → Publish
else if parsed.mode == Some(Request|Play) → 该 mode
else → config.ingress.default_mode   // 新默认 "request"
```

`Request` 与 `Play` 在业务上均走 **拉流** 路径（与现 module 一致）。

---

## 4. 鉴权目标模型

```rust
pub struct SrtAuthContext {
    pub mode: SrtStreamMode,
    pub vhost: String,
    pub app: String,
    pub stream: String,
    pub stream_key: StreamKey,
    pub user: Option<String>,
    pub auth_params: BTreeMap<String, String>,
    pub peer_addr: Option<SocketAddr>,
}
```

逻辑（`auth.enabled == true` 时）：

1. `token = auth_params.get("token")`  
2. 全局：`publish_token` / `request_token` 与 token 比较  
3. 用户表：`user`（字段 `u`）+ token 匹配 `auth.users[]`  
4. 失败 → `Err` → driver `Close`  
5. 可选后续：`webhook_enabled` 时把整个 `auth_params` 交给 control（Phase 01 可先定义 hook trait 或 `fn build_webhook_query(&SrtAuthContext) -> String`，HTTP 可 TODO）

---

## 5. 配置目标模型

在 `SrtModuleConfig` 上 **增量** 字段（serde default 兼容旧 JSON）：

```rust
// 新增建议字段
pub default_vhost: String,                    // "__defaultVhost__"
pub min_peer_srt_version: String,             // "1.3.0"
pub local_srt_version: String,                // "1.5.0"
pub require_peer_version_extension: bool,     // false
pub latency_mul: u32,                         // 4
pub pkt_buf_size: usize,                      // 8192
pub stream_id: SrtStreamIdModuleConfig,
pub fec: SrtFecModuleConfig,

// 修改默认
// ingress.default_mode: "request"   // 原 "publish"
```

```rust
pub struct SrtStreamIdModuleConfig {
    pub strict_prefix: bool,        // default true
    pub strict_resource: bool,      // default true
    pub allow_bare_key: bool,       // default false
    pub stream_key_vhost_mode: String, // "app_only" | "vhost_prefix"
}

pub struct SrtFecModuleConfig {
    pub enabled: bool,
    pub required: bool,
    pub cols: u32,
    pub rows: u32,
}
```

`SrtDriverConfig` 同步接收：`latency_ms`、`pkt_buf_size`/`recv_buffer_packets`、`min_peer_srt_version`、`local_srt_version`、fec 相关。

配置变更导致 listen/encrypt/fec 主开关变化 → `ConfigEffect::ModuleRestartRequired`（现有逻辑：任意 diff 即 restart，可保持）。

---

## 6. Driver 事件/命令（已有，扩展点）

**已有**（`driver.rs`）：

```text
Command: ConnectCaller, SendPayload, Close
Event: ListenerStarted, CallerConnecting, Connected{stream_id},
       Payload, KeyRefreshNeeded, Stats, Disconnected, Error
```

**建议增量**：

```text
Event::Rejected { peer_id, reason: SrtRejectReason }
// 或 Disconnected.reason 使用稳定前缀: "reject:peer_version_too_old"

Stats 增量字段（库可得才加）:
  nak_sent, nak_received, tlpktdrop_count, peer_srt_version
```

`ConnectionOptions` 构建处（`connection_options` / `caller_connection_options`）透传：

- `tsbpd_delay` ← `latency_ms`（已有）  
- `srt_version` ← 配置（若 `shiguredo_srt::ConnectionOptions` 支持；当前库默认 `0x010500`）  
- stream_id（已有）  

---

## 7. Module 文件拆分目标

当前 `module/src/module.rs` ~1400 行。目标：

```text
module/src/
  lib.rs
  config.rs
  metrics.rs
  http.rs
  module.rs           # Module trait + start 拼装 only
  stream_classify.rs  # parse options + classify → (mode, StreamKey, AuthContext)
  auth.rs
  ingress_session.rs  # demux + publish
  egress_session.rs   # play session
  jobs.rs             # job plan + retry
```

`AGENTS.md`：单文件尽量 <500 行，明显 >800 必须拆。

---

## 8. 版本工具（core 纯函数）

```rust
pub fn parse_srt_version(s: &str) -> Result<u32, ...>; // "1.3.0" → 0x00010300
pub fn format_srt_version(v: u32) -> String;
pub fn version_at_least(peer: u32, min: u32) -> bool;
```

Driver 在握手完成后（库若暴露 peer HS 扩展）比较；否则 Phase 02 记录「库 API 不可用」并至少完成配置+纯函数+单测。

---

## 9. FEC 架构（Phase 04）

```text
core: FecConfig validate + XOR recover 纯函数
driver: 协商 + 收发路径集成（依赖库扩展策略）
module: 配置 + metrics srt_fec_*
```

降级：见参考规范 §6。

---

## 10. 观测

Module HTTP（已有）：

- `GET /srt/metrics`  
- `GET /srt/metrics.json`  

必须能导出：连接数、bytes/packets、retransmit、lost、rtt/jitter、reject 计数、fec 计数（Phase 04）。

日志：`peer_id, remote, stream_id, vhost, app, stream, mode, reason`。

---

## 11. 测试分层

| 层 | 位置 | 内容 |
|----|------|------|
| 单元 | `core/tests/parser.rs` | streamid 全表 |
| 单元 | core version/fec | 纯函数 |
| property | `testing/property-tests` | 字段序、编码 |
| driver | `driver-tokio/tests` | 握手、stats、timeout、版本 |
| module | `module/tests` 或 src 内测 | classify、auth、publish/play |
| fuzz | `srt/fuzz` | stream_id/url/packet |
| 外部 | Phase 05 | ffmpeg/ffplay/OBS/VLC |

---

## 12. 实现检查清单（架构合规）

- [ ] core 无 Tokio  
- [ ] 默认无 m 为拉流  
- [ ] r 两段 + h vhost  
- [ ] auth_params 含 m  
- [ ] TS only  
- [ ] 单发布者租约  
- [ ] 媒体经 cheetah-codec  
- [ ] 缓冲均有上界  
- [ ] 无 vendor-ref 依赖  
