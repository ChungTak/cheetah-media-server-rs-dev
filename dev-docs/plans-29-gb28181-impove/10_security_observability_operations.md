# 10 · 安全、观测与运维

## 1. 安全控制

| ID | 控制 | 验收 |
| --- | --- | --- |
| SEC-01 | 对外媒体 API 使用认证身份、tenant scope、deadline 与 request rate limit | auth/scope/rate matrix |
| SEC-02 | media mutation 全部执行 tenant、resource、owner、generation 与 admission 校验 | cross-tenant/no-effect tests |
| SEC-03 | source binding、rebind rate limit、SSRC/PT continuity 防止 RTP 注入 | spoof tests |
| SEC-04 | API/RTP/PS/JTT/Ehome/framing 所有长度和集合都有 hard limit | fuzz/OOM guard |
| SEC-05 | 外部网络 adapter 使用 mTLS、SAN/source identity 与证书轮换 | live rotation test |
| SEC-06 | secret/Authorization/URL userinfo/原始报文不进入 log/event/store/error | automated secret scan |

compat profile 不得放松 tenant/admission/fencing/size limit。确需扩大某个 wire limit 时使用具名配置、
硬上限和启动警告，不能由远端报文动态决定。

## 2. 指标

指标 label 只使用 protocol、direction、transport、framing、container、codec、state、error code、
compat rule 等低基数枚举；tenant/device/session/SSRC 不作为常驻 label。

- session：requested/active/draining/stopped/failed、open/stop latency、rollback count。
- resources：ports/sockets/tasks/capacity/publisher leases 当前值和 high-water mark。
- RTP：packets/bytes/lost/duplicate/reordered/late/jitter/source-reject/rebind。
- RTCP：SR/RR/BYE、RTT、timeout。
- codec：PS/PES/PSM errors、probe result/limit、track changes、timestamp discontinuity、AU drops。
- transport：accept/connect errors、framing detect/resync、queue depth/drop、slow peer。
- control API：client/lease/drain state、mutation outcome、stale generation、idempotency replay。

## 3. 事件与日志

结构化事件至少包括 resource ref、generation、state transition、reason code、compat rule、统计摘要和
correlation ID。endpoint、external session ID 和 StreamKey 按安全策略哈希/截断；原始 RTP 只允许
在受控测试环境抓取，不进入默认生产日志。

关键事件：admission denied、capacity rejected、port bind failed、publisher conflict、source rejected/
rebound、format detected/changed、RTCP timeout、rollback incomplete、lease lost、reconcile orphan。

## 4. Readiness 与故障策略

- media readiness：配置有效、port pool 可用、必要 module Running、facade/event/recovery 已装配。
- 网络 adapter readiness：media readiness + mTLS server + media contract version + client/registry lease。
- lease loss 或 drain：readiness 降级并拒绝新 create，现有 query/stop 保留；策略明确时才 drain media。
- event/store 暂时不可用：mutation 是否允许必须由 905 durability policy 决定，不得静默丢事件。
- cleanup 失败：资源进入 Failed/Orphaned，保留 typed cleanup 重试和 audit，不伪装 Stopped。

## 5. 运维配置与诊断

- 启动时输出不含 secret 的 resolved capability/profile/limits/media contract revision。
- admin API 只读默认开放；强制 stop/reconcile/profile override 需要独立 scope 和 audit。
- 提供 session/resource/port/task/lease 的一致性诊断，不暴露 engine 私有对象。
- 配置变更标注 hot apply 或 `ModuleRestartRequired`，重启由 ModuleManager 生命周期执行。
- runbook 覆盖端口耗尽、RTP 无流、PS 无 PSM、SSRC 冲突、source change、RTCP timeout、controller/
  registry outage 和 rollback incomplete。
