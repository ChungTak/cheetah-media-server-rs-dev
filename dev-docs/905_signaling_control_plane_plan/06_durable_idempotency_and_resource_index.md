# 06 · 持久幂等与资源索引

## 1. STORE-01：Store trait 与 SQLite

`cheetah-media-control-plane` 定义 runtime-neutral store traits，生产默认实现使用每媒体进程
独立 SQLite：

- WAL；
- foreign keys；
- schema migration version；
- configurable busy timeout；
- bounded connection/operation concurrency；
- 所有 I/O 通过 `RuntimeApi::spawn_blocking`；
- startup integrity/migration 失败时 mutation gate 保持关闭。

SQLite 不是跨节点共享数据库。store path 必须位于受控 data root，禁止请求指定路径。

## 2. 数据模型

### `control_meta`

```text
schema_version
stable_node_id
process_instance_id
last_accepted_instance_epoch
last_contract_version
last_descriptor_checksum
event_sequence
```

### `idempotency_records`

```text
tenant_id
operation_kind
idempotency_key
canonical_digest
state = PREPARED | COMPLETED | FAILED | UNKNOWN
resource_kind/resource_handle
effect_outcome
serialized_domain_result
safe_error
created_at/updated_at/expires_at
attempt_count
```

唯一键固定为 `(tenant_id, operation_kind, idempotency_key)`。

### `controlled_resources`

```text
tenant_id
resource_kind
resource_handle
media_session_id
media_binding_id
media_key
idempotency scope/digest
accepted_owner_epoch
media_node_id/instance_id/instance_epoch
generation
state
safe_last_error
created_at/updated_at/terminal_at
```

唯一键固定为 `(tenant_id, resource_kind, resource_handle)`；另为 binding/session/idempotency
建立索引。

### `media_events`

保存 domain event envelope，而不是 prost bytes：

```text
instance_epoch
sequence
event_id
tenant_id
resource reference
occurred_at
event_kind
serialized domain payload
correlation/trace
expires_at
```

不得保存 AVFrame、packet、SDP 原文、secret、Authorization、URL userinfo 或内部路径。

## 3. IDEM-01：Canonical request

canonical digest 使用版本化、确定性的 domain 表示和 SHA-256：

- map key 排序；
- enum 使用稳定 domain 名；
- URL 先移除 userinfo、规范 host/port/path；
- 包含 tenant、operation kind、目标 resource、MediaSession/Binding 和业务参数；
- 不包含 request/message/correlation ID、trace、deadline、重试 attempt；
- credential 只包含 opaque handle，不包含解析后的 secret；
- canonical schema version 写入记录，升级时保留旧版本验证器。

相同 key 不同 digest 始终 Conflict，即使旧资源已经停止。

## 4. IDEM-02：执行状态机

```text
begin
  -> lookup key
     completed/same digest -> replay first domain result
     failed definitive     -> replay first error
     different digest      -> Conflict
     unknown/prepared      -> reconcile, never blind recreate
     absent                -> insert PREPARED
  -> execute side effect
  -> persist resource + result/outcome in one SQLite transaction
  -> send response
```

规则：

- success response 只能在 COMPLETED 持久化后发送；
- `NOT_APPLIED + retryable` 不作为永久 terminal，可按有界 attempt 重新进入 PREPARED；
- `NOT_APPLIED + non-retryable` 缓存并重放；
- `APPLIED` 缓存 resource ref 和第一次结果；
- `UNKNOWN` 阻止自动执行，直到 query/reconciliation 把它收敛为 completed/failed/gone；
- TTL 至少覆盖 signaling resource lifecycle 与最大重试窗口；非终态资源的幂等记录不得过期。

不得再用 idempotency key 明文作为 task/session/proxy ID。资源 ID 由 ID provider 生成并记录。

## 5. STORE-02：副作用窗口

针对崩溃窗口：

| 窗口 | 恢复规则 |
| --- | --- |
| PREPARED 前 | 无记录、无副作用，可重试 |
| PREPARED 后/副作用前 | query 无资源后可继续原 attempt |
| 副作用后/结果持久化前 | 按 tenant+binding+idempotency metadata 查询 provider |
| 结果持久化后/响应前 | 重试直接重放 |
| response 后/event 前 | resource/result 是权威，event journal 补发 |

所有 provider 创建结果必须能通过 idempotency、binding 或 handle 被查询。若 provider 无法证明
副作用是否存在，记录 UNKNOWN，交给 reconciler；禁止创建第二个资源“试试看”。

## 6. STORE-03：资源注册与终态

控制面在 create 前保存 intent，create 后登记 resource：

- resource kind 和 handle 不可变；
- owner epoch 可按 fencing CAS 推进；
- generation 单调增加；
- state 只允许显式状态图转换；
- stop/delete 记录终态后保留 tombstone 至 retention；
- NotFound 只在 tenant/ref 正确且 provider 确认不存在时作为补偿完成；
- permission/stale owner 不能转换为幂等成功。

本地 adapter 创建的 system tenant 资源不写入 cluster tenant 索引。

## 7. STORE-04：启动恢复

启动时在 gRPC Serving 前：

1. 校验 node/process/contract metadata。
2. 扫描 PREPARED/UNKNOWN/non-terminal resources，按页调用 typed provider Get/List。
3. 匹配 resource metadata、provider 实际 generation/state。
4. 对不存在资源写 Gone/NOT_APPLIED 或安全失败。
5. 对仍存在资源恢复 APPLIED/COMPLETED，并补写缺失事件。
6. 对歧义记录保持 UNKNOWN，节点可注册但相关 capability health 为 degraded。

恢复有总时间/每页/并发上限。超时不得静默跳过；create gate 保持关闭或仅关闭受影响 operation。

## 8. Retention 与清理

- terminal idempotency/resource tombstone 按配置 age + row count 双上限清理；
- 未终态、UNKNOWN、仍被 active binding 引用的记录不得删除；
- event retention 独立管理；
- vacuum/checkpoint 只在冷路径执行；
- store size 超 hard limit 时拒绝 create/subscribe，query/stop 保留；
- 清理 metrics/audit 不使用 tenant/resource ID 作为无限 label。

## 9. 验收

- 同 key 并发只有一次有效副作用。
- 同 key 不同 digest Conflict。
- response loss 后重放完全相同的 domain/wire 结果。
- 每个 crash injection window 恢复后至多一个有效资源。
- SQLite restart、migration、WAL recovery、disk full、busy/corrupt 均有确定结果。
- store 文件与日志扫描不包含 secret/userinfo/internal path。
- retention 不删除 active/unknown 记录。
