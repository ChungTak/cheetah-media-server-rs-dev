# 09 · 可重放事件流与对账

## 1. EVT-01：Domain Event Envelope

新增 runtime-neutral `ControlledMediaEvent`，header 固定包含：

```text
event_id
tenant_id
media_node_id / instance_id / instance_epoch
sequence
occurred_at
media_session_id / media_binding_id
resource kind / handle / generation
media_key
correlation_id
traceparent / tracestate
```

typed payload至少覆盖 resource state、stream online/offline、RTP timeout、proxy state、
record/snapshot/playback completion、processing result 和 node lifecycle。事件不携带 secret、
userinfo、内部路径或未脱敏 last error。

本地 in-process event bus 继续服务热路径；control plane subscriber 负责 enrichment 和 durable
append。append 失败不得阻塞媒体包处理，但必须使 event health degraded 并触发 reconciliation。

## 2. EVT-02：Sequence 与 event ID

- event ID 使用不可预测、全局唯一的 ID provider；
- sequence scope 固定为 `(media node instance epoch)`；
- sequence 在 SQLite transaction 中单调分配；
- 同资源状态重放保持同 event ID/sequence；
- node 新 instance epoch 可从 1 重新计数，cursor 同时携带 epoch；
- 乱序内部事件按实际 append 顺序发布，payload 保留 occurred_at。

## 3. EVT-03：Cursor

opaque cursor 内容：

```text
format version
node ID / instance epoch
last delivered sequence
tenant/filter digest
issued_at/expires_at
key ID
HMAC
```

cursor 不暴露 SQLite offset 或可猜 tenant。签名 key 通过受控配置轮换，保留验证前一 key 的
滚动窗口。filter/tenant/instance 不匹配返回 InvalidArgument/StaleOwner。

## 4. EVT-04：Subscribe 语义

请求包含 tenant、typed filters、resume cursor、max batch/bytes。服务端：

1. 验证 tenant/filter/cursor；
2. 从 `last sequence + 1` 按页读 durable journal；
3. 先追赶历史，再订阅 live append；
4. 每个 subscriber 使用独立有界队列；
5. 发送 event 及可作为下一次 resume 的 cursor；
6. 断开不影响媒体处理。

投递保证为至少一次：客户端在收到后保存 cursor，断线可能重复最后事件；同 event ID 必须
安全去重。不提供隐含 exactly-once 承诺。

## 5. EVT-05：Retention 与 Gap

retention 同时限制 age、rows、bytes。resume sequence 早于 retention floor 时：

- 发送唯一 typed Gap，包含 requested/first_available sequence、instance epoch 和
  `reconciliation_required=true`；
- 随后从 first available 继续；
- 不把 gap 当作普通 dropped count；
- metric/audit 记录 gap reason；
- signaling 必须启动本节点分页 query 对账。

cursor HMAC 错误不是 gap，直接拒绝。store/event corruption 返回 Unavailable/UNKNOWN 并关闭
该 stream，不跳过损坏记录。

## 6. EVT-06：慢消费者

- per subscriber queue/batch/bytes/idle deadline 有上限；
- queue 满先断开该 subscriber，并给出最后安全 cursor/ResourceExhausted；
- 不丢弃 journal 中的事件，不阻塞其他 subscriber；
- subscriber 总数受 capacity permit 控制；
- event stream cancellation 立即释放 queue/permit/read transaction。

## 7. REC-01：Reconciler 输入

以下条件触发：

- startup；
- event gap；
- lease/instance replacement；
- UNKNOWN idempotency；
- periodic low-rate scan；
- admin scoped request。

按 tenant/node/resource kind/非终态状态使用第 10 章 cursor query：

- signaling 有 binding，media 无资源；
- media 有 resource，signaling 重试查无结果；
- terminal control record 仍有 provider resource；
- owner/instance/generation 不一致；
- event append 缺失；
- orphan 无 binding。

媒体侧不访问 signaling 数据库。它只提供事实查询、typed cleanup 和事件；最终业务 desired
state 由 signaling 决定。

## 8. REC-02：Orphan 保护

- orphan 先标记并保留 configurable grace period；
- 通过 idempotency/binding/session metadata 再确认；
- 只接受 signaling 发起的 scoped typed Stop/Delete，或受保护 admin cleanup；
- 不提供跳过 tenant/fencing 的“清空全部”后门；
- cleanup 重复调用安全，NotFound 可作为已清理，但 stale/permission 不能。

## 9. 验收

- duplicate/order/resume/filter/cursor tamper/rotation/gap/slow subscriber。
- append-before-send 与 crash 后重放。
- live/历史切换无遗漏，允许边界重复。
- event loss 不影响媒体数据面。
- gap 能通过全量 cursor query 收敛。
- old instance event 不推进新 instance resource。
- retention 和 subscriber 压力不突破配置上限。
