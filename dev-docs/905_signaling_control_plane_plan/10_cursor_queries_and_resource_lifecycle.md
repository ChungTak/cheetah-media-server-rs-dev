# 10 · Cursor 查询与资源生命周期

## 1. QRY-01：统一分页

公共 Domain 新增：

```rust
pub struct CursorPageRequest {
    pub cursor: Option<OpaqueCursor>,
    pub page_size: u32,
}

pub struct CursorPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<OpaqueCursor>,
}
```

cluster contract 不返回 page number/offset/不稳定 total。page size 有统一默认和 hard max。
旧 `Page<T>`/`page` 字段按迁移期 deprecated；新 gRPC mapper 只能使用 CursorPage。

## 2. QRY-02：排序与 cursor 内容

每类资源固定排序：

```text
(updated_at, resource_handle)
```

或在无法稳定获得 updated_at 时：

```text
(created_at, resource_handle)
```

唯一 handle 必须作为最后 tie-breaker。cursor 包含 schema version、resource kind、sort key、
tenant/filter digest、node instance epoch、snapshot boundary/expiry 和 HMAC。

分页期间新建/更新资源不应造成已有 snapshot 重复/遗漏；实现使用查询开始时的 high-watermark。
节点重启/retention 使 cursor 无法继续时返回 CursorExpired，不能从第一页静默重来。

## 3. QRY-03：统一过滤

所有 RTP/proxy/record/snapshot/playback/processing/session 至少支持：

```text
tenant_id (required)
media_session_id
media_binding_id
resource_handle
media_key
idempotency_key
state/non_terminal
owner_epoch
node_instance_epoch
updated-before/after
```

filter 组合使用明确 AND 语义。未知 filter/非法状态返回 InvalidArgument，不忽略。

## 4. QRY-04：Controlled Resource View

每个 Get/List 结果包含：

```text
ControlledResourceMeta
typed resource state
generation
node/instance epoch
accepted owner epoch
created/updated/terminal time
safe last error
negotiation/result fields
MediaKey/output reference
```

RTP 补齐 advertised address、RTCP、SSRC/payload/transport/TCP mode；Proxy 返回 sanitized source；
file operation 只返回 FileHandle；output URL 由 resolver 单独生成。

## 5. QRY-05：Provider 迁移

按顺序迁移：

1. Domain query/request/response 类型。
2. engine facade。
3. RTP provider。
4. Proxy。
5. Record/Snapshot/Playback。
6. Processing/Session。
7. Native/ZLM compatibility adapter。
8. gRPC mapper。

同一阶段保持 workspace 编译。禁止 provider 各自设计不同 cursor encoding；统一 cursor codec
属于 control plane。provider 只接受 decoded stable boundary/filter，并返回下一 sort key。

## 6. 旧接口兼容

- Native API 新路径返回 opaque cursor。
- 旧 native page 参数给出 deprecation header，并限制可模拟的最大页数。
- ZLM 必须保留的 page 语义只在 adapter 对单次 bounded snapshot 做转换，不泄漏到 Domain。
- 不允许大 offset 扫描；超过兼容上限返回明确错误。
- release notes 给出 page → cursor 迁移示例和截止版本。

## 7. Typed cleanup

Stop/Delete 请求必须包含 tenant、resource ref、expected generation 和 mutation context：

- Running/Active → Stopping → terminal；
- terminal 重复 stop/delete 返回第一次结果；
- NotFound 且关联正确可视为补偿完成；
- wrong tenant/binding/owner/instance/generation 明确拒绝；
- 任何 cleanup 失败保留 resource index 和 safe error 供下次对账。

不增加 `force=true` 绕过 fencing。Admin cleanup 使用单独 scope/audit，但仍要求 tenant 和
current instance。

## 8. 验收

- 各资源 0/1/max/multi-page；
- 同 sort timestamp 多 handle；
- 分页中并发 create/update/delete；
- cursor tamper/filter/tenant/instance mismatch/expiry；
- 重启后可继续或明确 CursorExpired；
- reconciler 全扫描无重复/遗漏；
- legacy adapter 有界兼容；
- typed cleanup 无跨 tenant 或旧 owner 成功。
