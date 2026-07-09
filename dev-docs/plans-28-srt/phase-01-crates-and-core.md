# Phase 01 — Crate 脚手架与 Core 边界

- **状态**: 未开始
- **范围**: 新增 SRT 三段式 crate、workspace/server 集成、core API、URL 和 Stream ID 解析、基础配置模型
- **完成标准**: `cheetah-srt-core` 可独立测试；server 可通过 feature 注册 `SrtModuleFactory`；尚不要求真实网络收发

---

## 1.1 新增 crate 目录

新增目录：

```text
crates/protocols/srt/
├── core/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── error.rs
│       ├── stream_id.rs
│       ├── url.rs
│       ├── config.rs
│       └── session.rs
├── driver-tokio/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── config.rs
│       ├── driver.rs
│       ├── listener.rs
│       ├── caller.rs
│       ├── connection.rs
│       └── stats.rs
├── module/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── config.rs
│       ├── module.rs
│       ├── ingest.rs
│       ├── egress.rs
│       ├── jobs.rs
│       └── stream_id.rs
└── testing/
    └── property-tests/
        ├── Cargo.toml
        ├── src/lib.rs
        └── tests/prop_srt_core.rs
```

Cargo package names：

- `cheetah-srt-core`
- `cheetah-srt-driver-tokio`
- `cheetah-srt-module`
- `cheetah-srt-property-tests`

---

## 1.2 Workspace 与 server 集成

根 `Cargo.toml` 增加 workspace members：

```toml
"crates/protocols/srt/core",
"crates/protocols/srt/driver-tokio",
"crates/protocols/srt/module",
"crates/protocols/srt/testing/property-tests",
```

根 workspace dependencies 增加：

```toml
shiguredo_srt = "=2026.1.0-canary.1"
```

`apps/cheetah-server/Cargo.toml` 增加 feature 和依赖：

```toml
[features]
srt = ["dep:cheetah-srt-module"]

[dependencies]
cheetah-srt-module = { path = "../../crates/protocols/srt/module", optional = true }
```

`apps/cheetah-server/src/main.rs` 增加注册：

```rust
#[cfg(feature = "srt")]
use cheetah_srt_module::SrtModuleFactory;

#[cfg(feature = "srt")]
{
    builder = builder.register_module_factory(Arc::new(SrtModuleFactory));
}
```

---

## 1.3 Core 公共 API

`cheetah-srt-core/src/lib.rs` 导出：

```rust
pub mod config;
pub mod error;
pub mod session;
pub mod stream_id;
pub mod url;

pub use config::{
    SrtEncryptionOptions, SrtPayloadKind, SrtRole, SrtSessionOptions, SrtStreamMode,
};
pub use error::{SrtCoreError, SrtCoreResult};
pub use session::{
    SrtCoreCommand, SrtCoreEvent, SrtCoreInput, SrtCoreOutput, SrtSessionId,
};
pub use stream_id::{ParsedSrtStreamId, parse_srt_stream_id};
pub use url::{ParsedSrtUrl, parse_srt_url};
```

建议类型：

```rust
pub enum SrtRole {
    Listener,
    Caller,
}

pub enum SrtStreamMode {
    Publish,
    Request,
    Play,
}

pub enum SrtPayloadKind {
    MpegTs,
}

pub struct SrtSessionOptions {
    pub role: SrtRole,
    pub mode: SrtStreamMode,
    pub stream_key: String,
    pub latency_ms: u64,
    pub payload: SrtPayloadKind,
    pub encryption: SrtEncryptionOptions,
}
```

Core 的 session API 只包裹 `shiguredo_srt` 的输入/输出，不隐藏 timer：

```rust
pub enum SrtCoreInput {
    Packet { now_micros: u64, bytes: bytes::Bytes },
    SendPayload { now_micros: u64, payload: bytes::Bytes },
    Timer { now_micros: u64 },
    Close { now_micros: u64, reason: String },
}

pub enum SrtCoreOutput {
    SendPacket { bytes: bytes::Bytes },
    SetTimer { at_micros: u64 },
    ClearTimer,
    Event(SrtCoreEvent),
}

pub enum SrtCoreEvent {
    Connected,
    PayloadReceived { payload: bytes::Bytes },
    KeyRefreshNeeded,
    Disconnected { reason: String },
    Stats { snapshot: SrtStatsSnapshot },
}
```

---

## 1.4 Stream ID 解析

实现 `parse_srt_stream_id(input: &str) -> Result<ParsedSrtStreamId, SrtCoreError>`。

必须支持：

```text
#!::r=live/test,m=publish,u=alice
#!::r=live/test,m=request
live/test
/live/test
```

解析规则：

- 如果以 `#!::` 开头，按逗号分隔 key/value。
- key/value 中允许 URL percent-decoding。
- `r` 映射为 `stream_key`。
- `m=publish` 映射摄入；`m=request` 或 `m=play` 映射输出。
- bare string 映射为 `stream_key`，mode 由调用方默认值决定。

拒绝规则：

- 空 stream key。
- 包含 `..`。
- 包含 ASCII control 字符。
- 包含连续 `/`。
- 未知 `m`。

单元测试：

- `access_control_publish_stream_id_parses`
- `access_control_request_stream_id_parses`
- `bare_stream_key_parses`
- `percent_encoded_resource_parses`
- `empty_resource_is_rejected`
- `path_traversal_is_rejected`
- `unknown_mode_is_rejected`

---

## 1.5 SRT URL 解析

实现 `parse_srt_url(input: &str) -> Result<ParsedSrtUrl, SrtCoreError>`。

示例：

```text
srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/test,m=publish
srt://:9000?mode=listener
srt://example.com:9000?mode=caller&latency=120&passphrase=secret&pbkeylen=16
```

字段：

- `host`
- `port`
- `mode`
- `streamid`
- `latency_ms`
- `passphrase`
- `pbkeylen`

兼容 query：

- `mode=caller|listener`
- `streamid=` / `streamId=`
- `latency=`，单位毫秒
- `passphrase=`
- `pbkeylen=16|24|32`，v1 只接受库支持的 AES-128/256；不支持值返回配置错误

---

## 1.6 Module 配置类型

`cheetah-srt-module/src/config.rs` 定义：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SrtModuleConfig {
    pub enabled: bool,
    pub listen: String,
    pub max_connections: usize,
    pub idle_timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub latency_ms: u64,
    pub payload: SrtPayloadModuleConfig,
    pub encryption: SrtEncryptionModuleConfig,
    pub auth: SrtAuthConfig,
    pub ingress: SrtIngressConfig,
    pub egress: SrtEgressConfig,
    pub ingress_jobs: Vec<SrtIngressJobConfig>,
    pub egress_jobs: Vec<SrtEgressJobConfig>,
    pub relay_jobs: Vec<SrtRelayJobConfig>,
}
```

默认配置：

```yaml
modules:
  srt:
    enabled: true
    listen: "0.0.0.0:9000"
    max_connections: 1024
    idle_timeout_ms: 30000
    connect_timeout_ms: 5000
    latency_ms: 120
    payload:
      kind: "mpegts"
    encryption:
      enabled: false
      passphrase: ""
      key_length: 16
    ingress:
      default_mode: "publish"
      default_publish_stream_key: ""
      publish_keepalive_ms: 0
    egress:
      subscriber_queue_capacity: 256
      start_from_keyframe: true
```

---

## 验证方法

```bash
cargo fmt
cargo test -p cheetah-srt-core
cargo test -p cheetah-srt-property-tests
cargo clippy -p cheetah-srt-core
```

server feature 验证：

```bash
cargo check -p cheetah-server --features srt
```
