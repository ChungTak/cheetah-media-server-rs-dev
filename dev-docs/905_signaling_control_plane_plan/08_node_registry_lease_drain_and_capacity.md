# 08 · 节点注册、Lease、Drain 与容量

## 1. NODE-01：节点身份

配置区分：

- `node_id`：部署稳定 ID；
- `instance_id`：每次进程启动生成并持久到本次运行；
- `instance_epoch`：由 signaling registry 原子分配；
- gRPC control endpoint；
- network zone/region/labels；
- advertised media addresses；
- build version、contract range/checksum、capability generation。

不得用监听地址、主机名或进程 PID 推导稳定 node ID。mTLS SAN identity 必须与注册 node ID
匹配。

## 2. NODE-02：注册状态机

```text
Disabled
  -> Binding
  -> Registering
  -> Active
  -> Draining
  -> Isolated
  -> Deregistering
  -> Stopped
```

- engine/module/preflight 完成后注册；
- Register 返回 lease ID、instance epoch、TTL、heartbeat interval、cluster time、accepted version；
- response 持久化后才开放 create gate；
- 同 instance ID 在有效租约内重试返回同 epoch；
- 新 instance 替换旧 instance 时 registry 推进 epoch，旧 heartbeat/command 无效。

注册失败采用有界指数退避+jitter。未注册期间 gRPC read health 可诊断，mutation NotServing。

## 3. NODE-03：Heartbeat 与 load

Heartbeat 按 server 返回 interval 调度，不硬编码本地固定周期，携带：

```text
lease ID / node ID / instance ID / instance epoch
accepted contract version / descriptor checksum / capability generation
session/port/bandwidth/worker/store/event usage
normalized CPU/load
health/degraded reasons
drain state
```

load 来自实际 capacity permits 和运行指标，不使用手工配置的虚假值。heartbeat timeout 小于
lease TTL，并保留至少一次重试窗口。旧 lease/epoch response 不更新当前状态。

## 4. NODE-04：Lease loss 与自我隔离

registry 暂不可达时：

- lease deadline 前继续当前状态并告警；
- deadline 到达原子切换 Isolated；
- 拒绝 create/open/start/connect；
- 允许 tenant-scoped Get/List/Stop/Delete 和必要的 takeover cleanup；
- 现有媒体数据面继续运行，不因控制面短暂故障主动断流；
- 持续有界重新注册。

重新注册同 epoch 可恢复 Active；不同 epoch 按第 5 章把旧资源标记 NeedsVerification，等待
signaling 对账，不能无限接受旧 owner 命令。

## 5. NODE-05：Drain 与 shutdown

Drain 来源可以是 registry response 或受保护 admin 命令。进入 Draining 后：

- 拒绝所有新资源和可能扩容的 update；
- 允许 read、stop/delete 和明确不分配资源的 control；
- heartbeat 持续上报剩余资源；
- 按配置等待自然结束或 signaling 迁移；
- hard deadline 到达时只执行 typed cleanup，不直接清空内部 map。

shutdown 先 drain，随后 bounded deregister。deregister 失败记录 audit/metric 后继续关机；不得
超过 shutdown 总 deadline。

## 6. CAP-01：统一容量 permit

在 SDK 暴露 runtime-neutral `MediaCapacityApi`：

```text
acquire(CapacityRequest) -> CapacityPermit
snapshot() -> CapacitySnapshot
update_limits(...)
set_node_gate(...)
```

`CapacityRequest` 至少含 session、port、bandwidth、worker、blocking job、file task、event
subscriber 等维度。permit RAII 释放并可关联 resource handle；不得用先 count 后 allocate 的
TOCTOU 模式。

所有 create 在同一 guard 流程原子获得所需 permit，再分配端口/任务；失败释放全部 permit。
provider 私有上限继续作为第二层保护，但 capability/heartbeat 的 hard limit 必须与中央
capacity snapshot 一致。

## 7. CAP-02：过载语义

- hard limit：Busy/RateLimited + retry hint + NOT_APPLIED；
- queue 满：不创建 operation/resource；
- store/event hard limit：关闭 create/subscribe，保留 query/stop；
- 不允许无限 channel、subscriber、cursor、event retention、idempotency row 或 pending RPC；
- 低基数 metric 按 operation/reason 统计，不以 tenant/node/resource 作为无限 label。

## 8. 验收

- register/retry/replacement/old heartbeat/lease expiry/drain/deregister 状态机确定性测试。
- FakeClock 驱动，不用真实 sleep。
- registry outage 不影响数据面且 lease 后 create 被拒绝。
- capacity 并发 race 不超卖。
- drain 时 query/stop 成功、create 明确拒绝。
- load heartbeat 与实际 permit 数一致。
- shutdown 在 deadline 内完成且无资源泄漏。
