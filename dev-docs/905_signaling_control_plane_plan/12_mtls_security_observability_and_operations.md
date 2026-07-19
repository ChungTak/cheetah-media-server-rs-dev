# 12 · mTLS、安全、观测与运维

## 1. SEC-01：gRPC Server mTLS

生产配置必须提供 server certificate/key、client CA trust、允许的 signaling identity policy。
规则：

- 禁止 production plaintext；
- 从 TLS peer certificate SAN URI/DNS 提取 identity；
- identity 必须匹配 request source signaling node；
- tenant/resource scope 由 Principal/provider 再复核；
- 证书过期、未知 CA、SAN mismatch 在 mapper 前拒绝；
- 不接受普通 metadata 伪造 `x-mtls-identity`。

仅 loopback 测试 profile 可显式允许 plaintext，capability/health 必须标记非生产。

## 2. SEC-02：证书轮换

证书/CA/key 通过受控 handle 或部署文件监听器加载：

- 先解析/校验新材料；
- 原子切换新 listener/channel；
- 旧 listener 停止 accept，给现有 RPC 有界 drain；
- registry/SecretExchange channel 按新 identity 重建；
- 不重启 engine/module 或已有媒体 session；
- 失败保留旧有效证书并告警，不落回 plaintext。

轮换测试覆盖 server cert、client cert、CA overlap、撤销旧 CA 和并发 event stream。

## 3. SEC-03：输入与数据保护

- per RPC message/field/list/map/string/batch 上限；
- URL/endpoint/ID/cursor/SDP 字段专用验证；
- safe error 与 internal cause 分离；
- SQLite 文件权限最小化，backup 不含 secret；
- HMAC cursor key、TLS key、SecretExchange credential 不进入配置 dump；
- gRPC reflection/admin diagnostics 默认关闭；
- panic/poison/corrupt payload 不导致进程退出或跨 tenant 泄漏。

## 4. OBS-01：结构化日志与 audit

日志固定字段：

```text
service/method
request_id/correlation_id
operation/step
resource kind
node state/instance epoch
owner epoch/generation
contract version
result code/outcome
latency bucket
```

tenant、resource handle 仅在允许的 audit 中记录或 hash，不作为低基数业务日志默认字段。禁止
raw request/response、secret、Authorization、URL userinfo、完整内部路径、SDP/packet。

audit 覆盖 register、drain、fencing reject、create/stop/delete、credential exchange、
forced reconciliation、mTLS identity failure、证书轮换和 config change。

## 5. OBS-02：Metrics

至少提供：

- node state、lease remaining、heartbeat/register result；
- RPC requests/latency/error/outcome/inflight/rejected；
- idempotency replay/conflict/unknown/recovery；
- controlled resources/permits/usage/reject reason；
- event journal rows/bytes/lag/gap/subscribers/reconnect；
- reconciliation scanned/repaired/failed/orphan；
- SQLite latency/busy/error/size/checkpoint；
- credential exchange/fetch result（无 secret labels）；
- certificate expiry/reload result。

labels 只使用 service、operation、resource kind、state、code、reason、profile 等有限枚举。

## 6. OBS-03：Health

health 报告分项：

```text
contract
store
grpc listener
registry lease
capability/preflight
capacity
event journal
credential exchange
reconciliation
```

overall health 不把 Draining 当 crash；Draining/Isolated 明确影响 create readiness，但保留
read/stop readiness。health 不暴露 endpoint secret、路径、tenant 或 raw error。

## 7. OPS-01：故障注入

为每种 mutation 注入：

- before/after idempotency prepare；
- before/after capacity；
- before/after side effect；
- before/after result commit；
- response loss；
- event append/send loss；
- SQLite busy/full/corrupt；
- registry timeout/lease expiry/instance replacement；
- worker panic、module restart、engine shutdown；
- slow RPC/event subscriber、TLS rotate。

每个点断言 outcome、持久状态、resource count、permit、event 与 leak report。

## 8. OPS-02：Admin 操作

受 mTLS admin scope/audit 保护：

- enter/leave drain；
- trigger reconciliation；
- inspect safe node/store/event diagnostics；
- rotate TLS/cursor key；
- compact/checkpoint store；
- typed orphan cleanup。

不提供 dump secret、raw SQLite arbitrary query、任意文件读取、绕过 tenant/fencing 或“kill all”
接口。

## 9. 验收

- mTLS identity/tenant/resource scope 正负矩阵。
- cert rotation 不丢已有 media session。
- metrics label cardinality 检查。
- logs/audit/store artifact secret scan。
- fault matrix 后节点可恢复且无重复资源。
- health/readiness 与 Active/Draining/Isolated/NotServing 精确一致。
