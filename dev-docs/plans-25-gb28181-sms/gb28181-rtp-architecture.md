# GB28181 与 RTP 总体架构设计

- **状态**: 已完成
- **范围**: 固定 GB28181 与 RTP 的 crate 边界、共享媒体 API、REST 控制模型、主动/被动数据流和兼容策略
- **完成标准**: 实现者能够据此拆出 `cheetah-rtp-*` 与 `cheetah-gb28181-*` 三段式 crate，并把媒体基础能力收敛到 `cheetah-codec`

## 架构目标

本次能力分成两条主线：

1. **RTP 媒体面**：独立提供 RTP server/client、PS/TS/ES/Ehome 承载、UDP/TCP/RTCP、与 engine 的 publish/subscribe 桥接。这是一套独立协议能力，不依赖 GB28181 才能存在。
2. **GB28181 控制面**：负责 SIP/SDP/Invite/Register/Keepalive/Bye、主动拉流、被动收流、语音对讲、设备会话。GB28181 media plane 仍走 RTP，只是在会话和 API 上增加国标约束。

首版实现策略：

- 先把媒体面做成独立 `rtp` 协议三段式 crate
- 再让 `gb28181` 协议三段式 crate 调用 RTP 能力完成媒体收发
- 所有媒体最终统一为 `AVFrame + TrackInfo`

## Crate 与依赖方向

新增目录与 package：

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

依赖方向固定为：

```text
cheetah-rtp-module
  -> cheetah-rtp-driver-tokio
  -> cheetah-rtp-core
  -> cheetah-codec

cheetah-gb28181-module
  -> cheetah-gb28181-driver-tokio
  -> cheetah-gb28181-core
  -> cheetah-sdk
  -> cheetah-codec
```

约束：

- `cheetah-rtp-core` 不依赖 Tokio、SDK、engine、socket
- `cheetah-gb28181-core` 不依赖 Tokio、socket、数据库、engine
- `cheetah-gb28181-module` 不复制 RTP socket/RTCP/重排逻辑
- `cheetah-gb28181-module` 不直接编译期依赖 `cheetah-rtp-module`；二者通过内部 `RtpSessionService` 抽象协作
- `cheetah-codec` 负责容器、payload、时间戳和 codec 归一化，不负责 SIP 状态机

## 共享媒体 API

`cheetah-codec` 需要增强：

```rust
pub enum RtpPayloadKind {
    Ps,
    Ts,
    Es,
    EhomePs,
    EhomeEs,
    RawAudio,
    RawVideo,
}

pub struct PsDemuxer;
pub struct PsMuxer;
pub struct TsMuxer;
pub struct TsDemuxer;

pub enum MediaDemuxEvent {
    TrackInfo(Vec<TrackInfo>),
    Frame(AVFrame),
    Diagnostic(MediaDiagnostic),
}
```

设计要求：

- PS/TS/ES/Ehome 输入最终都输出 `TrackInfo` 和 `AVFrame`
- TS 继续复用现有 `ts_demux`/`ts_mux`，PS 需要提升到可用生产级
- ES 模式由 codec-specific depacketizer 直接输出 frame
- H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1 都能进入统一时间线
- 所有 source timestamp、RTP sequence、SSRC、payload type 以 side data 或 diagnostic 形式保留

## RTP Server / Client 数据流

RTP ingress：

```text
UDP/TCP socket
  -> cheetah-rtp-driver-tokio
  -> cheetah-rtp-core session/router
  -> cheetah-codec demux/payload decoder
  -> TrackInfo + AVFrame
  -> cheetah-rtp-module publisher
  -> Engine StreamManager
```

RTP egress：

```text
Engine StreamManager
  -> cheetah-rtp-module subscriber
  -> cheetah-codec mux/payload encoder
  -> cheetah-rtp-core send session
  -> cheetah-rtp-driver-tokio
  -> UDP/TCP peer
```

规则：

- server 支持 `recv_only`、`send_only`、`send_recv`
- client 支持先 create、后 start、再 stop
- 同一接收会话按 SSRC 固定路由，默认绑定 source address
- 允许 `app/stream` 显式映射，也允许默认落到 `/live/{ssrc}`

## GB28181 数据流

GB28181 控制面：

```text
SIP UDP/TCP
  -> cheetah-gb28181-driver-tokio
  -> cheetah-gb28181-core dialog/transaction state
  -> cheetah-gb28181-module device/session manager
  -> RTP module service
```

主动拉流：

```text
REST create recv(active=true)
  -> gb28181-module build invite session
  -> GB28181 SIP INVITE/ACK
  -> allocate RTP session/port
  -> remote pushes RTP
  -> rtp-module publishes local stream
```

双向语音：

```text
REST talk start
  -> gb28181-module create talk session
  -> subscribe local audio stream
  -> rtp-module send audio RTP to device
  -> optional reverse audio publish back to engine
```

## REST API 设计

RTP 路由保持 SMS 兼容：

```text
POST /api/v1/rtp/server/create
POST /api/v1/rtp/server/stop
POST /api/v1/rtp/client/create
POST /api/v1/rtp/client/start
POST /api/v1/rtp/client/stop
```

GB28181 路由保持 SMS 兼容并扩展标准控制：

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

输入兼容规则：

- `socketType` 支持 `tcp`、`udp`、`both` 和 SMS 数字值
- `payloadType` 支持 `ps`、`ts`、`es`、`ehome`
- `transportMode` 支持 `recv_only`、`send_only`、`send_recv`
- `active` 支持布尔和 `0/1`

## 配置草案

```yaml
modules:
  rtp:
    enabled: true
    listen_udp: "0.0.0.0:10000"
    listen_tcp: "0.0.0.0:10000"
    rtcp_listen_udp: "0.0.0.0:10001"
    write_queue_capacity: 256
    read_buffer_size: 65536
    max_reassembly_bytes: 4194304
    max_tracks: 32
    idle_timeout_ms: 15000
    max_sessions: 1024
    default_payload: ps
    allow_unaligned_payload: true
    pull_jobs: []

  gb28181:
    enabled: true
    sip_listen_udp: "0.0.0.0:5060"
    sip_listen_tcp: "0.0.0.0:5060"
    realm: "3402000000"
    server_id: "34020000002000000001"
    nonce_ttl_ms: 30000
    media_port_range: "30000-35000"
    device_timeout_ms: 90000
    invite_timeout_ms: 10000
    register_required: true
    talk:
      enabled: true
      default_payload: ps
```

配置变更结果：

- 改变 listen、端口池、queue、timeout、pull_jobs：`ModuleRestartRequired`
- 只改变阈值或诊断等级可后续扩展为 `Immediate`

## 安全与边界

- REST 请求体、SIP message、RTP TCP frame、PS/TS/ES 组包缓存全部有上限
- 单连接慢消费者不能拖累其他流
- UDP source address 漂移默认拒绝，显式兼容模式下允许重绑定并记录告警
- 同一 `StreamKey` 默认独占发布；主动/被动模式都不能绕过租约模型
- SIP 鉴权、nonce、CSeq、Call-ID、branch、dialog 状态必须做基本校验
