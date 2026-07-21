# 06 · RTP core、driver 与传输鲁棒性

## 1. Core 状态机

RTP core 通过显式输入输出推进，不读取系统时间、不持有 socket、不执行 async：

```text
Input: Packet(bytes, source, now) / Timer(id, now) / SendFrame(frame, now) /
       UpdateBinding / Stop(reason)
Output: EmitFrame / SendDatagram / SendFramed / ScheduleTimer / CancelTimer /
        SourceChanged / Stats / Event / Close
```

| ID | 实施 | 完成证据 |
| --- | --- | --- |
| RTP-CORE-01 | 将 receiver/sender/talk 状态和 transition 显式化 | exhaustive state tests |
| RTP-CORE-02 | reorder/duplicate/loss/SSRC/PT state 按 track/session 有界隔离 | property tests |
| RTP-CORE-03 | source binding/rebind 使用具名 policy 和 injected time | spoof/NAT tests |
| RTP-CORE-04 | stop/timeout/BYE/format change 产生一致 terminal event | terminal matrix |
| RTP-CORE-05 | 将大 session 文件按 receive/send/rtcp/binding/stats 拆分 | module size review |

## 2. TCP framing 与恢复

- `Rfc4571` 读取两字节 network-order 长度；`DollarPrefixed` 校验 `$`、channel 与两字节长度。
- `AutoDetect` 仅在连接建立后的有限字节/帧窗口运行，成功后固定 framing，禁止逐帧切换。
- frame length、connection buffer、scan bytes、recovery attempts 和 consecutive errors 全部配置上限。
- resync 采用“两个一致 SSRC 位置 + 合理 RTP header/PS system header”等双证据；不能只搜索单个
  magic byte。
- 恢复成功记录 compat rule 与丢弃字节数；达到预算后关闭连接，不继续消耗 CPU/内存。
- 一个 TCP connection 上 RTP/RTCP channel 映射固定，未知 channel 按 profile 决定拒绝或记录。

## 3. Driver I/O

| ID | 实施 | 完成证据 |
| --- | --- | --- |
| DRV-01 | UDP receive/send active/passive 与 RTP/RTCP mux/separate ports | socket matrix |
| DRV-02 | TCP active connect/passive accept、2/4-byte framing、half-close | integration tests |
| DRV-03 | 每 session 独立 cancellation、bounded ingress/egress channel | slow peer/cancel tests |
| DRV-04 | timer、RTCP interval、idle timeout 通过 RuntimeApi/driver 驱动 | FakeClock + Tokio tests |
| DRV-05 | port lease 与 socket bind 原子关联，bind 失败立即归还 | exhaustion/race tests |
| DRV-06 | listener/session/peer/bytes 限额与 overload rejection | load tests |

driver 不决定 tenant、鉴权、publisher、record 或 stream mapping；这些由 module 传入已授权 binding。

## 4. 来源绑定与 NAT

默认 `Strict`：首个通过 SSRC/PT/session 校验的 packet 锁定 source IP:port，后续不同来源丢弃并
计数。`AllowValidatedRebind` 仅在全部条件成立时切换：

1. profile 明确允许；
2. 原来源超过配置 idle window 或控制面提供新 generation；
3. 新包满足 SSRC、payload、sequence/timestamp 连续性；
4. tenant/session binding 未变化；
5. rebind rate limit 未超限。

IP 改变默认不自动 rebind。切换生成审计事件，但日志仅记录脱敏/哈希后的 endpoint。

## 5. Backpressure 与多目标发送

- 单个慢接收者/发送目标不能阻塞其他 session 或 engine dispatcher。
- queue full policy 必须按媒体类型显式配置：视频优先丢弃至下一关键帧，音频按 bounded latency
  丢弃；控制/RTCP 不与大媒体队列共用。
- 多目标 sender 为每个 target 维护独立状态和结果；API 返回成功/失败目标集合。
- 连续 send error 达阈值后只关闭对应 target；全部 target 失败才终止聚合 session。
- drop、queue depth、send latency 和 target state 进入低基数 metrics/event。
