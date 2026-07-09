# GB28181 与 RTP 总体架构设计（对标 ABLMediaServer）

- **状态**: 已完成
- **范围**: 固定 GB28181 与 RTP 的 crate 边界、共享媒体 API、RTP/RTCP 传输模型、REST 控制模型和兼容策略
- **完成标准**: 实现者能够据此完善 `cheetah-rtp-*` 与 `cheetah-gb28181-*` 三段式 crate，并把媒体基础能力收敛到 `cheetah-codec`

## 架构目标

本次能力分成两条主线：

1. **RTP 媒体面**：独立提供 RTP server/client、PS/TS/ES/JTT1078 承载、UDP/TCP/RTCP、主动/被动发送、语音回链和与 engine 的发布订阅桥接。
2. **GB28181 控制面**：负责 SIP/SDP/REGISTER/INVITE/ACK/BYE/Keepalive、主动拉流、被动收流、语音对讲和设备会话编排，媒体面仍走 RTP。

ABL 的实现说明媒体面兼容比协议名义更重要，因此首版顺序固定为：

- 先把 RTP 媒体面做成独立协议三段式
- 再让 GB28181 module 通过内部 `RtpSessionService` 调用 RTP 能力
- 所有媒体最终仍统一为 `AVFrame + TrackInfo`

## Crate 与依赖方向

目标目录与 package：

```text
crates/protocols/rtp/
  core/                    # cheetah-rtp-core
  driver-tokio/            # cheetah-rtp-driver-tokio
  module/                  # cheetah-rtp-module
  testing/property-tests/  # cheetah-rtp-property-tests
  fuzz/                    # standalone cargo-fuzz workspace

crates/protocols/gb28181/
  core/                    # cheetah-gb28181-core
  driver-tokio/            # cheetah-gb28181-driver-tokio
  module/                  # cheetah-gb28181-module
  testing/property-tests/  # cheetah-gb28181-property-tests
  fuzz/                    # standalone cargo-fuzz workspace
```

依赖方向：

```text
cheetah-rtp-module
  -> cheetah-rtp-driver-tokio
  -> cheetah-rtp-core
  -> cheetah-codec
  -> cheetah-sdk

cheetah-gb28181-module
  -> cheetah-gb28181-driver-tokio
  -> cheetah-gb28181-core
  -> cheetah-sdk
  -> cheetah-codec
```

约束：

- `cheetah-rtp-core` 和 `cheetah-gb28181-core` 都必须是 Sans-I/O
- `cheetah-gb28181-module` 不直接编译期依赖 `cheetah-rtp-module`，二者通过内部 `RtpSessionService` 协作
- `cheetah-codec` 只做容器、payload、timestamp、track 归一化，不做 SIP 状态机

## 共享媒体 API

`cheetah-codec` 需要补齐的共享能力：

```rust
pub enum RtpPayloadMode {
    Ps,
    Ts,
    Es,
    Xhb,
    Jtt1078,
}

pub enum RtpTcpFraming {
    TwoByteLength,
    Interleaved4Byte,
    AutoDetect,
}

pub enum RtpMediaEvent {
    TrackInfo(Vec<TrackInfo>),
    Frame(AVFrame),
    Diagnostic(RtpMediaDiagnostic),
}
```

设计要求：

- PS/TS/ES/JTT1078 输入最终都输出 `TrackInfo` 和 `AVFrame`
- JTT1078 兼容只在共享 compat 或 core 层做，不散落到 module
- G711 RTP 包时长、payload type hint、timestamp wrap、B-frame DTS 生成统一回到 `cheetah-codec`
- 所有缓存、reassembly 和 reorder window 必须 bounded

## RTP 数据流

RTP ingress：

```text
UDP/TCP socket
  -> cheetah-rtp-driver-tokio
  -> cheetah-rtp-core session/router
  -> cheetah-codec payload/container decode
  -> TrackInfo + AVFrame
  -> cheetah-rtp-module publisher
  -> Engine StreamManager
```

RTP egress：

```text
Engine StreamManager
  -> cheetah-rtp-module subscriber
  -> cheetah-codec payload/container encode
  -> cheetah-rtp-core send session
  -> cheetah-rtp-driver-tokio
  -> UDP/TCP peer
```

规则：

- 支持 `recv_only`、`send_only`、`send_recv`
- 支持 `udp_active`、`udp_passive`、`tcp_active`、`tcp_passive`
- 支持 `voice_talk` 模式复用现有上行链路返回音频 RTP
- 未显式指定 stream 时允许默认映射 `/live/{ssrc}`
- 支持 `only_audio` 与 `only_video`

## ABL 风格兼容层

必须保留的 compat 语义：

- TCP RTP 自动识别 2-byte 和 4-byte 头，不把 framing 细节泄漏到 module
- `nMaxRtpLength` 风格的动态最大包长学习，但用显式上界和诊断事件替代 ABL 的裸整数共享状态
- 单端口 RTP 按首个有效载荷自动分流到 PS、TS 或 ES 路径
- JTT1078 的 2013/2016 与 2019 版本分离解析，共享 SIM/channel 路径命名
- `ForceSendingIFrame` 落到发送端的 IDR/参数集缓存策略，而不是散落在 module 编排中

## GB28181 数据流

控制面：

```text
SIP UDP/TCP
  -> cheetah-gb28181-driver-tokio
  -> cheetah-gb28181-core
  -> cheetah-gb28181-module session manager
  -> RtpSessionService
```

主动拉流：

```text
REST create recv(active=true)
  -> GB28181 INVITE/ACK
  -> allocate RTP recv session
  -> remote pushes RTP
  -> rtp publish to engine
```

双向语音：

```text
REST talk start
  -> gb28181-module create talk session
  -> subscribe local audio stream
  -> rtp send audio to device
  -> optional reverse audio publish
```

## REST API 设计

RTP 路由：

```text
POST /api/v1/rtp/server/create
POST /api/v1/rtp/server/stop
POST /api/v1/rtp/client/create
POST /api/v1/rtp/client/start
POST /api/v1/rtp/client/stop
```

GB28181 路由：

```text
POST /api/v1/gb28181/recv/create
POST /api/v1/gb28181/recv/stop
POST /api/v1/gb28181/send/create
POST /api/v1/gb28181/send/stop
POST /api/v1/gb28181/talk/start
POST /api/v1/gb28181/talk/stop
GET  /api/v1/gb28181/devices
POST /api/v1/gb28181/invite
POST /api/v1/gb28181/bye
```

兼容字段：

- `transport`: `tcp`、`udp`、`both`，兼容 ABL 的 `enable_tcp`/`is_udp`
- `payloadType`: `ps`、`ts`、`es`、`xhb`、`jtt1078`
- `tcpHeaderType`: `two_byte`、`interleaved_4byte`、`auto`
- `disableVideo`、`disableAudio`
- `recvApp`、`recvStream`
- `jtt1078Version`: `2013`、`2016`、`2019`
- `keepOpenMode`: `single`、`live`、`playback`、`talk`、`sub`

## 配置草案

```yaml
modules:
  rtp:
    enabled: true
    listen_udp: "0.0.0.0:10000"
    listen_tcp: "0.0.0.0:10000"
    rtcp_listen_udp: "0.0.0.0:10001"
    tcp_header_type: auto
    max_rtp_len_initial: 2048
    max_rtp_len_cap: 65536
    video_mtu: 1400
    audio_mtu: 600
    g711_packet_duration_ms: 100
    idle_timeout_ms: 15000
    save_debug_payload: false

  gb28181:
    enabled: true
    sip_listen_udp: "0.0.0.0:5060"
    sip_listen_tcp: "0.0.0.0:5060"
    realm: "3402000000"
    server_id: "34020000002000000001"
    media_port_range: "30000-35000"
    force_sending_iframe: false
    g711_convert_aac: true
    device_timeout_ms: 90000
```
