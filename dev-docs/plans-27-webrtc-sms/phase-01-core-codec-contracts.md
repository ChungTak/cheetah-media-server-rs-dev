# Phase 01 — WebRTC core 与 codec contract

- **状态**: 已完成
- **完成位置**: `crates/protocols/webrtc/core/`、`crates/foundation/cheetah-codec/{src/adapter.rs,tests/future_protocol_adapter_contract.rs}`
- **范围**: 建立 `cheetah-webrtc-core` crate、引入 `str0m 0.19.0`、固定 Sans-I/O 输入输出模型、补齐 `cheetah-codec` WebRTC ingress/egress contract
- **完成标准**: core 可独立完成 offer/answer smoke test、timer/input/output 驱动、基本 media/datachannel event 转换；codec contract 能验证 WebRTC 入站归一化和出站 access-unit/参数集要求
- **落地清单**:
  - 新增 workspace 成员 `cheetah-webrtc-core`、`cheetah-webrtc-driver-tokio`、`cheetah-webrtc-module`、`cheetah-webrtc-property-tests`，并在 `Cargo.toml` 注册 `str0m = { version = "0.19.0", default-features = false, features = ["rust-crypto"] }` 共享版本。
  - `cheetah-webrtc-core` 提供 `WebRtcCore` 多会话状态机封装，`u64 now_micros` 边界类型，单调时间 clamp、bounded 输入/输出队列，never 调用 `Instant::now()` 或系统时间 API（仅在 `#[cfg(test)]` 测试用例内使用）。
  - 输入：`AcceptOffer`/`ApplyAnswer`/`AddRemoteCandidate`/`SendDataChannel`/`RequestKeyframe`/`Close` 命令、`Network`/`Timeout`/`Tick` 信号；输出：`SendPacket`/`SetTimer`/`Event`/`Diagnostic`/`LocalDescription`/`CloseSession`。
  - 事件层翻译 `Connected`/`IceConnectionStateChange`/`MediaAdded`(含 simulcast send/recv RID)/`MediaData`/`PeerStats`/`MediaIngressStats`/`MediaEgressStats`/`EgressBitrateEstimate`/`KeyframeRequest`/DataChannel open/data/close。
  - SDP 兼容预处理 `sdp_compat::preprocess_remote_sdp` 完成行尾归一、行末空白裁剪、CRLF 终结符补齐；diagnostic 上报。
  - codec 层新增 `WebRtcIngressContractView`，并在 `cheetah_codec::lib` 重新导出；既有的 ingress 归一化 / egress access-unit / 参数集补发测试保持通过。
  - 单元测试覆盖 SDP 预处理 4 项、core lifecycle/SDP error/close/单调时间 6 项；属性测试 `cheetah-webrtc-property-tests` 提供 SDP 预处理幂等性、不 panic、CRLF 终结三项 proptest。
  - 引用 SMS fixture（`vendor-ref/simple-media-server/Src/Webrtc/SdpExample/publish-offer-sms.sdp`）作为 core/driver/module 测试用例的最小 offer。

## 1.1 Workspace 与依赖

新增 workspace members：

```toml
"crates/protocols/webrtc/core",
"crates/protocols/webrtc/driver-tokio",
"crates/protocols/webrtc/module",
"crates/protocols/webrtc/testing/property-tests",
```

Phase 01 只要求 `core` 和 testing skeleton 可编译，driver/module 可以先建空 crate 或推迟到后续 phase。

`crates/protocols/webrtc/core/Cargo.toml`：

```toml
[package]
name = "cheetah-webrtc-core"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
bytes.workspace = true
cheetah-codec = { path = "../../../foundation/cheetah-codec" }
serde = { workspace = true, features = ["derive"] }
str0m = { version = "0.19.0", default-features = false, features = ["rust-crypto"] }
thiserror.workspace = true
tracing.workspace = true
```

策略：

- 默认使用 `rust-crypto`，避免系统 OpenSSL 依赖。
- release profile 可在后续增加 `aws-lc-rs` feature。
- 不启用会引入平台耦合的 feature，除非后续实测需要。

## 1.2 Core public API

新增模块建议：

```text
crates/protocols/webrtc/core/src/
  lib.rs
  config.rs
  error.rs
  event.rs
  input.rs
  output.rs
  session.rs
  sdp_compat.rs
  stats.rs
  types.rs
```

`lib.rs` 只 re-export 稳定边界类型：

```rust
pub mod config;
pub mod error;
pub mod event;
pub mod input;
pub mod output;
pub mod session;
pub mod sdp_compat;
pub mod stats;
pub mod types;

pub use config::{WebRtcCoreConfig, WebRtcCoreLimits};
pub use error::{WebRtcCoreDiagnostic, WebRtcCoreError};
pub use event::{WebRtcCoreEvent, WebRtcMediaEvent, WebRtcRtcpFeedback};
pub use input::{WebRtcCoreCommand, WebRtcCoreInput, WebRtcNetworkInput};
pub use output::{WebRtcCoreOutput, WebRtcPacketOut, WebRtcTimer};
pub use session::WebRtcCore;
pub use stats::{WebRtcBweStats, WebRtcSessionStats};
pub use types::{
    DataChannelId, WebRtcCodecProfile, WebRtcDirection, WebRtcIceRole, WebRtcSessionId,
    WebRtcSessionRole, WebRtcSessionState,
};
```

核心设计：

- `WebRtcCore` 可以管理多个 session，也可以在 driver shard 中每个 worker 管理一组 session。
- `WebRtcCoreSession` 封装 `str0m::Rtc`，但不把 `Rtc` 暴露到 module/driver 之外。
- 所有用户操作、网络输入、timer 都通过 `WebRtcCoreInput` 进入。
- 所有网络输出、timer、事件、诊断都通过 `WebRtcCoreOutput` 返回。

## 1.3 Core 输入输出模型

输入：

```rust
pub enum WebRtcCoreInput {
    Command(WebRtcCoreCommand),
    Network(WebRtcNetworkInput),
    Timeout {
        session_id: WebRtcSessionId,
        now_micros: u64,
    },
    Tick {
        now_micros: u64,
    },
}

pub enum WebRtcCoreCommand {
    AcceptOffer {
        session_id: WebRtcSessionId,
        role: WebRtcSessionRole,
        remote_sdp: String,
        now_micros: u64,
    },
    CreateOffer {
        session_id: WebRtcSessionId,
        role: WebRtcSessionRole,
        now_micros: u64,
    },
    ApplyAnswer {
        session_id: WebRtcSessionId,
        remote_sdp: String,
        now_micros: u64,
    },
    AddRemoteCandidate {
        session_id: WebRtcSessionId,
        candidate: String,
        now_micros: u64,
    },
    SendFrame(Box<WebRtcFrameOut>),
    SendRtp(Box<WebRtcRtpOut>),
    SendDataChannel(WebRtcDataChannelOut),
    RequestKeyframe {
        session_id: WebRtcSessionId,
        track_id: TrackId,
        now_micros: u64,
    },
    Close {
        session_id: WebRtcSessionId,
        reason: WebRtcCloseReason,
    },
}

pub struct WebRtcNetworkInput {
    pub session_id: WebRtcSessionId,
    pub source: SocketAddr,
    pub destination: SocketAddr,
    pub data: Bytes,
    pub now_micros: u64,
}
```

输出：

```rust
pub enum WebRtcCoreOutput {
    SendPacket(WebRtcPacketOut),
    SetTimer(WebRtcTimer),
    CancelTimer { session_id: WebRtcSessionId },
    Event(WebRtcCoreEvent),
    Diagnostic(WebRtcCoreDiagnostic),
    CloseSession {
        session_id: WebRtcSessionId,
        reason: WebRtcCloseReason,
    },
}

pub struct WebRtcPacketOut {
    pub session_id: WebRtcSessionId,
    pub destination: SocketAddr,
    pub data: Bytes,
}

pub struct WebRtcTimer {
    pub session_id: WebRtcSessionId,
    pub deadline_micros: u64,
}
```

注意：

- core 层使用 `u64 now_micros` 作为边界类型，内部再转换为 `Instant` adapter；adapter 只能基于 driver 注入的起始时间，不得调用系统当前时间。
- `SocketAddr` 可作为纯数据类型留在 core input/output，但 core 不创建 socket。
- 如果后续需要 `no_std`，再把地址类型替换为内部 endpoint newtype。

## 1.4 str0m 配置策略

`WebRtcCoreConfig`：

```rust
pub struct WebRtcCoreConfig {
    pub ice_lite: bool,
    pub codec_profile: WebRtcCodecProfile,
    pub enable_bwe: bool,
    pub bwe_initial_bitrate_bps: Option<u64>,
    pub enable_simulcast: bool,
    pub rtx_cache_packets: usize,
    pub rtx_cache_age_ms: u64,
    pub rtx_ratio_cap: Option<f32>,
    pub video_reorder_packets: usize,
    pub audio_reorder_packets: usize,
    pub enable_rtp_mode: bool,
}
```

映射到 `str0m::RtcConfig`：

- `ice_lite=false` 为默认，支持 ICE full。
- browser profile 默认启用 Opus、H264、VP8、VP9、AV1。
- device profile 额外启用 H265、PCMA、PCMU。
- passthrough profile 可以启用 RTP mode，但必须显式配置。
- `enable_bwe=true` 时调用 BWE 配置并打开 TWCC extension。
- `video_reorder_packets` / `audio_reorder_packets` 映射到 reorder size。
- `rtx_cache_*` 应用于 send streams，或作为默认发送 buffer 配置。

## 1.5 codec contract 扩展

`cheetah-codec` 已有 future protocol WebRTC contract 测试，Phase 01 需要把这些 contract 明确化并补齐接口。

必须测试：

1. WebRTC ingress 如果绕过 timestamp normalization，返回 `WebRtcBypassedMediaNormalization`。
2. WebRTC egress 视频没有 access-unit boundary，返回 `WebRtcVideoMissingAccessUnitBoundary`。
3. H264/H265/H266 keyframe 缺少必要参数集时返回 `MissingRequiredParameterSets`。
4. WebRTC egress 使用 canonical timeline 导出的 RTP timestamp，而不是其他协议 source timestamp。
5. Opus 使用 48 kHz clock rate，视频默认 90 kHz clock rate。

建议新增：

```rust
pub struct WebRtcIngressContractView {
    pub track_id: TrackId,
    pub codec: CodecId,
    pub rtp_timestamp_ticks: u32,
    pub sequence_number: u16,
    pub marker: bool,
    pub rid: Option<SmallString>,
    pub repaired_rid: Option<SmallString>,
    pub twcc_sequence: Option<u16>,
}
```

如果不引入 `SmallString`，使用 `Option<String>`，后续性能优化再替换。

## 1.6 SDP fixture 测试

使用 SMS fixtures：

```text
vendor-ref/simple-media-server/Src/Webrtc/SdpExample/offer.sdp
vendor-ref/simple-media-server/Src/Webrtc/SdpExample/offer-answer.sdp
vendor-ref/simple-media-server/Src/Webrtc/SdpExample/offer-simulcast.sdp
vendor-ref/simple-media-server/Src/Webrtc/SdpExample/h265-offer.sdp
vendor-ref/simple-media-server/Src/Webrtc/SdpExample/publish-offer-sms.sdp
vendor-ref/simple-media-server/Src/Webrtc/SdpExample/janus_offer.sdp
```

测试要求：

- 能解析可支持 SDP 并生成 answer。
- 对不支持 codec/profile 的 SDP 返回明确错误。
- simulcast SDP 能提取 RID/SSRC 层信息。
- h265 offer 在 browser profile 下默认拒绝或降级，在 device profile 下允许。
- Janus/Chrome/SMS SDP 的 extmap、rtcp-fb、ice 参数都能通过。

## 1.7 Phase 01 测试要求

命令：

```text
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec -- webrtc
cargo clippy -p cheetah-webrtc-core
cargo test -p cheetah-webrtc-core
```

核心测试：

- `core_accept_offer_emits_answer_and_timer`
- `core_create_offer_then_apply_answer_reaches_connecting`
- `core_rejects_invalid_sdp_with_diagnostic`
- `core_does_not_call_system_time`
- `core_datachannel_message_event_roundtrip`
- `codec_webrtc_ingress_requires_normalized_timeline`
- `codec_webrtc_egress_requires_access_unit_boundary`
- `codec_webrtc_parameter_set_replay_for_h26x_keyframe`
- property test：随机 timer/network/user input 顺序不 panic
- fuzz target：SDP compat preprocessor、core packet classifier helper

## 1.8 Phase 01 验收标准

- 新增 crate 名称、目录、workspace 成员符合 AGENTS.md。
- `cheetah-webrtc-core` 编译通过，且不含 `tokio` 依赖。
- core API 中没有 `async fn` 作为状态机接口。
- core 不调用 `Instant::now()` 或系统时间 API。
- codec WebRTC contract 测试全部通过。
- SMS SDP fixtures 被纳入测试或测试资源索引。

