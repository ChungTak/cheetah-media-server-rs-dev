# 04 · 架构与公共契约

## 1. 数据流与所有权

```text
third-party signaling/control system (all GB signaling responsibility)
        |
        | negotiated structured media parameters
        v
HTTP/gRPC/C-API adapter (optional, DTO mapper only)
        v
cheetah-media-api: RtpSessionApi / controlled resource ports
        v
cheetah-gb28181-module --typed call--> cheetah-rtp-module
                                           v
                                     RTP core/driver
                                           v
                       cheetah-codec -> AVFrame + TrackInfo -> engine
```

本项目不提供 local signaling owner。GB module 只编排媒体资源；现有 SIP core/driver/listener 退出
运行时与默认/生产 feature，并最终从仓库删除。

删除信令实现后仍遵守协议三段式：

- `cheetah-gb28181-core`：纯 GB 媒体会话状态、SSRC/PT/container/framing 校验和显式 action；
  不包含 SIP/SDP/XML 类型。
- `cheetah-gb28181-driver-tokio`：只驱动 GB 媒体连接、RTP/RTCP framing、timer 和 backpressure，
  不监听信令端口。
- `cheetah-gb28181-module`：实现公开媒体接口、admission、资源分配、StreamKey 绑定和业务无关的
  媒体编排，不维护设备/通道/信令状态。

## 2. 公共类型

在 `cheetah-media-api` 增加或收敛以下 runtime-neutral 类型，名称可在实现时与既有同义类型合并，
但 wire/domain 语义不得改变：

- `RtpSessionId`、`RtpSessionGeneration`、`RtpSessionResourceRef`。
- `RtpTransport::{Udp,Tcp}`。
- `TcpRole::{Active,Passive}`，仅 TCP 请求可设置。
- `RtpFraming::{Datagram,Rfc4571,DollarPrefixed,AutoDetect}`。
- `MediaContainer::{Ps,Ts,ElementaryStream,AutoDetect}`。
- `RtpDirection::{Receive,Send,DuplexTalk}`。
- `SourceBindingPolicy::{Strict,AllowValidatedRebind}`。
- `GbMediaCompatibilityProfile` 使用 03 的受控枚举，禁止任意字符串改变安全策略。
- `RtpPayloadBinding` 表达 PT、codec、clock rate、channels；动态 PT 不由 codec 名隐式猜测。
- `RtpSessionState::{Allocating,Ready,Active,Draining,Stopped,Failed}`。

## 3. Typed port

`RtpSessionApi` 至少提供：

```text
open_receiver(context, OpenRtpReceiver) -> RtpSessionDescriptor
open_sender(context, OpenRtpSender) -> RtpSessionDescriptor
open_talk(context, OpenRtpTalk) -> RtpSessionDescriptor
update_session(context, UpdateRtpSession) -> RtpSessionDescriptor
get_session(read_context, RtpSessionRef) -> RtpSessionDescriptor
stop_session(context, StopRtpSession) -> EffectOutcome
```

所有 mutation 必须携带 tenant、deadline、idempotency key、owner epoch、media node instance epoch
和 expected generation。响应返回 resolved local/remote address、SSRC、payload/container/framing、
generation、state 和资源引用；不得返回 socket、Tokio handle 或内部文件路径。

`open_*` 不提供绕过鉴权的便捷入口。所有 adapter 和应用内调用都必须构造 mutation context 并
经过统一 admission policy，不能直达 driver。

外部系统必须传入已解析、已协商的结构化字段；API 明确拒绝 raw SIP、raw SDP、MANSCDP/XML、
REGISTER/INVITE dialog 或设备目录对象。

## 4. 原子资源协议

固定执行顺序：

```text
validate/deadline
 -> owner + instance + generation fence
 -> drain + capacity precheck
 -> MediaAdmissionApi::authorize
 -> reserve capacity permit
 -> reserve port/publisher lease
 -> create socket/worker/task
 -> publish resource index + idempotent result
```

使用 rollback guard 逆序释放尚未提交的资源。只有 durable resource/result 同步成功后才提交
guard；恢复阶段遇到 UNKNOWN 必须 query/reconcile，不得无条件重复 open。

## 5. 错误与 outcome

至少区分 `InvalidArgument`、`Unauthorized`、`Forbidden`、`Conflict`、`StaleOwner`、
`StaleGeneration`、`CapacityExceeded`、`Unavailable`、`Unsupported`、`DeadlineExceeded`、
`TransportFailure` 和 `Internal`。错误携带 sanitized resource ref 与 retry hint，不携带 secret、
原始控制报文或完整外部地址凭据。

stop 对已停止/不存在资源返回幂等 `NOT_APPLIED` 或既有成功结果；实际关闭失败返回明确错误，
不得通过 HTTP 200 或空 body 伪造成功。

## 6. Crate 依赖约束

- core 只依赖 foundation/domain，不依赖 runtime、engine、HTTP、DB 或 module。
- driver 负责 socket/timer/task/framing/backpressure，可依赖 runtime adapter，不持有业务权限状态。
- module 依赖 SDK/media API/codec，通过 `EngineContext` 注入能力，不暴露 Tokio 类型。
- generated external DTO 只存在对应 adapter，mapper 转换后立即进入 Domain 类型。
- `cheetah-codec` 不依赖 GB/RTP module，也不泄漏 avcodec/backend 类型。
