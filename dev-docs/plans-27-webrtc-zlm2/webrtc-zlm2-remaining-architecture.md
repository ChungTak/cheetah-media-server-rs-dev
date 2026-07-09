# WebRTC 剩余架构设计（ZLM2）

## 架构目标

本补充架构只处理三类剩余能力：

1. 多线程 driver shard：把一个 WebRTC driver 从单任务模型拆成前端 I/O + 多个 session owner shard。
2. ZLM 风格 P2P signaling：让 `signaling_protocols=1` 的 URL 走 WebSocket room/peer 信令。
3. 外部互操作测试基础设施：把 ignored scaffold 变成可复现的实体测试矩阵。

核心不变：

- `cheetah-webrtc-core` 继续包装 `str0m`，保持 Sans-I/O。
- driver 负责 socket、framing、timer、route、migration、backpressure。
- module 负责 HTTP/P2P signaling、engine bridge、job lifecycle、鉴权和资源上界。

## Driver shard 架构

目标拓扑：

```text
UDP socket / TCP listener
        |
        v
WebRtcIoFront
        |
        +-- route directory lookup by remote addr
        +-- route directory lookup by STUN ufrag
        +-- session id -> shard lookup for commands
        |
        v
WebRtcShard[N]
        |
        +-- WebRtcCore
        +-- RouteTable
        +-- Timer queue
        +-- TcpWriterRegistry subset
        +-- session stats
```

关键原则：

- 一个 session 只有一个 owner shard。
- `WebRtcCore` 不跨线程共享；每个 shard 自己持有一个 core。
- 前端 I/O 不直接操作 core，只负责把 datagram/frame/command 投递给 owner shard。
- route directory 只保存路由元数据，不保存协议状态。
- shard 内仍复用现有 route table、migration stale TTL、TCP writer、backpressure 逻辑。

## Route directory

全局 route directory 负责：

- `session_id -> shard_id`
- `remote_addr -> shard_id`
- `ice_ufrag -> shard_id`
- `tcp_remote_addr -> shard_id`
- `stale_remote_addr -> shard_id`

更新来源：

- 创建 session 后注册 session id。
- core 生成 local description 后注册 ICE ufrag。
- 收到可接受 remote addr 后注册 route。
- 连接迁移后更新 active/stale route。
- session 关闭后移除所有关联项。

一致性要求：

- route directory 更新必须由 owner shard 发起或确认。
- 前端只读 route directory；未命中时发 `UnroutedPacket`。
- route 迁移不能把同一个 remote addr 同时绑定给两个 shard。
- stale route 只允许投递给原 shard，TTL 过期后删除。

## P2P signaling 架构

ZLM 风格 P2P 有两个角色：

- signaling server：维护 room、peer、candidate、answer 转发。
- signaling peer/client：连接 signaling server，注册临时 room，向目标 room check-in，交换 SDP/candidate。

本项目建议模块：

```text
cheetah-webrtc-module
  p2p/
    message.rs      # wire schema
    room.rs         # local room keeper registry
    client.rs       # outbound WebSocket signaling client
    server.rs       # optional inbound signaling server adapter
    job.rs          # pull/push P2P job orchestration
```

公共行为：

- `room_keeper/add`：向远端 signaling server 注册本地 room。
- `room_keeper/remove`：注销。
- `room_keeper/list`：列出本地 keeper。
- `rooms/list`：列出远端或本地 rooms。
- `client pull/push`：解析 `webrtc://host:port/app/stream?signaling_protocols=1&peer_room_id=...`，走 P2P signaling。

P2P 信令消息必须是明确 schema，不直接把任意 JSON 透传进 core。

## P2P 数据流

### Pull

```text
client pull/start
  -> parse ZLM WebRTC URL
  -> SSRF / auth / resource limit
  -> create local WebRtc session as offerer
  -> P2P signaling check-in target room
  -> send offer + transport id
  -> receive answer
  -> exchange candidates
  -> ICE/DTLS connected
  -> remote media enters engine as publisher
```

### Push

```text
client push/start
  -> subscribe local engine stream
  -> create local WebRtc session as offerer
  -> P2P signaling check-in target room
  -> send offer
  -> receive answer/candidates
  -> send local stream frames through WebRTC
```

## 外部互操作测试架构

测试分三层：

1. **Unit / integration**：不依赖外部进程，验证 parser、schema、state machine、route directory。
2. **Local entity**：测试代码启动本地 helper 或 docker container，例如 Pion test peer。
3. **Manual / CI entity**：由环境变量指向已启动的 ZLMediaKit、Janus、browser grid、GStreamer。

统一约定：

- 所有外部测试 `#[ignore]`。
- 缺少 env var 时 skip，不失败。
- 设置 env var 后必须执行真实 SDP exchange 或媒体面验证。
- artifact 写入 `target/webrtc-interop/<test-name>/`。
- artifact 至少包含 request/response SDP、日志、session stats、失败原因。

## 安全与资源边界

driver shard：

- shard command queue 有上界。
- route directory entry 有上界。
- shard 数量有上界。
- route migration 有 hard cap。

P2P signaling：

- URL 默认执行 SSRF 防护。
- room id、peer id、transport id、candidate、SDP 长度有限制。
- WebSocket message 大小有限制。
- 每个 keeper、room、peer、job 都有超时。
- 活跃 job 同 stream key 冲突返回 409；终态 job 可覆盖。

互操作测试：

- 外部 URL 必须由环境变量显式给出。
- 默认不访问公网。
- 测试容器端口、日志目录和清理行为固定。

