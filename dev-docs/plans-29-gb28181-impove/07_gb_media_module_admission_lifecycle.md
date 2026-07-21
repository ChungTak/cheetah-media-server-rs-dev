# 07 · GB module 准入与生命周期

## 1. 目标状态机

```text
Requested
 -> Validated
 -> Authorized
 -> CapacityReserved
 -> PortOrPublisherReserved
 -> TransportReady
 -> MediaBound
 -> Active
 -> Draining
 -> Stopped | Failed
```

状态只允许单向推进；每个 state transition 携带 generation 和 effect outcome。`MediaBound` 表示
外部请求提供的 stream/session 与已创建 RTP transport 已完成结构化绑定，不涉及任何信令事务。

## 2. Admission 与资源顺序

| ID | 实施 | 完成证据 |
| --- | --- | --- |
| ADM-01 | 所有 publish/play/proxy/RTP open/talk/playback/download 先 authorize | Deny zero-allocation tests |
| ADM-02 | deadline、fence、drain、capacity 在 socket/task 前验证 | old owner/no effect tests |
| LIFE-01 | capacity/port/socket/task/publisher 用 rollback guard | injected failure at every step |
| LIFE-02 | first frame 前完成 publish lease，失败不向 engine 发布 | competing publisher test |
| LIFE-03 | idempotency 记录 canonical request digest 和 final resource | response-loss/restart tests |
| LIFE-04 | stop 统一清理 transport、timer、task、lease、resource/event | leak counter tests |
| LIFE-05 | module restart 由基础层 create→init→start，恢复后 reconcile | restart E2E |

不得保留“先 bind RTP、后调用鉴权”的 legacy 顺序。需要返回监听端口供第三方继续协商时，也必须
在 authorize 后租用；外部请求取消、超时或后续 connect 失败由 guard/显式 stop 归还。

## 3. GB 与 RTP module 解耦

- 删除 `call_rtp_service` 类内部 HTTP/JSON 调用，注入 `Arc<dyn RtpSessionApi>` 或 SDK service
  handle。
- REST 只作为外部兼容 adapter：解析 ZLM/SMS/ABL aliases 后构造 Domain request，不直接操作
  socket/map。
- REST success 必须来自 typed outcome；stop/send/talk 错误不能统一返回成功。
- 用户输入端口使用 checked conversion 和范围校验，不允许 `u64 as u16` 截断。
- module 内 session map 只作 runtime index；durable truth 在 905 controlled resource store。

## 4. Publisher 与 StreamKey

- 同一 `StreamKey` 默认单发布者，receiver 建立不代表已取得 publish lease。
- 在首帧可发布前申请带 tenant/session/generation 的 lease；竞争失败终止新 receiver 并清理。
- 临时 pre-auth/pre-track buffer 只有在 admission PASS 后启用，同时限制 frames、bytes、age。
- codec 探测完成前不得创建错误 TrackInfo；探测失败返回明确 UnsupportedPayload。
- 派生/回放/下载输出使用独立 StreamKey，不覆盖源流。

## 5. 幂等和并发

- 同 idempotency key + 同 canonical digest 返回首次结果；不同 digest 返回 Conflict。
- create 与 stop 并发由 generation CAS 决定，stop 已胜出后迟到的 bind/task 不得提交。
- 多个重复 open/connect/update 请求只能推进同一 session，不得再租端口或再 spawn worker。
- shutdown/drain 拒绝新 create，允许 get/list/stop；达到 deadline 后报告未清理资源，不静默退出。

## 6. 配置

配置使用具名结构，至少包含 port pool、session limits、buffer limits、idle/connect timeout、source
binding 和 media compat profile。固定端口只允许显式 single-session/test 配置，生产默认
使用有界 port range。配置应用返回 `Applied` 或 `ModuleRestartRequired`，module 不私建重启流程。
