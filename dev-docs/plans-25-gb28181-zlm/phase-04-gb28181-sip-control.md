# Phase 04 — GB28181 SIP 控制面与主动拉流

- **状态**: 已完成
- **范围**: 新增 `cheetah-gb28181-core`、`cheetah-gb28181-driver-tokio`、`cheetah-gb28181-module`，实现标准 GB28181 SIP 控制面、主动拉流、被动收流和设备会话管理
- **完成标准**: 设备可完成注册、保活、Invite/BYE、主动拉流，被动收流路径通过 RTP session service 落地到 engine

---

## 4.1 `cheetah-gb28181-core` Sans-I/O

职责：

- SIP message parse/render
- transaction/dialog 状态机
- REGISTER、digest challenge、MESSAGE、Keepalive
- INVITE、ACK、BYE、SDP offer/answer
- device/channel/session 状态事件输出

---

## 4.2 SIP Driver

职责：

- SIP UDP/TCP bind、recv、send
- message reassembly、header/body 限制
- transaction timer、retransmission、cancellation
- 与 `gb28181-core` 的 command/event 通道

要求：

- 支持 UDP 和 TCP SIP
- 读写缓存有上限
- 对 `Via`、`From`、`To`、`Call-ID`、`CSeq`、`Contact` 做基础一致性校验

---

## 4.3 Module 会话编排

module 职责：

- 设备注册表和在线状态
- channel 与 stream 的映射
- 主动拉流时通过 `RtpSessionService` 创建 RTP receive session
- 被动收流时通过 `RtpSessionService` 分配媒体端口并等待远端推流
- 设备保活和超时清理
- 语音对讲和会话编排

---

## 4.4 REST API

兼容路由：

```text
POST /api/v1/gb28181/recv/create
POST /api/v1/gb28181/recv/stop
POST /api/v1/gb28181/send/create
POST /api/v1/gb28181/send/stop
```

扩展控制路由：

```text
GET  /api/v1/gb28181/devices
POST /api/v1/gb28181/invite
POST /api/v1/gb28181/bye
POST /api/v1/gb28181/talk/start
POST /api/v1/gb28181/talk/stop
```

主动拉流规则：

1. `recv/create` 且 `active=true` 时主动发起 Invite
2. 分配本地 RTP session、SSRC 和端口
3. 完成 SDP offer/answer
4. 远端 RTP 到达后发布本地流
5. 超时或失败时清理 dialog 和 media session

---

## 4.5 测试

测试场景：

1. REGISTER 与 digest challenge 成功
2. Keepalive 更新设备在线状态
3. 主动 Invite 成功建立 RTP 收流
4. 主动 Invite 超时或拒绝时会话清理
5. 被动收流返回本地端口并等待远端媒体
6. `recv/stop`、`send/stop` 正确释放会话
7. device timeout 后状态转离线

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
