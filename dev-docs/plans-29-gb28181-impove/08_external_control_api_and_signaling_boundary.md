# 08 · 第三方控制接口与信令边界

## 1. 绝对职责边界

本项目是流媒体服务器，只负责媒体资源、传输、封装、时间线、发布/订阅和处理，不负责任何
GB28181 信令实现。

第三方系统负责：

- SIP UDP/TCP listener、连接管理、报文解析与序列化；
- REGISTER、Digest、事务、dialog、路由、重传、ACK/CANCEL/BYE；
- SDP 生成/解析与 offer/answer；
- MANSCDP/XML、Catalog、RecordInfo、Alarm、MobilePosition、Broadcast 等业务；
- 设备、通道、注册、心跳、目录、录像和信令状态数据库。

本项目不得出现上述能力的新实现。现有相关代码只作为待删除历史代码，不进行功能补全或兼容加固。

## 2. 第三方传入的媒体参数

第三方完成信令和 SDP 协商后，将以下结构化字段映射为 `RtpSessionApi` 请求：

- tenant、external session ID、StreamKey、media purpose；
- receive/send/talk direction；
- UDP/TCP、active/passive、local/remote endpoint、RTP/RTCP mux；
- RFC4571/`$` framing、PS/TS/ES container；
- SSRC、payload type、codec、clock rate、channels、fmtp 的媒体等价参数；
- live/playback/download 的 start/end、source binding 和 rate；
- compatibility profile、deadline、idempotency key、owner epoch 和 expected generation。

接口不得包含 raw SIP、raw SDP、XML body、Digest、Call-ID/CSeq/Via/Contact/Route 或设备目录 DTO。
如第三方需要保留这些字段，应保存在第三方自身 correlation state，仅向本项目传递 opaque、受限长度
的 external session ID/correlation ID。

## 3. 对外媒体 API

| ID | 接口 | 语义 | 完成证据 |
| --- | --- | --- | --- |
| EXT-01 | `open_receiver` | 鉴权后分配接收端口/transport，返回 resolved media endpoint | contract + leak tests |
| EXT-02 | `open_sender` | 绑定已存在媒体源并连接/等待远端媒体端点 | target matrix |
| EXT-03 | `open_talk` | 创建结构化音频 RTP 会话 | codec/device fixtures |
| EXT-04 | `update_session` | 更新远端 endpoint、binding 或状态，必须带 expected generation | fencing tests |
| EXT-05 | `stop_session` | 幂等停止并返回真实 cleanup outcome | failure matrix |
| EXT-06 | `get/list` | 查询媒体状态、resolved binding、统计和错误 | pagination/query tests |
| EXT-07 | media events | session/track/timeout/loss/source/cleanup 事件，支持 replay/gap | event tests |

HTTP、gRPC、C API 或应用内 trait 都只是上述 Domain API 的 adapter。REST 中为 ZLM/SMS/ABL 保留的
字段别名只能做媒体字段规范化，不能引入信令动作或隐式触发 SIP 请求。

## 4. 两阶段调用

当第三方必须先取得本地端口再完成远端协商时，使用明确的两阶段媒体流程：

1. `open_receiver`：完成 admission/capacity/port/socket，返回本地媒体 endpoint 和 generation；
2. 第三方自行完成信令；
3. `update_session`/`connect`：提交协商后的远端 endpoint/PT/SSRC 等结构化参数；
4. 协商失败时第三方调用 `stop_session`；超过 connect deadline 时本项目自动清理。

第一阶段资源必须有短期、可配置、可观测的 connect deadline，避免第三方崩溃产生永久端口占用。
重试通过 idempotency 找回同一 session，不能再次分配。

## 5. 旧信令代码退出

| ID | 实施 | 完成证据 |
| --- | --- | --- |
| RMV-01 | 盘点 GB crate 中 SIP/SDP/XML/listener/transaction/auth 的文件、feature、路由和依赖 | inventory |
| RMV-02 | 先将调用方迁移到 typed media API，停止新增旧接口调用 | workspace compile |
| RMV-03 | 删除生产装配、配置、listener、REST 信令路由和 runtime task | process/socket inspection |
| RMV-04 | 删除 SIP/SDP/XML core/parser/session/auth 及只服务它们的测试/依赖 | rg + cargo tree |
| RMV-05 | 更新 SystemArchitecture、README、配置和 capability matrix | documentation review |

如果第三方仍需过渡期，兼容 adapter 必须部署在仓库外或独立进程，通过公开媒体 API 调用本项目；
不得以迁移为由保留本项目内的信令 listener。

## 6. 边界验收

- 运行制品不监听 SIP 标准/配置端口，不创建 SIP timer/transaction/dialog task。
- `rg` 与依赖检查确认生产源码无 SIP/MANSCDP/SDP/XML parser/auth 实现和对应配置。
- 公开接口文档只出现结构化媒体参数；任何 raw signaling payload 输入均返回 InvalidArgument。
- 第三方 simulator 仅调用 media API 即可完成 live receive/send、talk、playback 和 download 数据面。
- 第三方超时、断连、重复请求、乱序 update 和进程崩溃都能通过 deadline/idempotency/query/stop
  收敛，不要求本项目理解其信令状态。
