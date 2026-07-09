# Phase 01: Signaling HTTP WHEP ABL Compat

- **状态**: 未开始
- **目标**: 补齐 ABL 风格 WHEP HTTP 信令兼容，使浏览器和历史客户端能通过 `/rtc/v1/whep/?app=&stream=` 稳定播放。

## 实现范围

| 项目 | 状态 | 说明 |
| --- | --- | --- |
| 现有 WHEP/WHIP 路由 | 已有/复用 | 复用 `module/src/http.rs` 中的信令入口 |
| ABL URL 别名 | 未开始 | `/rtc/v1/whep/`、`/rtc/v1/whep` 均映射到 WHEP play |
| OPTIONS 预检 | 未开始 | 返回 200、CORS、Content-Length 0，可配置私网头 |
| POST 响应 Location | 部分具备 | 对齐 WHEP session URL 与 ABL 兼容 URL |
| PATCH/DELETE 会话资源 | 部分具备 | 复用当前 session route，补异常和清理语义 |

## 参考 ABL 行为

ABL `ResponseOPTIONS` 用于解决短时间多路播放时的浏览器预检问题，返回 `Connection: Close`、CORS 和空 body。`ResponsePost` 从 query 中读取 `app` 和 `stream`，确认流存在后生成 answer，并返回 `201 Created`、`application/sdp` 与 `Location`。

## 开发任务

### Task 01: 梳理并补齐 URL 解析测试

- **状态**: 未开始
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/compat.rs`
  - 修改: `crates/protocols/webrtc/module/src/http.rs`
  - 测试: `crates/protocols/webrtc/module/src/compat.rs` 内单元测试或现有 HTTP 测试文件

检查用例：

- `/rtc/v1/whep/?app=live&stream=camera01`
- `/rtc/v1/whep?app=live&stream=camera01`
- `/whep?app=live&stream=camera01`
- 缺少 `app` 或 `stream` 时返回明确错误。

### Task 02: 实现 OPTIONS 兼容响应

- **状态**: 未开始
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/http.rs`
  - 修改: `crates/protocols/webrtc/module/src/module.rs`
  - 修改: `crates/protocols/webrtc/module/src/config.rs`

验收点：

- `OPTIONS /rtc/v1/whep/?app=live&stream=s` 返回 `200`。
- 响应包含 `Access-Control-Allow-Origin`、`Access-Control-Allow-Methods`、`Access-Control-Allow-Headers`、`Content-Length: 0`。
- 配置开启时追加 ABL 兼容的私网访问头。
- OPTIONS 不创建 WebRTC 播放会话。

### Task 03: 规范 POST/PATCH/DELETE 生命周期

- **状态**: 未开始
- **建议文件**:
  - 修改: `crates/protocols/webrtc/module/src/http.rs`
  - 修改: `crates/protocols/webrtc/module/src/session` 或现有会话注册表文件

验收点：

- POST 成功后返回 `201 Created` 和 `application/sdp`。
- `Location` 指向后续 PATCH/DELETE 可使用的稳定资源。
- HTTP 连接关闭不会触发播放会话销毁。
- POST 中流不存在、payload 不匹配、answer 生成失败时释放半初始化资源。

## 测试计划

```powershell
cargo test -p cheetah-webrtc-module whep
cargo test -p cheetah-webrtc-module abl
cargo clippy -p cheetah-webrtc-module
```

新增测试名称建议：

- `abl_whep_url_alias_maps_to_play_request`
- `abl_whep_options_returns_cors_without_session`
- `abl_whep_post_location_supports_patch_delete`
- `abl_whep_http_close_does_not_close_media_session`
