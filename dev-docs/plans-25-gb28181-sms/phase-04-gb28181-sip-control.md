# Phase 04 — GB28181 SIP 控制面与主动拉流

- **状态**: 已完成
- **范围**: 新增 `cheetah-gb28181-core`、`cheetah-gb28181-driver-tokio`、`cheetah-gb28181-module`，实现标准 GB28181 SIP 控制面、主动拉流、被动收流和设备会话管理
- **完成标准**: 设备可完成注册、保活、Invite/BYE、主动拉流，被动收流路径通过 RTP module 落地到 engine

---

## 4.1 `cheetah-gb28181-core` Sans-I/O

新增 crate：

```text
crates/protocols/gb28181/core
```

职责：

- SIP message parse/render
- transaction/dialog 状态机
- REGISTER、401/407 digest challenge、MESSAGE、Keepalive
- INVITE、ACK、BYE、SDP offer/answer
- device/channel/session 状态事件输出

核心类型：

```rust
pub enum Gb28181CoreInput {
    SipMessage(SipMessage),
    Tick { now_ms: u64 },
    Command(Gb28181Command),
}

pub enum Gb28181Command {
    RegisterChallenge(GbDeviceId),
    StartInvite(GbInviteSpec),
    StopInvite(GbSessionId),
    StartTalk(GbTalkSpec),
    StopTalk(GbSessionId),
}

pub enum Gb28181CoreOutput {
    SendSip(SipSendAction),
    Event(Gb28181Event),
    Diagnostic(Gb28181Diagnostic),
    Timeout(GbSessionId),
}
```

---

## 4.2 SIP Driver

新增 crate：

```text
crates/protocols/gb28181/driver-tokio
```

职责：

- SIP UDP/TCP bind、recv、send
- message reassembly、header 限制、body 限制
- transaction timer、retransmission、cancellation
- 与 `gb28181-core` 的 command/event 通道

实现要求：

- 首版支持 UDP 和 TCP SIP
- 读写缓存和 message 大小有上限
- 处理 `Via`、`From`、`To`、`Call-ID`、`CSeq`、`Contact` 的基础一致性校验
- 鉴权失败、超时、dialog 关闭都要输出结构化 diagnostic

---

## 4.3 Module 会话编排

新增 crate：

```text
crates/protocols/gb28181/module
```

module manifest：

- `module_id`: `gb28181`
- `display_name`: `GB28181 Module`
- `config_namespace`: `gb28181`
- `routes_prefix`: `/api/v1/gb28181`
- capabilities: `HttpApi`、`BackgroundJob`、`Publish`、`Subscribe`

module 职责：

- 设备注册表和状态缓存
- channel 与 stream 的映射
- 主动拉流时通过内部 `RtpSessionService` 创建 RTP receive session
- 被动收流时通过内部 `RtpSessionService` 分配媒体端口并等待远端推流
- 设备保活和超时清理
- 语音对讲和收发会话编排

---

## 4.4 REST API 与主动拉流

兼容 SMS 路由：

```text
POST /api/v1/gb28181/recv/create
POST /api/v1/gb28181/recv/stop
POST /api/v1/gb28181/send/create
POST /api/v1/gb28181/send/stop
```

扩展标准控制路由：

```text
GET  /api/v1/gb28181/devices
POST /api/v1/gb28181/invite
POST /api/v1/gb28181/bye
POST /api/v1/gb28181/talk/start
POST /api/v1/gb28181/talk/stop
```

主动拉流规则：

1. `recv/create` 且 `active=true` 时，module 主动发起 Invite
2. 分配本地 RTP session、SSRC 和端口
3. 生成 SDP offer/answer，等待远端 ACK
4. 远端 RTP 到达后交给 RTP module 发布本地流
5. 超时或失败则发送 BYE/清理会话

被动收流规则：

1. `recv/create` 且 `active=false` 时，只开媒体端口或返回已开的 server 信息
2. 允许先返回本地 `ip/port/ssrc`，再等待上级平台或设备发流
3. 收到流后映射到指定 `app/stream` 或默认 `/live/{ssrc}`

---

## 4.5 测试

测试场景：

1. REGISTER 与 digest challenge 成功
2. Keepalive MESSAGE 正常更新设备在线状态
3. 主动 Invite 成功建立 RTP 收流
4. 主动 Invite 超时或拒绝时会话正确清理
5. 被动收流返回本地端口并等待远端媒体
6. `recv/stop`、`send/stop` 正确释放会话
7. device timeout 后状态转离线
8. RTP session 与 SIP dialog 生命周期一致

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-gb28181-core
cargo test -p cheetah-gb28181-core
cargo clippy -p cheetah-gb28181-driver-tokio
cargo test -p cheetah-gb28181-driver-tokio
cargo clippy -p cheetah-gb28181-module --tests
cargo test -p cheetah-gb28181-module
```
