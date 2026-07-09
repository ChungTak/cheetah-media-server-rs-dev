# WebRTC 总体架构设计（对标 ZLMediaKit）

## 架构目标

WebRTC 仍按本项目协议三段式组织：

- `cheetah-webrtc-core`：纯 Sans-I/O 会话状态机，包装 `str0m::Rtc`，只处理 SDP、ICE candidate、网络包、timer、媒体事件、RTCP/DataChannel 事件。
- `cheetah-webrtc-driver-tokio`：Tokio socket、timer、任务、单端口调度、UDP/TCP framing、连接迁移、背压和多线程分片。
- `cheetah-webrtc-module`：HTTP API、WHIP/WHEP、ZLM-style API、engine publish/subscribe、资源分配、鉴权、client/P2P job 编排。

ZLMediaKit 的实现把 `WebRtcTransport` 同时承担 SDP、ICE、DTLS、SRTP、SCTP、RTP、RTCP、弱网和业务接入。本项目不能复制这种混合边界；ZLM 只作为兼容行为与工程策略参考。协议状态交给 `str0m`，媒体归一化交给 `cheetah-codec`，业务编排留在 module。

## Crate 与依赖方向

依赖方向必须保持：

```text
apps / control
  -> cheetah-webrtc-module
  -> cheetah-webrtc-driver-tokio
  -> cheetah-webrtc-core
  -> cheetah-codec
```

约束：

- `core` 不依赖 Tokio、socket、HTTP、engine、数据库或系统时间 API。
- `driver` 可以依赖 Tokio，但不持有业务状态，不直接操作 stream lease。
- `module` 不直接依赖 `tokio::net`、`tokio::time`、`tokio::sync` 作为公共接口；内部已有 HTTP client 使用 Tokio 的部分需要收敛为私有实现。
- `cheetah-codec` 不依赖 WebRTC crate；只暴露通用 RTP、SDP、时间戳、参数集、Access Unit 与编码视图。

## 数据流

### WebRTC 推流

```text
WHIP / ZLM publish / P2P offer
  -> module 创建 session、申请 PublishLease
  -> driver 创建 transport session
  -> core 应用 SDP、驱动 ICE/DTLS/SRTP
  -> core 输出 WebRtcMediaEvent::Frame
  -> module bridge 按 simulcast 策略选择 RID
  -> cheetah-codec 归一化 timestamp、AU、参数集
  -> engine PublisherSink
```

推流侧不能在 module 私自维护一套 RTP 时间戳修正或参数集缓存。ZLM `WebRtcPusher` 的“按 RID 创建多个源”可映射为两个策略：

- 默认策略：一个 `StreamKey` 只选一路 RID 入 engine，符合单发布者独占。
- 扩展策略：为 RID 显式生成子流 key，如 `live/cam@rid:h`，必须由配置开启。

### WebRTC 播放

```text
WHEP / ZLM play / P2P answer
  -> module 订阅 engine
  -> engine bootstrap 输出 tracks + GOP
  -> cheetah-codec 导出 WebRTC egress contract
  -> core SendFrame / str0m packetize
  -> driver 发送 UDP/TCP packet
```

ZLM `WebRtcPlayer::sendConfigFrames` 的行为应落到 `cheetah-codec` 参数集补发和 engine bootstrap，而不是在 WebRTC module 中手写 H264/H265 缓存。H264 B 帧过滤只作为播放兼容策略，由 codec 层提供检测或重排结果，module 只消费策略开关。

### Echo

Echo 包含两条路径：

- media echo：收到 RTP media frame 后按同一 track 回发，answer SDP 改写 `msid`，避免 Chrome 忽略远端 track。
- DataChannel echo：收到 DataChannel 消息后原样或按配置前缀返回，受最大消息长度和队列上界限制。

## `str0m` 边界

`str0m` 负责：

- SDP offer/answer、ICE、DTLS、SRTP/SRTCP、SCTP、DataChannel。
- RTP/RTCP 收发、NACK/RTX、TWCC/BWE、simulcast 事件、keyframe request。
- RTP packetize/depacketize 和基础 reorder。

本项目负责：

- UDP/TCP socket、单端口路由、多线程 shard、连接迁移。
- WHIP/WHEP、ZLM-style HTTP API、WebSocket P2P 信令。
- engine publish/subscribe、GOP bootstrap、协议互转。
- ZLM 风格兼容策略、配置、鉴权、观测、弱网测试。
- 自适应发送策略：BWE 驱动降层、限速、丢弃 delta frame、请求关键帧。

## 配置模型

建议在现有 `WebRtcModuleConfig` 上扩展：

```text
listen_udp: "0.0.0.0:8000"
listen_tcp: "0.0.0.0:8000"
enable_tcp: true
driver_shards: 0              # 0 表示按 CPU 自动
ice_lite: false
ice_transport_policy: "all"   # all | relay-only | p2p-only
stun_servers: []
turn_servers: []
extern_ips: []
interfaces: []
handshake_timeout_ms: 30000
migration_route_ttl_ms: 30000
max_sessions: 4096
max_remote_candidates_per_session: 64
rtx_cache_packets: 2048
rtx_cache_age_ms: 3000
nack_max_count: 15
nack_interval_rtt_ratio: 1.0
twcc_max_packets: 20
twcc_max_interval_ms: 256
simulcast_default_policy: "highest"
gop_bootstrap_ms: 2000
datachannel_max_message_bytes: 262144
client_allow_private_ip: false
```

所有缓存、队列、RTX cache、candidate map、DataChannel buffer、P2P room map 必须有上界。

## HTTP 与控制接口

标准接口：

- `POST /whip`：WebRTC ingest，body 为 offer SDP，返回 answer SDP。
- `POST /whep`：WebRTC egress，body 为 offer SDP，返回 answer SDP。
- `PATCH /session/{id}`：trickle ICE candidate fragment。
- `DELETE /session/{id}`：关闭会话。
- `GET /session/{id}`：查询状态和统计。

ZLM-style 兼容接口：

- `POST /api/v1/rtc/publish`：参数兼容 `vhost/app/stream/type/offer`。
- `POST /api/v1/rtc/play`：参数兼容 `vhost/app/stream/type/offer`。
- `POST /api/v1/rtc/echo`：media/DataChannel echo。
- `POST /api/v1/rtc/client/pull/start|stop|list`。
- `POST /api/v1/rtc/client/push/start|stop|list`。
- `POST /api/v1/rtc/p2p/add|remove|list`。

接口返回必须包含明确错误码：鉴权失败、stream 不存在、发布租约冲突、codec 不可协商、SDP 不合法、资源上限、ICE 超时、远端信令失败。

## 观测与诊断

Core 输出事件：

- session lifecycle、ICE state、route migration。
- media track added、frame、RID、codec、clock rate、random access。
- RTCP feedback：PLI、FIR、NACK、TWCC、REMB、SR、RR、BYE。
- BWE：kind、bitrate、loss、rtt、jitter。
- DataChannel opened/message/closed/error。

Module 输出指标：

- active sessions、publish sessions、play sessions、client jobs、P2P rooms。
- send/recv bitrate、packet loss、RTX hit/miss、NACK count、BWE bitrate。
- GOP bootstrap 命中、首帧耗时、首关键帧耗时。
- DataChannel 队列长度、丢弃数、最大消息拒绝数。

## 安全与边界

- WHIP/WHEP client 必须保留 SSRF 防护：默认拒绝 loopback、link-local、private IP，除非显式配置允许。
- HTTP body、SDP、candidate、DataChannel message、ICE candidate 数量必须限长。
- P2P room id、stream key、RID、MID 必须做长度和字符集校验。
- 远端 candidate 不得绕过 ICE 策略和地址策略。
- 所有会话关闭都必须释放 PublishLease、subscriber、DataChannel 队列、client job 和 route entry。

