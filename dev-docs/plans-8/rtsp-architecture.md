# RTSP 完善总体架构设计

- 状态：计划中
- 范围：固定 RTSP 完善工作的三段式边界、四类 RTP 传输语义、服务端推拉流、远端转发、兼容策略和测试边界。
- 完成标准：实现者能够据此在现有 RTSP crate 内补齐能力，并确保 core Sans-I/O、driver 承载 I/O、module 只做 engine 编排。

## 架构目标

RTSP 在 cheetah 中承担两类职责：

1. 对外 server：接收客户端 ANNOUNCE/RECORD 推流，或响应 DESCRIBE/SETUP/PLAY 播放本地 engine stream。
2. 对外 client/转发：从远端 RTSP 源 pull 到本地 engine stream，或把本地 engine stream push 到远端 RTSP server。

媒体统一仍然是 `AVFrame + TrackInfo`。RTSP module 不应保存私有媒体模型；RTP packetize/depacketize、时间戳、参数集、RTP reorder、PS/PES 兼容逻辑应尽量回到 `cheetah-codec` 或明确的 RTSP compat helper。

## Crate 边界

保持现有目录和 package：

```text
crates/protocols/rtsp/
  core/          # cheetah-rtsp-core
  driver-tokio/  # cheetah-rtsp-driver-tokio
  module/        # cheetah-rtsp-module
  testing/property-tests/  # cheetah-rtsp-property-tests
  fuzz/          # 独立 cargo-fuzz workspace
```

依赖方向固定为：

```text
cheetah-rtsp-module
  -> cheetah-rtsp-driver-tokio
  -> cheetah-rtsp-core
  -> cheetah-codec

cheetah-rtsp-module -> cheetah-sdk -> cheetah-codec
cheetah-rtsp-driver-tokio -> cheetah-runtime-api
cheetah-runtime-tokio -> cheetah-runtime-api
```

禁止关系：

- `cheetah-rtsp-core` 不依赖 Tokio、SDK、engine、HTTP 框架或真实 socket。
- `cheetah-rtsp-module` 公共接口不暴露 `tokio::*` / `tokio_util::*`。
- RTSP module 不复制 RTMP/HTTP-FLV module 的封装逻辑，不绕过 `cheetah-codec` 时间戳和参数集模型。
- HTTP tunnel 不引入 Axum/Tide/Actix 到 SDK/module 公共接口。

## Target Capability Matrix

| 场景 | RTP over UDP | RTP over TCP | RTP over HTTP tunnel | RTP multicast |
| --- | --- | --- | --- | --- |
| Server publish: ANNOUNCE/SETUP/RECORD | 必须支持 | 必须支持 | 必须支持 interleaved-over-tunnel | 可选支持 publish-side multicast ingest，首版默认关闭 |
| Server play: DESCRIBE/SETUP/PLAY | 必须支持 | 必须支持 | 必须支持 tunnel GET 输出 | 必须支持 PLAY multicast |
| RTSP pull job: remote -> local | 必须支持 | 必须支持 | 必须支持 | 可选支持接收远端 multicast SDP/SETUP，默认关闭 |
| RTSP push job: local -> remote | 必须支持 | 必须支持 | 必须支持 | 不作为首版默认目标 |
| Relay job: remote -> local -> remote | 由 pull + push 组合 | 由 pull + push 组合 | 由 pull + push 组合 | 仅在两端配置允许时启用 |

首版默认优先级：TCP interleaved > UDP unicast > HTTP tunnel > multicast。原因是 TCP interleaved 兼容 NAT 和测试成本最低；UDP 是主流 IPC/NVR 必需；HTTP tunnel 是防火墙兼容；multicast 需要 runtime multicast API 和地址池治理。

## Transport Model

新增或收敛到统一模型：

```rust
pub enum RtspRtpTransportKind {
    UdpUnicast,
    TcpInterleaved,
    HttpTunnel,
    Multicast,
}

pub struct RtspTransportSelection {
    pub kind: RtspRtpTransportKind,
    pub mode: RtspTransportMode,
    pub rtp_channel: Option<u8>,
    pub rtcp_channel: Option<u8>,
    pub client_rtp: Option<std::net::SocketAddr>,
    pub client_rtcp: Option<std::net::SocketAddr>,
    pub server_rtp: Option<std::net::SocketAddr>,
    pub server_rtcp: Option<std::net::SocketAddr>,
    pub multicast_group: Option<std::net::IpAddr>,
    pub multicast_ttl: Option<u8>,
    pub ssrc: Option<u32>,
}
```

该类型是设计目标，不要求一次性按此名称落地；关键是所有 SETUP 逻辑都应先解析多个 Transport 候选，再按配置选择一个候选，而不是在 module 中用字符串 `contains` 分支散落处理。

## HTTP Tunnel Semantics

RTSP-over-HTTP tunnel 按 Axis/QuickTime 常见兼容行为实现：

- 客户端建立两个 HTTP 连接：GET 用于 server -> client，POST 用于 client -> server。
- 两个连接使用相同 `x-sessioncookie` 绑定为一个逻辑 RTSP 连接。
- GET 返回 `HTTP/1.0 200 OK` 或 `HTTP/1.1 200 OK`，`Content-Type: application/x-rtsp-tunnelled`；后续 payload 是 plaintext RTSP response 与 `$` interleaved RTP/RTCP。
- POST 使用 `Content-Type: application/x-rtsp-tunnelled`，body 是 base64 编码的 RTSP request 和 `$` interleaved frame byte stream。
- POST 的 `Content-Length` 可能是任意大值，不能当作真实数据结束条件；driver 应按连接关闭或 cancellation 结束。
- tunnel 只改变承载，不改变 RTSP session、Transport、RTP/RTCP、module 行为。

## Multicast Semantics

首版 multicast 以 PLAY 输出为主：

- 配置 `multicast.enabled`、地址池、端口池、TTL、reuse policy、每 stream 最大 multicast tracks。
- SETUP 接收到 `Transport: RTP/AVP;multicast` 时，从地址池为 stream/track 分配 multicast group 和 RTP/RTCP 端口。
- 响应包含 `Transport: RTP/AVP;multicast;destination=<group>;port=<rtp>-<rtcp>;ttl=<ttl>;ssrc=<ssrc>`。
- 同一 stream 的多个 multicast player 复用 multicast sender，慢客户端不会影响发送。
- 没有 player 或 session 超时后释放 multicast sender；释放需延迟 grace period，避免频繁 join/leave 抖动。
- publish-side multicast ingest 后续可扩展为 server join 客户端给定 group；首版默认拒绝或配置关闭，避免非预期接收外部组播流。

## Server Publish Flow

```text
ANNOUNCE SDP
  -> parse stream key and SDP
  -> acquire publisher lease
  -> sink.update_tracks
  -> SETUP each track
  -> RECORD starts ingest
  -> RTP/RTCP packets arrive through selected transport
  -> cheetah-codec packetize/depacketize/timestamp normalize
  -> sink.push_frame(Arc<AVFrame>)
  -> PAUSE stops ingest without dropping session
  -> TEARDOWN/close releases lease
```

兼容要求：

- ANNOUNCE 的 SDP IP 可以规范化为 `0.0.0.0` 或保留 origin side data，但 engine track 不依赖原始 IP。
- 支持 aggregate 和 per-track SETUP；多轨时缺 track control 返回 459。
- RECORD 前收到 RTP 应丢弃或只更新最小统计，不进入 engine。
- publish side 默认不强制鉴权；是否鉴权由配置决定，以兼容 FFmpeg 和部分设备。

## Server Play Flow

```text
DESCRIBE stream URI
  -> get stream snapshot
  -> build SDP from TrackInfo
  -> SETUP selected tracks and selected transport
  -> PLAY subscribes engine stream
  -> send 200 with Range/RTP-Info
  -> packetize AVFrame to RTP
  -> pace by media timestamp when enabled
  -> send RTP/RTCP over selected transport
  -> PAUSE stops forwarding but preserves session/seq/rtcp continuity
  -> TEARDOWN sends BYE and cleans resources
```

兼容要求：

- `Content-Base`、absolute/relative `a=control` 都可解析。
- PLAY 支持 aggregate URI 和 selected track URI。
- 有视频默认从关键帧起播；audio-only 不等待关键帧。
- RTP-Info 包含每条 selected track 的 url/seq/rtptime，rtptime 不再固定为 0。

## Outbound Client And Forwarding

新增 outbound RTSP client 能力：

```text
Pull:  remote RTSP server -> cheetah engine stream
Push:  cheetah engine stream -> remote RTSP server
Relay: Pull job + Push job with explicit source/target mapping
```

配置模型沿用 RTMP jobs 的监督语义：

- `pull_jobs[]`：`source_url` -> `target_stream_key`。
- `push_jobs[]`：`source_stream_key` -> `target_url`。
- `relay_jobs[]`：可选语法糖，内部展开成 pull + push 或 source subscription + outbound push。
- 失败后指数退避，退避有最大值。
- pull job 目标 stream 已有 publisher 时停止，不循环抢占。
- push job 源不存在时等待或退避，不阻塞 module stop。

## Session Lifecycle And Resource Semantics

统一 session 状态机（server publish 与 server play 共用语义）：

```text
Init
  -> Announced (ANNOUNCE accepted, publish lease acquired)
  -> Described (DESCRIBE accepted, stream metadata resolved)
  -> Ready (至少一个 track 完成 SETUP)
  -> Playing (PLAY active)
  -> Recording (RECORD active)
  -> Paused (PAUSE active, transport/session 保留)
  -> Teardown (TEARDOWN / connection close / module stop cleanup complete)
```

关键约束：

- 所有 SETUP 必须先解析 Transport 候选集，再按策略选择一个候选；选择失败返回 461，不创建半状态。
- `Paused` 不释放 transport/socket/interleaved mapping，恢复 `PLAY/RECORD` 时保持 seq/ssrc/rtcp continuity。
- `Teardown` 必须释放 publish lease、subscriber、UDP socket、multicast sender 引用、tunnel registry 绑定。
- outbound `pull/push/relay` 的 stop/retry 必须可中断；module stop 时不允许后台 job 挂起。

RTCP 发送时机（首版统一）：

- publish side：收到 SR 后按策略回 RR；`Paused` 期间不回 RR；恢复 `RECORD` 后继续 continuity。
- play side：`PLAY` 周期发送 SR+SDES；`TEARDOWN` 发送 BYE；`PAUSE` 不重置统计状态。
- 上述行为统一走 `cheetah-rtsp-core` / `cheetah-codec` parser/builder，禁止模块内散落裸字节拼包。

## Compatibility Policy

集中管理以下兼容点：

- Transport 参数大小写不敏感；支持多个 Transport 候选，以逗号分隔。
- UDP `client_port=rtp` 可按奇偶推断 RTCP，但端口 65535 不推断。
- TCP `RTP/AVP/TCP` 缺 `interleaved` 时可按 track index 默认分配；是否允许由配置控制。
- `destination` 默认只能是控制连接 peer IP；允许第三方 destination 必须显式配置，避免反射攻击。
- `source`、`server_port`、`port`、`ttl`、`layers`、`mode`、`ssrc` 要保留并 round-trip 到响应。
- SDP 可接受 `\n`、`\r\n`、多余空白、absolute control URI、`streamid=0`、`trackID=0`、payload type 静态推断。
- 对 MP2P/PS、AAC LATM、G711、MP3、ADPCM、H26x sprop 缺失等兼容处理集中在 `cheetah-codec` 或 `rtsp::media` 的明确 compat 层。
- 错误输入做 bounded robustness：非法 header、oversize body、超大 interleaved frame、base64 半包、UDP 短包、RTCP compound 截断均不能 panic。

## Verification And Doc Sync Policy

测试断言分层：

- 标准样例（H264/AAC、H265/AAC、audio-only、标准 Transport）必须做强断言：状态码、RTP-Info、seq/rtptime continuity、RR/SR/BYE 时机、TrackInfo 就绪状态。
- compat probe/fault 样例（PS/MP2P、缺失 rtpmap/fmtp、bad marker、乱序/丢包/截断）只要求 bounded health：不 panic、无无界缓存、module 可 stop、会话可 cleanup。
- Transport/SDP/Auth/HTTP tunnel/multicast 输入兼容列表以本文件 `Compatibility Policy` 为唯一基线，新增兼容必须补对应回归。

首版范围外（保持冻结）：

- RTSPS/TLS、HTTPS tunnel、SRTP/SAVP、完整 VOD seek/scale 不进入首版。

文档同步清单（每次调整边界或行为时必须同步）：

- `SystemArchitecture.md`
- `dev-docs/plans-8/rtsp-architecture.md`
- RTSP README（模块能力、配置项、已知限制）
- 配置示例（含 transport 选择、auth、multicast、job 配置）

## 不进入首版范围

- RTSPS/TLS、HTTPS tunnel。
- RTSP 2.0 完整状态机；只参考 RFC 7826 的 Transport 例子和兼容语义。
- SRTP/SAVP。
- NAT traversal 扩展协议完整实现；只做 UDP 打洞与 destination 安全策略。
- 点播 seek/scale 完整媒体文件读取；首版 Range/Scale 仍以 live 流兼容响应为主。

## 具体任务

### A.1 固定三段式边界和目标矩阵

- [x] 确认不新增 RTSP crate，只扩展现有 RTSP 三段式。
- [x] 在设计文档中固定四类 RTP transport 的 server/client/job 支持矩阵。
- [x] 明确 HTTP tunnel 是 RTSP 承载，不复用 HTTP-FLV module。
- [x] 明确 multicast 首版主目标是 PLAY 输出，publish-side multicast 默认关闭。

### A.2 固定传输抽象、session lifecycle 和转发语义

- [x] 抽象统一 Transport selection，所有 SETUP 先解析候选再选择。
- [x] 固定 session 状态：Init、Announced、Described、Ready、Playing、Recording、Paused、Teardown。
- [x] 固定 server publish/play 与 outbound pull/push/relay 的资源释放语义。
- [x] 固定 RTP/RTCP 统计、BYE、SR/RR、SDES 的发送时机。

### A.3 固定兼容策略和测试分层

- [x] 固定 Transport、SDP、Auth、HTTP tunnel、multicast 的兼容输入列表。
- [x] 标准样例做强断言，probe/fault 样例只做 bounded health 断言。
- [x] 固定首版不做 RTSPS/HTTPS/SRTP/完整 VOD。
- [x] 规划文档同步项：`SystemArchitecture.md`、RTSP README、配置示例。

## 完成后检查

```bash
cargo fmt
cargo check -p cheetah-rtsp-core
cargo check -p cheetah-rtsp-driver-tokio
cargo check -p cheetah-rtsp-module
```
