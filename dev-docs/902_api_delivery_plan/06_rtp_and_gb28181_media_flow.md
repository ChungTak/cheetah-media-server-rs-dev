# 06 · RTP Provider 与 GB28181 媒体闭环

## 1. 当前问题

当前 domain RTP provider 直接向 driver 发送 `CreateServer/CreateClient`：

- receiver request 的 IP/port 没有决定实际 socket。
- command 没有成功确认，session 可能在实际失败前显示 Listening/Created。
- active receiver 尚未实现。
- sender/talk 没有复用 module 的 engine subscriber 与 egress worker。
- update 只返回原 session，大部分字段 Unsupported。

完成标准不是“session map 中出现记录”，而是实际 socket 收发媒体并驱动 engine。

## 2. 单一 RTP Session Orchestrator

在 RTP module 内抽取 `RtpSessionOrchestrator`，现有 module HTTP route、native adapter 和 ZLM adapter 全部调用它。orchestrator 负责：

- 端口分配与 socket bind。
- driver command 和成功/失败确认。
- ingress worker、egress worker、timeout/check timer。
- engine publisher/subscriber lease。
- session directory、domain RtpSession 和 module 内状态同步。
- stop/restart/cleanup。

adapter 不直接构造 driver command。

## 3. Receiver

端口策略：

- `port=Some(n)`：尝试绑定指定端口；失败返回 Conflict/Unavailable。
- `port=None` 或 `0`：从配置端口池分配，返回实际端口。
- `ip` 缺省使用配置监听地址；非本机地址拒绝。
- RTCP 根据 request 决定 mux 或相邻/独立端口。

支持 UDP passive、TCP passive、TCP active 和 multiplex。`connect_rtp_receiver` 在 Created/Listening session 上发起主动连接，并更新 remote endpoint/state。

收到 PS/TS/ES/raw 后继续复用现有 core/codec 管线，发布 `AVFrame + TrackInfo`。首帧成功发布后 session 进入 Bound/Connected，媒体 online 事件只发布一次。

## 4. Sender 与 talk

创建 sender 时：

1. 校验目标 endpoint、SSRC、payload、transport。
2. 打开 MediaKey subscriber。
3. driver 成功创建 client/server transport。
4. 启动有界 egress worker，将 AVFrame 交给 RTP packetizer。
5. 首包成功后 session 进入 Connected。

Passive sender 必须等待对端连接；Talk 使用 SendRecv，并明确音频 track filter。停止 sender 同时关闭 subscriber 和 driver session。

## 5. Update、check 与事件

- SSRC/payload 只在安全状态修改；需要重建时返回明确结果或新 generation。
- pause check 只暂停 timeout 健康检查，不停止收包。
- resume 恢复 deadline，不能立即误判 timeout。
- timeout、driver error、remote close 发布 RtpSessionTimeout/SessionClosed。
- session store 上限、分页、直接 lookup 保持可测。

## 6. GB28181 生产 contract

测试不实现 SIP，只模拟信令已完成后的媒体动作：

1. 调用 native HTTP 或 Rust SDK 打开 RTP receiver。
2. 测试发送端向返回端口发送包含 H264/AAC 或 G711 的 PS/RTP。
3. 等待 MediaKey online，校验 TrackInfo 和至少一个 AVFrame。
4. 查询可用播放 URL并请求关键帧。
5. 启动/停止录制并收到 RecordCompleted。
6. 创建 RTP sender，测试接收端验证实际 RTP packet。
7. talk 模式验证音频双向媒体。
8. stop/BYE 等价调用关闭 session，资源和端口释放。

覆盖 UDP、TCP passive；TCP active 和回放按第二批用例执行。

## 7. 任务与验收

| ID | 任务 | DoD |
| --- | --- | --- |
| S5-T1 | orchestrator 抽取 | 三个入口共用 |
| S5-T2 | port allocator/bind ack | 请求端口与返回端口真实一致 |
| S5-T3 | receiver 全模式 | UDP/TCP 收包进入 engine |
| S5-T4 | sender egress | 对端收到真实媒体 RTP |
| S5-T5 | talk | 双向音频和关闭 |
| S5-T6 | update/check/timeout | 状态和事件正确 |
| S5-T7 | GB production contract | 非 fake E2E 通过 |

```bash
cargo test -p cheetah-rtp-core
cargo test -p cheetah-rtp-driver-tokio
cargo test -p cheetah-rtp-module
cargo test -p cheetah-sdk gb28181
```

