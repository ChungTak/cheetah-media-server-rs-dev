# 08 · 信令所有权与 legacy SIP

## 1. 生产边界

外部 `cheetah-signaling` 负责 SIP transport/transaction/dialog、REGISTER/Digest、MANSCDP/XML、
device/channel/catalog/record/alarm/location 状态和调度。Cheetah media 只消费固定媒体命令并报告
媒体资源状态，不持久化设备目录或信令事务。

| Owner | SIP listener | MANSCDP/device DB | RTP/media | 使用场景 |
| --- | --- | --- | --- | --- |
| `local` | 开启 legacy listener | 仅现有迁移所需最小状态 | 本仓 typed lifecycle | 单机迁移/兼容 |
| `signaling` | 关闭 | 外部 signaling | 本仓 typed lifecycle | 生产集群 |

配置同时请求两个 owner、signaling owner 无固定合同、或 lease 未取得时必须 fail closed。

## 2. 外部控制合同

等待 CT-01 固定合同后，adapter 将 wire DTO 映射到 04 的 Domain request。合同至少能表达：

- open receiver/sender/talk、update、stop、get/list；
- tenant、MediaSession/Binding、SSRC、local/remote endpoint、transport/framing/container/payload；
- owner epoch、node instance epoch、expected generation、deadline、idempotency key；
- effect outcome、resource ref、resolved endpoint、state 和 sanitized error；
- resource/event sequence、resume cursor 和 reconciliation identity。

媒体仓不自行增加 Proto 字段；缺能力时保持 P4 BLOCKED，并在 CT issue 中提出 domain requirement。

## 3. Legacy SIP 安全加固范围

legacy 路径只补到安全迁移所需的互操作水平，不扩建完整设备平台：

| ID | 实施 | 完成证据 |
| --- | --- | --- |
| SIG-LEG-01 | REGISTER Digest 验证，随机 nonce、expiry、realm、qop/nc/cnonce 与 replay cache | auth matrix |
| SIG-LEG-02 | INVITE 的 100/180/2xx/failure、ACK、CANCEL、BYE 和 transaction timer | FakeClock/dialog tests |
| SIG-LEG-03 | 解析 answer SDP、Contact、To-tag、route set，校验 media/SSRC/PT | device fixtures |
| SIG-LEG-04 | MESSAGE body 只做迁移必须的 keepalive/status hook；完整 XML 归 signaling | ownership test |
| SIG-LEG-05 | UDP/TCP parser hard limits、compact headers、重复头、CRLF/LF/CR | fuzz/limit tests |
| SIG-LEG-06 | local session 使用同一 RtpSessionApi/admission/resource index | lifecycle E2E |

当前只处理 REGISTER/MESSAGE/BYE、REGISTER 直接 200、INVITE 2xx 后不 ACK 的行为必须在 capability
matrix 中标为不完整，直到上述测试通过。

## 4. Parser 限制

配置并强制 max start-line、header bytes/count/line、body bytes、TCP connection buffer、pipeline
messages、connections per source 和 parse errors。`Content-Length`：

- TCP 有 body 的方法缺失长度时按 profile 明确拒绝或只消费当前完整 datagram，禁止猜测跨消息边界；
- UDP 可使用 datagram 边界，但声明长度必须与可用 body 一致或进入具名 compat rule；
- 超长/负数/溢出/冲突重复长度立即拒绝并关闭有风险的 TCP connection。

日志不输出 Authorization、完整 XML、SDP 中的凭据、原始号码或 nonce。

## 5. SDP 与 GB 兼容

- 生成/解析 Subject、`y=` SSRC、live/playback/download 时间范围、TCP `setup/connection`、方向、
  media address、PT/rtpmap/fmtp。
- answer 必须与 request 的 tenant/session/transport/media 兼容；不接受意外回环/任意地址覆盖已授权
  endpoint。
- 厂商 `y=` 位数、空格、大小写和非标准 PT 通过 profile 规范化，保留原始规则 ID。
- live、playback、download 的语义由 signaling command 明确传入，media 不根据端口或 URL 猜测。

## 6. 迁移与回滚

固定顺序：register-only/shadow observation → drain local creates → snapshot/query reconcile → acquire
signaling owner lease → disable local listener → canary mutations → full rollout。回滚必须先停止 signaling
新建、drain/reconcile、释放 owner，再启用 local；禁止在同一资源上热切双写。
