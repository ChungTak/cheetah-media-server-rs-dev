# 13 · S10 发布验收报告

> 本报告对应 `dev-docs/902_api_delivery_plan/11_test_ci_security_and_release.md` 与 `12_execution_roadmap_and_agent_handoff.md` 的 S10 任务。

## 1. 工具链基线

- `rust-toolchain.toml`：`channel = "1.94.1"`，`components = ["rustfmt", "clippy"]`。
- 本地验证：`rustc 1.94.1` 可获取，`rustfmt`/`clippy` 可安装。
- `vendor-ref/simple-media-server` 仅用于本地参考；WebRTC SMS SDP fixtures 已内嵌到 `crates/protocols/webrtc/core/tests/fixtures/sms/`，不再依赖外部仓库即可编译 `cheetah-webrtc-core`。

## 2. 形式化门禁结果

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --check` | 通过 |
| `cargo clippy --workspace --tests` | 通过（仅有历史 `cheetah-connector` 与 `cheetah-hls-module` 未使用警告，无错误） |
| `cargo test -p cheetah-media-api -p cheetah-sdk -p cheetah-engine -p cheetah-media-module -p cheetah-rtp-module -p cheetah-record-module -p cheetah-proxy-module` | 通过 |
| `cargo test -p cheetah-connector --features full` | 51 passed / 0 failed |
| `cargo test -p cheetah-webrtc-core --test sms_sdp_fixtures` | 8 passed / 0 failed |
| `cargo check -p cheetah-server` | 通过 |
| `cargo check -p cheetah-server --no-default-features --features 'rtmp,rtsp,http-flv,hls,rtp,record,webrtc,mp4,fmp4,srt'` | 通过 |
| `cargo check -p cheetah-server --features media-control-full` | 通过 |

## 3. 能力矩阵（实际 provider 状态）

| 能力 | 默认 Engine | `media-control-full` | 说明 |
| --- | --- | --- | --- |
| `Query` | 可用 | 可用 | native/ZLM `getMediaList`、`isMediaOnline`、`getMediaInfo` |
| `SessionControl` | 可用 | 可用 | `close_stream`、`kick_session`、`list_sessions` |
| `Publish` | 可用 | 可用 | `MediaDataPlaneApi::open_frame_publisher` |
| `Subscribe` | 可用 | 可用 | `MediaDataPlaneApi::open_subscriber` / `SubscriberApi::subscribe` |
| `Record` | 无 | 可用 | `cheetah-record-module` 注册 `RecordApi` |
| `Rtp` | 无 | 可用 | `cheetah-rtp-module` 注册 `RtpApi` |
| `Snapshot` | 无 | 可用 | `cheetah-snapshot-module` 注册真实 `SnapshotApi`（订阅关键帧并写 FileHandle） |
| `Proxy` | 无 | 可用 | `cheetah-proxy-module` 注册 `ProxyApi`；`media-control-full` 启用 `rtsp/http-flv/rtmp` 数据面 feature |
| `Webhook` | 无 | 可用 | `cheetah-webhook-dispatcher` 注册 `WebhookApi`（S6 已完成） |

> 默认 Engine 不声明未注册的能力；`media-control-full` 通过模块注册真实 provider，能力版本随注册/注销实时变化。

## 4. HTTP 与兼容路由覆盖

### 4.1 native `/api/v1`

- 路由数量：**35** 条（`crates/system/cheetah-media-module/src/native_routes.rs`）。
- 覆盖：media（含 urls）、sessions、record、snapshots、file store、proxies pull/push/ffmpeg CRUD、RTP。
- 已验证：每条路由均有 `native_routes::native_required_scope` 定义的 scope，未知路由返回 404。
- 动态路径参数：`{vhost}`、`{app}`、`{stream}`、`{session_id}`、`{task_id}`、`{file_id}`、`{proxy_id}` 通过 `cheetah-control` path-template 匹配。

### 4.2 ZLM 兼容 `/index/api`

- 路由总数：**64/64**（`zlm::routes::tests::zlm_catalog_contains_64_required_routes` 通过）。
- 分级：
  - **L1**（核心媒体能力，真实 provider 成功路径）：`getMediaList`、`isMediaOnline`、`close_stream`、`addStreamProxy`、`listStreamProxy`、`openRtpServer` 等。
  - **L2**（可选 provider，无 provider 时返回 `-501`）：`broadcastMessage`、`addFFmpegSource` 等。
  - **L3**（管理类，需 `server.admin` scope）：`getApiList`、`getThreadsLoad`、`restartServer` 等。
  - **L4**（超出本轮范围，显式返回 `-501`）：`getStatistic`、`getMp4RecordFile` 等。
- 已验证：secret 认证、login session cookie Max-Age、`api/list` 字段 golden。

## 5. Webhook / 事件 hook 状态

| Hook 类型 | 状态 | 说明 |
| --- | --- | --- |
| 决策 hook（`on_publish`、`on_play`、`on_rtp_server_timeout` 等） | 可用 | 返回 allow/deny/timeout，支持 TTL |
| 通知 hook（`on_record_mp4`、`on_stream_changed` 等） | 可用 | ZLM 兼容 payload，含 golden fixture 测试 |
| 出站 dispatcher | 可用 | 有界重试、熔断、SSRF 拒绝、日志脱敏 |

## 6. 四类第三方信令生产 Contract

测试入口：`cargo test -p cheetah-sdk --test signal_contracts`

| 协议 | 生产测试 | 关键验证点 |
| --- | --- | --- |
| GB28181 | `gb28181_can_open_receiver_and_sender_sessions` | 真实 `RtpModule`，UDP socket 实际收发 RTP |
| ONVIF | `onvif_can_query_media_take_snapshot_and_record` | `is_media_online`、`get_media_list`、`request_keyframe`、`take_snapshot`、`start_record`、`stop_record`、`query_record_tasks` |
| ONVIF | `onvif_proxy_rejects_internal_target_and_lists_empty` | 真实 `ProxyApi` SSRF 拒绝 `127.0.0.1` |
| HomeKit | `homekit_can_subscribe_and_snapshot` | `SubscriberApi::subscribe` 接收 VP8 帧、`request_keyframe`、真实 `SnapshotModule` 抓取关键帧 |
| Matter | `matter_can_query_capabilities_and_subscribe` | 能力集合断言、订阅、真实 snapshot、`subscribe_events` |
| Common | `media_list_can_filter_and_paginate` / `unknown_stream_is_not_online` | 通用过滤、未知流离线 |

结果：**13 passed / 0 failed**。所有测试启动真实 `Engine`，不依赖公网或真实设备。

## 7. 安全测试结果

| ID | 风险 | 验证 |
| --- | --- | --- |
| SEC-01 | 无鉴权踢流/删文件 | native 路由 `MediaScope` 已覆盖；ZLM `secret` 认证要求 header |
| SEC-02 | 跨 vhost/app 权限 | 路由 scope 拒绝越权访问 |
| SEC-03 | 文件路径穿越 | `cheetah-record-module` 拒绝 `..`、绝对路径、空段 |
| SEC-04 | webhook SSRF | `ProxyModule` 与 webhook dispatcher 拒绝 loopback/link-local/内网 |
| SEC-05 | FFmpeg 参数注入 | typed `FFmpegJob` 校验 `-i`、filter_complex 等，拒绝换行与未授权选项 |
| SEC-06 | secret 日志泄漏 | ZLM URL `secret` 参数被拒绝，token 不写入日志 |
| SEC-07 | 大 body/分页/队列 | `MediaQuery` 页大小 clamp；record/registry 上限测试 |
| SEC-08 | URL Host 注入 | `MediaUrlResolver` 使用配置 `public_host` |
| SEC-09 | 下载 handle 猜测/过期 | `MediaFileStoreApi` 公开文件需显式注册，private 下载拒绝未授权 |
| SEC-10 | webhook 重试风暴 | 指数退避 + 最大重试次数 + 熔断 |

## 8. 已知限制

1. `cheetah-connector` 默认 features 为空，完整能力测试需 `cargo test -p cheetah-connector --features full`（与 `900_sdk_gaps_plan2` 一致）。
2. `cheetah-hls-module` 存在若干未使用字段/方法警告（死代码），不影响 `cargo clippy` 退出码，建议后续 HLS 阶段清理。
3. `cheetah-webrtc-core` 的 SMS SDP fixtures 已从 `vendor-ref` 内嵌到 `tests/fixtures/sms/`，来源为 [simple-media-server](https://gitee.com/inyeme/simple-media-server)（MulanPSL-2.0）。

## 9. 发布结论

- S0–S9 已全部完成并通过 Devin Review。
- L0–L4 测试门禁（domain unit、provider integration、adapter contract、protocol loopback、signal production）全绿。
- `cargo check -p cheetah-server --features media-control-full` 通过，完整能力组合可编译。
- 未发现 P0 未完成项、capability 说谎、鉴权绕过或 production contract 依赖 fake 的情况。

**S10 发布门禁通过。**
