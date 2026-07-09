# Phase 04: Session Lifecycle Events Observability

- **状态**: 未开始
- **目标**: 补齐 ABL 对播放对象生命周期、断开事件、流列表 URL 和诊断信息的工程能力。

## 实现范围

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| HTTP 连接与播放会话分离 | 未开始 | ABL 2025-06-13 明确修复 |
| 播放断开事件 | 未开始 | ABL 2026-02-02 增加 WebRTC 播放断开通知 |
| WebRTC 播放 URL 暴露 | 未开始 | ABL 2025-12-26/29 增加 getOutList URL |
| 会话诊断指标 | 部分具备 | 复用 module metrics/session registry |
| 清理顺序 | 部分具备 | 需要覆盖半初始化失败和超时关闭 |

## 参考 ABL 行为

ABL 早期曾在 HTTP 连接关闭时误删 WebRTC 对象，后来修正为播放结束或 timeout 后清理。`deleteWebRTC` 负责停止 RTP、释放 FIFO、清理 ICE/DTLS/SRTP、从流源移除客户端并销毁音频编码器。发布记录还要求播放超过一定时长后触发 `on_play_disconnect`，携带 app、stream、networkType、key、ip、port、playDuration 等信息。

## 开发任务

### Task 01: 会话生命周期状态机文档化并测试

- **状态**: 未开始
- **建议文件**:
  - 修改: WebRTC module 会话注册表相关文件
  - 测试: WebRTC module session 测试

验收点：

- POST 创建信令会话。
- ICE/DTLS/SRTP 建立后进入播放会话。
- HTTP request drop 不等于 session close。
- DELETE、driver close、timeout、stream closed 均能进入统一清理路径。
- 半初始化失败不会留下 session registry 残留。

### Task 02: 播放断开事件与最小时长阈值

- **状态**: 未开始
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/config.rs`
  - 修改: WebRTC module event/metrics 相关文件
  - 视情况修改: `crates/sdk` 事件扩展文档或类型

验收点：

- 默认阈值可参考 ABL 的 8 秒策略，但必须配置化。
- 事件至少包含 stream key、session id、network type、remote addr、duration、close reason。
- 不满足阈值的短连接只记录指标，不触发业务断开事件。
- 不引入独立 hook 子系统；优先复用现有 SDK 事件总线。

### Task 03: 控制面暴露 WebRTC URL 和会话摘要

- **状态**: 未开始
- **建议文件**:
  - 修改: WebRTC module HTTP/control API
  - 视情况修改: `cheetah-control` 或 stream snapshot 转换层

验收点：

- 流列表可展示 WebRTC WHEP URL。
- URL 使用 `public_webrtc_base_url` 或 request host 推导。
- 会话摘要展示协议、app、stream、remote addr、创建时间、播放时长、candidate 类型。
- 不泄漏 DTLS fingerprint 私钥或认证 token。

## 测试计划

```powershell
cargo test -p cheetah-webrtc-module session
cargo test -p cheetah-webrtc-module event
cargo clippy -p cheetah-webrtc-module
```

新增测试名称建议：

- `http_drop_does_not_delete_play_session`
- `delete_closes_session_once`
- `play_disconnect_event_respects_min_duration`
- `stream_list_contains_webrtc_whep_url`
- `failed_post_releases_partial_session`
