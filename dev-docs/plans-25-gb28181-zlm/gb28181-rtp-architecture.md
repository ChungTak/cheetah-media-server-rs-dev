# GB28181 与 RTP 总体架构设计（对标 ZLMediaKit）

- **状态**: 已完成
- **范围**: 固定 GB28181 与 RTP 的 crate 边界、共享媒体 API、RTP/RTCP 传输模型、REST 控制模型和兼容策略
- **完成标准**: 实现者能够据此完善 `cheetah-rtp-*` 与 `cheetah-gb28181-*` 三段式 crate，并把媒体基础能力收敛到 `cheetah-codec`

## 架构目标

本次能力分成两条主线：

1. **RTP 媒体面**：独立提供 RTP server/client、PS/TS/ES/Ehome 承载、UDP/TCP/RTCP、主动/被动发送、语音回链和与 engine 的发布订阅桥接。
2. **GB28181 控制面**：负责 SIP/SDP/REGISTER/INVITE/ACK/BYE/Keepalive、主动拉流、被动收流、语音对讲和设备会话编排，媒体面仍走 RTP。

ZLM 的实现说明媒体面兼容比协议名义更重要，因此首版顺序固定为：

- 先把 RTP 媒体面做成独立协议三段式
- 再让 GB28181 module 通过内部 RTP session service 调用 RTP 能力
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
    Ehome,
}

pub enum RtpMediaEvent {
    TrackInfo(Vec<TrackInfo>),
    Frame(AVFrame),
    Diagnostic(RtpMediaDiagnostic),
}

pub struct PsMuxer;
pub struct PsDemuxer;
pub struct TsMuxer;
pub struct TsDemuxer;
pub struct RtpTcpFramer;
pub struct RtpTcpDeframer;
```

设计要求：

- PS/TS/ES/Ehome 输入最终都输出 `TrackInfo` 和 `AVFrame`
- Ehome 兼容只在共享 compat 或 core 层做，不散落到 module
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

- `socketType`: `tcp`、`udp`、`both` 以及数字兼容值
- `payloadType`: `ps`、`ts`、`es`、`ehome`
- `transportMode`: `recv_only`、`send_only`、`send_recv`
- `conType`: `tcp_active`、`udp_active`、`tcp_passive`、`udp_passive`、`voice_talk`
- `onlyAudio`: 布尔或 `0/1`

## 配置草案

```yaml
modules:
  rtp:
    enabled: true
    listen_udp: "0.0.0.0:10000"
    listen_tcp: "0.0.0.0:10000"
    rtcp_listen_udp: "0.0.0.0:10001"
    video_mtu: 1400
    audio_mtu: 600
    max_rtp_kb: 10
    idle_timeout_ms: 15000
    g711_packet_duration_ms: 100
    udp_recv_buffer: 4194304
    max_tracks: 32
    pull_jobs: []

  gb28181:
    enabled: true
    sip_listen_udp: "0.0.0.0:5060"
    sip_listen_tcp: "0.0.0.0:5060"
    realm: "3402000000"
    server_id: "34020000002000000001"
    media_port_range: "30000-35000"
    device_timeout_ms: 90000
    invite_timeout_ms: 10000
    talk:
      enabled: true
```

## 安全与边界

- REST 请求体、SIP message、RTP TCP frame、PS/TS/ES 组包缓存都有上限
- 慢客户端不能拖累其他流或其他 sender
- UDP source address 默认锁定，兼容模式下允许受控重绑定
- TCP 上下文恢复只能在 bounded 窗口内进行，失败后丢弃连接或当前坏片段
- SIP 鉴权、dialog、CSeq、Call-ID、branch 基础校验必须做
