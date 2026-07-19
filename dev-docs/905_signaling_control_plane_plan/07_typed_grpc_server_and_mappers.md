# 07 · Typed gRPC Server 与 Mapper

## 1. GRPC-01：服务目录

只实现 signaling 固定合同中的 typed RPC：

```text
MediaCapability
  GetCapabilities

MediaQuery
  GetMedia / IsMediaOnline / ListSessions

MediaRtp
  OpenReceiver / ConnectReceiver / OpenSender / Update / Get / List / Stop

MediaProxy
  CreatePull / GetPull / ListPull / DeletePull

MediaRecord
  Start / Stop / Get / ListTasks / ListFiles

MediaSnapshot
  Take / Fetch / Get / List

MediaPlayback
  Open / Get / List / Control / Stop

MediaOutput
  ResolveUrls

MediaControl
  RequestKeyframe / CloseSession

MediaEventStream
  Subscribe
```

若最终合同命名不同，按锁定 descriptor 的 typed method 建立一一 mapper；不得回退到 generic
`bytes payload`/map。未注册 provider 返回 Unsupported/Unavailable，不提供伪成功。

## 2. GRPC-02：Handler 公共流程

每个 handler 使用同一 middleware：

1. 提取真实 mTLS peer identity，不信任普通 metadata 伪造身份。
2. 检查 message size、contract version、deadline。
3. 构造 Principal 与 tenant/resource grants。
4. 显式解析 required Proto 字段和 unknown enum。
5. 映射 `MediaRequestContext`/`MediaMutationContext`。
6. capability、fencing、idempotency、capacity guard。
7. 调用 domain port。
8. 持久化结果/event。
9. 映射 response 或 stable error/outcome。
10. 记录低基数 metrics 与脱敏 audit。

请求 cancellation 传播至 semaphore、store、DNS/connect 和 provider。handler 不持锁跨 await。

## 3. GRPC-03：Capability

响应来源只能是：

- feature 已编译；
- module/provider 已注册且 Running；
- startup preflight 已通过；
- node 当前 lease/health 允许；
- operation contract version 被接受。

返回 node ID/instance epoch、contract range/accepted version、descriptor checksum、capability
generation、operation、runtime state、constraints、capacity、network zones、advertised addresses。
硬件/PNG/SVC 等未交付能力不得因 enum 存在而声明。

## 4. GRPC-04：Query 与 Control

- GetMedia/IsMediaOnline/ListSessions 强制 tenant/filter。
- RequestKeyframe/CloseSession 使用 typed resource ref，不仅使用 MediaKey。
- CloseSession 必须验证 session 所属 tenant/binding/owner/generation。
- output URL 只由 `MediaUrlResolverApi` 基于 configured public endpoint 生成，不信任 Host。
- list 使用统一 cursor，不使用 offset/page number。

## 5. GRPC-05：RTP

mapper 覆盖：

- advertised local address/port；
- RTP/RTCP；
- SSRC policy/value；
- payload type、codec、封装；
- UDP/TCP active/passive/talk；
- remote endpoint；
- state/generation/last safe error。

OpenReceiver/OpenSender 在端口分配前 authorize/fence/capacity。Connect/Update/Stop 要求 expected
generation。所有失败路径释放 port/session/task permit。

## 6. GRPC-06：Proxy

- CreatePull 只接受 sanitized URL + credential handle；
- URL userinfo 在 mapper 阶段直接拒绝；
- scheme/port/zone policy 在 provider 再验证；
- response/source/event 不回显 userinfo；
- Get/List/Delete 使用 opaque proxy handle 与 generation；
- Create 的 processing policy 继续通过现有 typed domain，不在 handler 重写转码逻辑。

## 7. GRPC-07：Record、Snapshot、Playback

- Record Start/Stop/Get/ListTasks/ListFiles 使用 typed handle、MediaSession/Binding 和 generation。
- 不接受任意 storage path；只接受受控 storage policy/handle。
- Snapshot Take 只针对已有 MediaKey；Fetch 使用第 11 章受限 URL 流程。
- Snapshot/Record file response 返回 FileHandle/安全 download reference，不返回服务器路径。
- Playback Open/Control/Stop 使用 typed file/session handle；pause/seek/scale 串行检查 generation。
- 完成/失败事件必须关联原 binding、operation、instance epoch。

## 8. GRPC-08：Mapper 规则

- 每个 request/response/error 独立函数；不使用通用 JSON roundtrip。
- 时间戳检查合法范围，duration/size/port 使用 checked conversion。
- map 在 canonical digest 前排序。
- unknown enum 返回 InvalidArgument 或 VersionMismatch。
- domain non-exhaustive enum 使用显式兼容分支，不伪造已知值。
- URL、endpoint、SDP、last error 经过专用 sanitizer。
- generated DTO 不实现或派生 domain trait。

建立 `mapper_matrix.md` 或本章内完整表，列出每个 wire 字段、domain 字段、默认/拒绝策略。

## 9. GRPC-09：限流与消息边界

配置并强制：

- 全局/每 peer/每 tenant concurrent RPC；
- 每 service create rate；
- request/response/message/event batch bytes；
- list page size；
- stream subscriber 数和队列；
- RPC/decode/store/provider deadline；
- safe retry-after。

超过边界返回 Busy/RateLimited/NOT_APPLIED，不能接收后静默丢弃。query/stop 与 create 使用独立
budget，防止过载时无法收敛。

## 10. GRPC-10：Health 与反射

gRPC health 分 service：

- NotServing：未注册、合同不兼容、store 不可用；
- Serving：read/stop 可用；
- create capability 通过 capability runtime state 单独表示 Active/Draining/Isolated。

生产默认关闭 reflection；仅诊断配置+mTLS admin scope 可开启。health 不返回 tenant、secret、
内部路径或完整配置。

## 11. 验收

- fake facade 与真实 EngineMediaFacade 运行同一 RPC contract。
- 每个 typed RPC 有成功、required field、unsupported、deadline、cancel、fencing、capacity 测试。
- mapper property test 覆盖边界数值、unknown enum、恶意字符串和 roundtrip。
- handler 无 engine 私有 import；架构测试阻止 tonic/prost 泄漏至 SDK/engine/modules。
- 所有 create 在 response loss/重复请求下至多一个有效资源。
