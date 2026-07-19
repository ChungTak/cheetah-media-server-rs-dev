# 05 · 请求上下文、Fencing 与错误

## 1. CTX-01：强类型标识

在 `cheetah-media-api` 增加强类型 newtype，禁止在公共 port 间传裸 String：

```text
TenantId
MessageId
MediaNodeId
MediaNodeInstanceId
MediaNodeInstanceEpoch(u64)
OwnerEpoch(u64)
OperationId
OperationStepId
MediaSessionId
MediaBindingId
CredentialHandle
ResourceGeneration(u64)
```

构造器拒绝空值、超长值、控制字符和非规范 UUID（合同规定 UUID 的类型）。Debug/Display
不得输出 credential 内容。wire mapper 负责 Proto UUID 与 domain newtype 的显式转换。

## 2. CTX-02：MediaMutationContext

`MediaRequestContext` 保留 read/local adapter 共有字段，并新增：

```rust
pub struct MediaRequestContext {
    pub request_id: RequestId,
    pub correlation_id: Option<String>,
    pub principal: Option<Principal>,
    pub source_adapter: String,
    pub trace_context: Option<String>,
    pub deadline: Option<i64>,
    pub idempotency_key: Option<String>,
    pub mutation: Option<MediaMutationContext>,
}
```

`MediaMutationContext` 固定包含：

```text
tenant_id
message_id
source_signaling_node_id
owner_epoch
target_media_node_id
target_media_node_instance_epoch
operation_id
operation_step_id
media_session_id
media_binding_id
contract_version
traceparent
tracestate
```

所有 cluster mutation 要求 `mutation`、idempotency key 和绝对 UTC deadline。read RPC 也必须
有 tenant、target node/instance 和 contract version，但不要求 idempotency/operation step。

Native/ZLM/local module 不得伪造 signaling owner。它们使用显式 `LocalControlContext`，资源
归属保留的 system tenant/local owner namespace。local adapter 不能读取或修改 cluster
tenant 资源；cluster adapter 也不能枚举 system tenant。

## 3. CTX-03：统一验证顺序

每个 mutation 在 adapter 和 provider guard 各验证一次，顺序固定：

1. contract version/checksum；
2. mTLS identity 与 source signaling node；
3. required field/格式/长度；
4. tenant 和 resource scope；
5. deadline；
6. target media node ID/instance epoch；
7. node Active/Draining/Isolated gate；
8. owner epoch、binding/session、expected generation；
9. capability/preflight；
10. admission 与 capacity permit；
11. 执行副作用。

adapter 校验用于快速拒绝；provider guard 是安全边界，防止其他 adapter 绕过。deadline 在
真正分配端口、文件、publisher、task 或 blocking worker 前必须再次检查。

## 4. FENCE-01：节点实例 fencing

节点 supervisor 持有当前：

```text
stable node ID
process instance ID
accepted instance epoch
lease ID/status/deadline
accepted contract version
```

mutation 的 target node/epoch 必须精确匹配当前值。mismatch 在副作用前返回 StaleOwner，
不泄漏当前 epoch 之外的 tenant 状态。尚未注册、lease 已过期或重新注册未完成时 create
返回 Unavailable/NOT_APPLIED。

若同一进程重新注册获得新 epoch：

- 原 epoch 资源标记 `NeedsVerification`；
- 可 query；
- 只允许带当前 target epoch、匹配 tenant/binding 的 takeover stop/delete；
- 在 signaling reconciliation 确认前不得 update 或创建关联子资源；
- 不静默把旧资源改写为新 epoch。

## 5. FENCE-02：Owner epoch 与 generation

每个 controlled resource 保存 `accepted_owner_epoch` 和 `generation`：

- request epoch 小于记录：StaleOwner/NOT_APPLIED；
- 相等：继续；
- 大于记录：只有 tenant、MediaSession、MediaBinding、resource handle 全匹配时，使用
  SQLite CAS 原子推进 owner epoch，再执行；
- expected generation 不匹配：Conflict/NOT_APPLIED，返回安全的当前 generation；
- mutation 成功后 generation 单调加一；
- 终态 resource 不可通过 update 复活。

stop/delete 的高 epoch takeover 是允许的收敛动作；低 epoch 不得把新 owner 的资源误报为
NotFound 或幂等成功。

## 6. ERR-01：稳定错误

`MediaErrorCode` 至少增加：

```text
StaleOwner
RateLimited
Cancelled
VersionMismatch
CursorExpired
UnknownOutcome
```

`MediaError` 增加：

```rust
pub outcome: EffectOutcome,
pub resource_ref: Option<ControlledResourceRef>,
pub retry_after_ms: Option<u64>,
pub violations: Vec<FieldViolation>,
```

`EffectOutcome`：

- `NotApplied`：确认没有端口、文件、任务、租约或受控状态留下；
- `Applied`：副作用已生效，错误中必须带 resource ref，客户端应 query/compensate；
- `Unknown`：无法证明是否生效，只能 query/reconcile，不得自动重试。

默认不得把内部错误映射为 NotApplied。只有 guard/preflight/capacity 明确发生在副作用前，
或补偿已被验证完成时才可使用 NotApplied。

## 7. gRPC 错误映射

| Domain code | gRPC status | retryable | 默认 outcome |
| --- | --- | --- | --- |
| InvalidArgument/CursorExpired | INVALID_ARGUMENT/OUT_OF_RANGE | false | NOT_APPLIED |
| Unauthenticated | UNAUTHENTICATED | false | NOT_APPLIED |
| PermissionDenied | PERMISSION_DENIED | false | NOT_APPLIED |
| NotFound | NOT_FOUND | false | NOT_APPLIED |
| Conflict/StaleOwner | ABORTED/FAILED_PRECONDITION | false | NOT_APPLIED |
| Busy/RateLimited | RESOURCE_EXHAUSTED | true | NOT_APPLIED |
| Timeout/Cancelled | DEADLINE_EXCEEDED/CANCELLED | 条件化 | 实际判断 |
| Unavailable | UNAVAILABLE | true | NOT_APPLIED 或 UNKNOWN |
| Unsupported/VersionMismatch | UNIMPLEMENTED/FAILED_PRECONDITION | false | NOT_APPLIED |
| UnknownOutcome | UNKNOWN | false | UNKNOWN |
| Internal/StorageFailed | INTERNAL | false | 实际判断 |

wire `safe_message` 只含稳定描述。内部 cause 写入受限日志并关联 request/correlation，不进入
客户端。所有 mapper 分支穷尽匹配已知 code，新增 code 触发编译或 contract test 失败。

## 8. Deadline、取消与审计

- UTC deadline 转换为当前 runtime deadline时检查溢出和 clock skew。
- deadline 过期不排队、不拿 capacity、不写幂等成功。
- 等待 semaphore、DNS、connect、blocking worker、SQLite 时均传播取消。
- 取消发生在副作用后时先有界清理，再依据清理结果返回 NOT_APPLIED/APPLIED/UNKNOWN。
- audit 记录 tenant、operation、step、resource ref、epochs、generation、outcome 和安全 code；
  不记录 payload、secret 或完整 source URL。

## 9. 验收

- 缺任一 required context 字段均在分配前拒绝。
- old owner、old instance、wrong tenant、wrong binding、generation mismatch 无副作用。
- higher owner takeover stop 成功并永久 fence 旧 owner。
- deadline/cancel 在每个故障点返回正确 outcome。
- local/cluster tenant 双向不可见。
- error serialization 不泄漏凭据、路径或内部 cause。
