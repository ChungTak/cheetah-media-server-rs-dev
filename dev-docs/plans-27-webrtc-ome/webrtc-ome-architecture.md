# WebRTC OME 对标架构设计

- **状态**: 已完成
- **范围**: 设计文档
- **参考**: `docs/live-source/webrtc.md`、`docs/streaming/webrtc-publishing.md`、`src/projects/{providers,publishers,webrtc,modules}`

## 设计目标

本阶段不是替换本地 WebRTC 技术栈，而是把 OME 里已经验证过的 WebRTC 工程行为沉淀为本项目的 compat 行为、配置项、观测指标和回归样例。

本地 WebRTC 继续保持以下边界：

- `cheetah-webrtc-core`: SDP/ICE/RTCP/RTP 相关 Sans-I/O 状态与纯逻辑。
- `cheetah-webrtc-driver-tokio`: UDP/TCP 收发、route、timer、候选调度、迁移、背压。
- `cheetah-webrtc-module`: HTTP/WebSocket 信令、WHIP/WHEP、引擎接入、playlist/ABR 业务编排。
- `cheetah-codec`: 时间戳归一化、参数集缓存/补发、Access Unit、输出封装视图。

## OME 行为抽象

OME 的 WebRTC 主要体现六类行为：

1. **双信令模型**: WebSocket 自定义信令与 WHIP 并存；publish/play 路径可通过 URL 参数区分。
2. **候选策略可配置**: `udp`、direct TCP ICE、TURN relay、`udptcp`、`all`，以及 `TcpRelayForce` 和 `iceServers` 下发。
3. **播放侧业务编排**: playlist/rendition、`WebRtcAutoAbr`、带宽信号驱动切层、播放通知。
4. **媒体时序与鲁棒性**: `RtcpBasedTimestamp`、`CompositionTime`、`JitterBuffer`、`PlayoutDelay`、周期性 FIR。
5. **损伤恢复**: RTX、RED、ULPFEC、TWCC、REMB、NACK、PLI/FIR 的组合使用。
6. **浏览器与客户端兼容**: H264/H265/VP8/Opus、simulcast RID、`extmap-allow-mixed`、非标准 SDP 兼容。

## 本地落点

| 能力 | 本地落点 | 说明 |
| --- | --- | --- |
| WebSocket 自定义信令 | `crates/protocols/webrtc/module/src/http.rs` 或独立 signaling module | 不进 core；统一走 driver 会话 |
| `direction`/`transport` URL 兼容 | `module/src/compat.rs`、`http.rs` | 集中实现，不散落在 handler |
| UDP/TCP/relay 候选输出 | `driver-tokio` + module response builder | core 仅保留策略枚举，不感知 socket 绑定 |
| `iceServers`/relay 策略 | module 层 HTTP/WebSocket 信令响应 | 不把 TURN 账户模型放进 core |
| playlist/rendition/ABR | module bridge + engine stream model | 用现有 simulcast/multi-stream 能力承接 |
| jitter/playout/FIR 策略 | module config + bridge/driver 调度 | 不在 core 私自实现播放器级缓冲 |
| RTCP-SR timestamp 模式 | `cheetah-codec` + bridge timestamp policy | 时间戳逻辑保持统一 |
| RED/RTX/ULPFEC 兼容 | core/driver/codec 能力协作 | 以配置与测试矩阵方式推进 |

## 配置模型建议

后续实现可在现有 `WebRtcModuleConfig` / `WebRtcDriverConfig` 基础上扩展：

```rust
pub struct WebRtcOmeCompatConfig {
    pub enabled: bool,
    pub signaling_websocket_enabled: bool,
    pub default_transport: WebRtcTransportMode,
    pub tcp_relay_force: bool,
    pub external_ice_servers: Vec<WebRtcIceServer>,
    pub periodic_fir_interval_ms: u64,
    pub rtcp_based_timestamp: bool,
    pub jitter_buffer_enabled: bool,
    pub playout_delay_min_ms: Option<u16>,
    pub playout_delay_max_ms: Option<u16>,
    pub bwe_mode: WebRtcBweMode,
    pub red_enabled: bool,
    pub ulpfec_enabled: bool,
}
```

这些类型只是计划草案；真正实现时应贴合现有配置结构和 serde 默认值。

## 明确不迁移的内容

- 不迁移 OME 自研 ICE/DTLS/SRTP/SCTP 实现，本地继续由 `str0m` 承担核心协议状态机。
- 不把 playlist、带宽估计、媒体 packetizer 全部复制到 module；优先复用 `cheetah-codec` 和已有 bridge。
- 不为 OME 兼容在公共 SDK 接口中引入 tokio、TURN server 或 OME 专属类型。
- 不把 vendor 特判散落到热路径；统一收敛到 compat/config/test。
