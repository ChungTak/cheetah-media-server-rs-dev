# 10. 实施路线图与迁移

## 1. Phase 0：契约冻结

交付：`cheetah-media-api` 的 crate skeleton、模型、错误、capability、事件和 trait；完成 MediaKey/StreamKey bridge 设计；为所有目录项登记 capability 和 TODO。

完成条件：domain crate 不依赖 runtime、HTTP、数据库和具体协议；文档中的 trait、字段、错误和事件有对应 Rust 类型规划。

## 2. Phase 1：查询与 stream/session control

交付：Engine provider、native media/session routes、ZLM `getMedia*`、`isMediaOnline`、`getAllSession`、`kick_*`、`close_stream*`。

完成条件：两个 adapter 对同一媒体返回一致的在线、轨道、session 和关闭结果；无第二套 stream registry。

## 3. Phase 2：record、snapshot、file

交付：统一 RecordApi/SnapshotApi；迁移 record module 的旧 `/zlm/*` route 到 ZLM adapter；native record/file routes；record event/webhook。

迁移方式：

1. 先把旧 DTO 转换为 domain request/response。
2. 保留 `/zlm/startRecord` 等旧别名一段时间，内部只调用新 adapter handler。
3. 新增回归测试后删除 record module 中的重复路由和重复转换。
4. 默认保留 `/index/api/startRecord` 等标准兼容路径。

## 4. Phase 3：RTP 与输出

交付：RtpApi provider、native RTP routes、ZLM RTP API、RTP timeout/stopped event、与 RTP module 的端到端测试。

重点：open/connect/send/talk/passive 的生命周期必须复用既有 RTP module；GB28181 项目可以据此接入，但本项目不实现 SIP。

## 5. Phase 4：proxy、WebRTC 和运维 capability

交付：pull/push/FFmpeg proxy、WHIP/WHEP、WebRTC room keeper（若 provider 存在）、server config/load/version 的组合 API。

未有 provider 的能力必须返回 `Unsupported`/`-501`，并在 capability 查询中标记为 unavailable。

## 6. Phase 5：外部信令接入验证

使用独立测试 double 模拟 GB28181、ONVIF、HomeKit、Matter 项目，只验证它们能完成媒体调用流程。不要把任何信令 crate 放入本项目主依赖。

## 7. 配置和开关

建议配置：

```text
media.native.enabled
media.native.path_prefix
media.zlm.enabled
media.zlm.path_prefix              # 默认 /index
media.zlm.legacy_http_200          # 默认 true
media.zlm.strict_fields
media.zlm.secret/auth_mode
media.events.queue_capacity
media.events.webhook_timeout
media.capabilities.record/snapshot/rtp/proxy/webrtc
```

两个 adapter 可独立启停；关闭 ZLM adapter 不得影响 native API，关闭 native API 也不得影响已启用的兼容面。

## 8. TODO 管理规则

每个 TODO 必须包含 capability 名称、当前返回码、预期 domain trait、依赖的 provider、测试占位和目标阶段。禁止以“先返回成功”占位。所有完成的 TODO 从目录中移除并补充实测行为。

