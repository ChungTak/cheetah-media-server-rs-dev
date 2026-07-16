# ZLM 兼容接口证据目录 (ZLM-01)

本文件按 `dev-docs/903_api_completion_plan/11_zlm_compatibility_revalidation.md` 要求，
重新登记所有 ZLMediaKit 兼容 `/api/*` 接口的证据等级（L0-L4）。

## 证据等级定义

| 等级 | 含义 |
| --- | --- |
| L0 | 路由、参数、JSON golden、fake provider；兼容响应已定义 |
| L1 | production provider + 本地真实媒体/文件；单接口真实可用 |
| L2 | 多接口流程/生命周期或本地集成测试验证 |
| L3 | 独立 HTTP 客户端黑盒测试 |
| L4 | 长稳、故障、性能与资源泄漏测试 |

## 接口目录

| 方法 | 路由 | Scope | 路由级别 | Capability / Domain Port | 证据等级 | 原状态 | 测试引用 | 备注 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Get | `/api/getThreadsLoad` | `ServerAdmin` | L3 | System/Admin | L2 | interop-tested | zlm_l3_l4 | 返回 `-501` |
| Get | `/api/getWorkThreadsLoad` | `ServerAdmin` | L3 | System/Admin | L0 | capability-gated |  |  |
| Get | `/api/getServerConfig` | `ServerAdmin` | L3 | System/Admin | L0 | capability-gated |  |  |
| Post | `/api/setServerConfig` | `ServerAdmin` | L3 | System/Admin | L0 | capability-gated |  |  |
| Get | `/api/getApiList` | `ServerAdmin` | L3 | System/Admin | L2 | interop-tested | zlm_l3_l4 |  |
| Get | `/api/version` | `ServerAdmin` | L3 | System/Admin | L2 | interop-tested | zlm_l3_l4, zlm_session_auth |  |
| Post | `/api/restartServer` | `ServerAdmin` | L3 | System/Admin | L0 | capability-gated |  |  |
| Get | `/api/getMediaList` | `MediaRead` | L1 | MediaControlApi | L2 | interop-tested | zlm_golden |  |
| Get | `/api/isMediaOnline` | `MediaRead` | L1 | MediaControlApi | L2 | interop-tested | zlm_golden |  |
| Get | `/api/getMediaPlayerList` | `MediaRead` | L1 | MediaControlApi | L2 | interop-tested | zlm_golden |  |
| Get | `/api/getMediaInfo` | `MediaRead` | L1 | MediaControlApi | L2 | interop-tested | zlm_golden |  |
| Post | `/api/close_stream` | `MediaControl` | L1 | MediaControlApi | L2 | interop-tested | zlm_golden |  |
| Post | `/api/close_streams` | `MediaControl` | L1 | MediaControlApi | L2 | interop-tested | zlm_golden |  |
| Get | `/api/getAllSession` | `MediaRead` | L1 | MediaControlApi | L2 | interop-tested | zlm_golden |  |
| Post | `/api/kick_session` | `MediaControl` | L1 | MediaControlApi | L2 | interop-tested | zlm_golden |  |
| Post | `/api/kick_sessions` | `MediaControl` | L1 | MediaControlApi | L2 | interop-tested | zlm_golden |  |
| Post | `/api/broadcastMessage` | `MediaControl` | L2 | MediaControlApi | L0 | capability-gated |  |  |
| Post | `/api/addStreamProxy` | `MediaPublish` | L1 | ProxyApi | L2 | interop-tested | zlm_proxy |  |
| Post | `/api/delStreamProxy` | `MediaControl` | L1 | ProxyApi | L2 | interop-tested | zlm_proxy |  |
| Get | `/api/listStreamProxy` | `MediaRead` | L1 | ProxyApi | L2 | interop-tested | zlm_proxy |  |
| Get | `/api/getProxyInfo` | `MediaRead` | L1 | ProxyApi | L2 | interop-tested | zlm_proxy |  |
| Post | `/api/addStreamPusherProxy` | `MediaPublish` | L1 | ProxyApi | L2 | interop-tested | zlm_proxy |  |
| Post | `/api/delStreamPusherProxy` | `MediaControl` | L1 | ProxyApi | L2 | interop-tested | zlm_proxy |  |
| Get | `/api/listStreamPusherProxy` | `MediaRead` | L1 | ProxyApi | L2 | interop-tested | zlm_proxy |  |
| Get | `/api/getProxyPusherInfo` | `MediaRead` | L1 | ProxyApi | L2 | interop-tested | zlm_proxy |  |
| Post | `/api/addFFmpegSource` | `MediaPublish` | L2 | ProxyApi | L2 | interop-tested | zlm_l2 |  |
| Post | `/api/delFFmpegSource` | `MediaControl` | L2 | ProxyApi | L2 | interop-tested | zlm_l2 |  |
| Get | `/api/listFFmpegSource` | `MediaRead` | L2 | ProxyApi | L2 | interop-tested | zlm_l2 |  |
| Get | `/api/getRtpInfo` | `MediaRead` | L1 | RtpApi | L2 | interop-tested | rtp_adapter_mapping, zlm_golden |  |
| Post | `/api/openRtpServer` | `MediaPublish` | L1 | RtpApi | L2 | interop-tested | rtp_adapter_mapping, zlm_golden |  |
| Post | `/api/openRtpServerMultiplex` | `MediaPublish` | L2 | RtpApi | L2 | interop-tested | zlm_l2 |  |
| Post | `/api/connectRtpServer` | `MediaPublish` | L1 | RtpApi | L2 | interop-tested | zlm_golden |  |
| Post | `/api/closeRtpServer` | `MediaControl` | L1 | RtpApi | L2 | interop-tested | rtp_adapter_mapping, zlm_golden |  |
| Post | `/api/updateRtpServerSSRC` | `MediaControl` | L2 | RtpApi | L0 | capability-gated | zlm_l2 | 返回 `-501` |
| Get | `/api/listRtpServer` | `MediaRead` | L1 | RtpApi | L2 | interop-tested | rtp_adapter_mapping, zlm_golden |  |
| Post | `/api/pauseRtpCheck` | `MediaControl` | L2 | RtpApi | L2 | interop-tested | zlm_l2 |  |
| Post | `/api/resumeRtpCheck` | `MediaControl` | L2 | RtpApi | L2 | interop-tested | zlm_l2 |  |
| Post | `/api/startSendRtp` | `MediaConsume` | L1 | RtpApi | L2 | interop-tested | rtp_adapter_mapping, zlm_golden |  |
| Post | `/api/startSendRtpPassive` | `MediaConsume` | L1 | RtpApi | L2 | interop-tested | zlm_golden |  |
| Post | `/api/startSendRtpTalk` | `MediaConsume` | L1 | RtpApi | L2 | interop-tested | zlm_golden |  |
| Get | `/api/listRtpSender` | `MediaRead` | L1 | RtpApi | L2 | interop-tested | rtp_adapter_mapping, zlm_golden |  |
| Post | `/api/stopSendRtp` | `MediaControl` | L1 | RtpApi | L2 | interop-tested | rtp_adapter_mapping, zlm_golden |  |
| Post | `/api/startRecord` | `RecordManage` | L1 | RecordApi | L2 | interop-tested | zlm_golden |  |
| Post | `/api/startRecordTask` | `RecordManage` | L1 | RecordApi | L2 | interop-tested | zlm_golden |  |
| Post | `/api/setRecordSpeed` | `RecordManage` | L2 | PlaybackApi | L1 | golden-tested | zlm_golden | 已覆盖缺失文件错误路径 |
| Post | `/api/seekRecordStamp` | `RecordManage` | L2 | PlaybackApi | L1 | golden-tested | zlm_golden | 已覆盖缺失文件错误路径 |
| Post | `/api/stopRecord` | `RecordManage` | L1 | RecordApi | L2 | interop-tested | zlm_golden |  |
| Get | `/api/isRecording` | `MediaRead` | L1 | RecordApi | L2 | interop-tested | zlm_golden |  |
| Get | `/api/getMP4RecordFile` | `MediaRead` | L1 | RecordApi/FileStore | L2 | interop-tested |  |  |
| Post | `/api/deleteRecordDirectory` | `FileDelete` | L1 | RecordApi/FileStore | L2 | interop-tested | zlm_golden |  |
| Post | `/api/loadMP4File` | `RecordManage` | L2 | PlaybackApi | L0 | capability-gated |  |  |
| Post | `/api/controlRecordPlay` | `RecordManage` | L2 | PlaybackApi | L1 | golden-tested | zlm_golden | 已覆盖缺失文件错误路径 |
| Get | `/api/getSnap` | `MediaControl` | L1 | SnapshotApi | L2 | interop-tested | zlm_golden |  |
| Post | `/api/deleteSnapDirectory` | `FileDelete` | L1 | SnapshotApi/FileStore | L2 | interop-tested | zlm_golden |  |
| Get | `/api/downloadFile` | `FileRead` | L1 | MediaFileStoreApi | L2 | interop-tested | zlm_golden |  |
| Post | `/api/webrtc` | `MediaPublish` | L2 | WebRtc | L0 | capability-gated |  |  |
| Post | `/api/whip` | `MediaPublish` | L2 | WebRtc | L0 | capability-gated |  |  |
| Post | `/api/whep` | `MediaConsume` | L2 | WebRtc | L0 | capability-gated |  |  |
| Post | `/api/delete_webrtc` | `MediaControl` | L2 | WebRtc | L0 | capability-gated |  |  |
| Get | `/api/getWebrtcProxyPlayerInfo` | `MediaRead` | L2 | WebRtc | L0 | capability-gated |  |  |
| Post | `/api/addWebrtcRoomKeeper` | `MediaControl` | L2 | WebRtcRoom | L0 | capability-gated |  |  |
| Post | `/api/delWebrtcRoomKeeper` | `MediaControl` | L2 | WebRtcRoom | L0 | capability-gated |  |  |
| Get | `/api/listWebrtcRoomKeepers` | `MediaRead` | L2 | WebRtcRoom | L0 | capability-gated |  |  |
| Get | `/api/listWebrtcRooms` | `MediaRead` | L2 | WebRtcRoom | L0 | capability-gated |  |  |
| Post | `/api/login` | `MediaRead` | L3 | ControlAuthApi | L2 | interop-tested | zlm_session_auth | session auth |
| Post | `/api/logout` | `MediaRead` | L3 | ControlAuthApi | L2 | interop-tested | zlm_session_auth | session auth |
| Get | `/api/searchOnvifDevice` | `MediaRead` | L4 | (out of scope) | L2 | interop-tested | zlm_l3_l4 | 返回 `-501` |
| Get | `/api/getStreamUrl` | `MediaRead` | L2 | MediaUrlResolverApi | L0 | provider-wired |  | Engine `MediaUrlResolverApi` via StreamInfo.urls |
| Post | `/api/addProbe` | `ServerAdmin` | L3 | System/Admin | L0 | capability-gated |  |  |
| Post | `/api/stack/start` | `ServerAdmin` | L3 | System/Admin | L0 | capability-gated |  |  |
| Post | `/api/stack/reset` | `ServerAdmin` | L3 | System/Admin | L0 | capability-gated |  |  |
| Post | `/api/stack/stop` | `ServerAdmin` | L3 | System/Admin | L0 | capability-gated |  |  |
| Get | `/api/downloadBin` | `ServerAdmin` | L3 | System/Admin | L0 | capability-gated |  |  |

## 参数与响应字段族说明

以下说明按接口族给出公共参数/别名及成功/错误字段模式；
各端点具体字段以 handler 和 DTO 为准。

| 族 | 必选参数 | 常用可选参数/别名 | 成功字段 | 错误响应 |
| --- | --- | --- | --- | --- |
| 媒体查询 | `vhost`, `app`, `stream`（支持 `stream_id`/`stream` 别名） | `schema`, `page`, `page_size` | `code`, `data` 数组 | `code=-300` 参数冲突；`code=-500` 未找到 |
| 会话控制 | `id`（session_id）或 `vhost`/`app`/`stream` 批量 | `close_flag` | `result`(int/bool), `count_hit` | `code=-500` |
| Proxy | `vhost`, `app`, `stream`, `url`/`source_url` | `enable_hls`, `enable_mp4`, `rtp_type`, `retry`, `timeout` | `key`, `result`, `port` 等 | `code=-400`/`code=-501` |
| RTP | `vhost`, `app`, `stream`, `port`, `ssrc`, `dst_url`/`dst_port` | `tcp_mode`, `reuse_port`, `payload_type`, `rtcp_port` | `port`, `ssrc`, `session_id` | `code=-300`/`-400` |
| 录制/回放 | `vhost`, `app`, `stream`, `start`/`end`/`format` | `file`, `taskId` | `result`, `taskId`, `status` | `code=-200`/`-500` |
| 快照/文件 | `vhost`, `app`, `stream` / `file_id` | `timeout_sec`, `expire_sec`, `quality`, `scale` | JPEG 二进制或 `code`, `data` | `code=-200`/`-500` |
| WebRTC | `vhost`, `app`, `stream`, `type`/`sdp` | - | SDP/ICE 字段 | `code=-400`/`-501` |
| 系统/管理 | 见各接口白名单 | - | `code`, `data` | `code=-100`/`-501` |

## 已知限制

- L3/L4 黑盒与长稳测试未在 ZLM-01 完成，将在 ZLM-04 中补充。
- WebRTC/ROOM 系列当前返回 `-501`（capability-gated），证据等级暂为 L0。
- `searchOnvifDevice`、`addProbe`、`stack/*`、`downloadBin` 为非媒体能力占位，证据等级 L0。

## 生成说明

本目录由 `crates/system/cheetah-media-module/src/zlm/routes.rs`、
`dev-docs/902_api_delivery_plan/09_zlm_full_compatibility_catalog.md`、
及 `crates/system/cheetah-media-module/tests/*.rs` 中的测试引用自动生成。

## Golden 参考（ZLM-03）

### 1. 公共别名表

| 内部字段 | 兼容别名 | 说明 |
| --- | --- | --- |
| `stream` | `stream_id` | stream id 与 stream 互转，只保留 `MediaKey.stream` |
| `vhost` | `domain` | 缺省 vhost 取部署默认值，空字符串不视为任意 vhost |
| `schema` | `protocol` | 输出 URL schema 过滤 |
| `timeout_ms` | `timeout`, `timeout_sec` | 毫秒优先；秒别名乘以 1000 |
| `quality` | `scale` | 图片质量 0-100 |
| `max_width` | `width` | 快照最大宽度 |
| `max_height` | `height` | 快照最大高度 |
| `file_path` | `fileId` | 文件句柄/安全路径句柄 |
| `dst_url` | `dst_ip` + `dst_port` | RTP 发送目标 |
| `tcp_mode` | `tcp`, `enable_tcp`, `is_udp` | `0`/UDP, `1`/passive, `2`/active |
| `codec_hint` | `codec`, `payload_mode` | 编码提示 |

### 2. 布尔/数值别名

- 布尔：`true`/`false`、`1`/`0`、`yes`/`no`、`on`/`off`、非零数字均解析为 true。
- 输出保持 ZLM profile 的固定格式，不混合格式。

### 3. 错误码映射

| `MediaErrorCode` | ZLM `code` | 含义 |
| --- | --- | --- |
| `InvalidArgument` | `-300` | 参数缺失/格式错误/别名冲突 |
| `Unauthenticated` | `-100` | 未认证/无 scope |
| `PermissionDenied` | `-100` | 有认证但权限不足 |
| `NotFound` | `-500` | 资源/会话/流不存在 |
| `Unavailable` | `-400` | 能力未注册/ provider 未启动 |
| `StorageFailed` | `-200` | 存储操作失败 |
| `Unsupported` | `-501` | 能力被能力门控明确拒绝（非媒体能力） |
| `Conflict` | `-300` | 同幂等键冲突或资源重复 |
| `Internal` | `-400` | 内部错误/未分类 |

### 4. 响应信封

- 成功（data 型）：`{"code":0,"data":{...}}`
- 成功（action 型）：`{"code":0,"result":true}` 或 `{ "result": true, "taskId": "..." }`
- 成功（状态型）：`{"code":0,"online":true}` / `{"code":0,"status":true}`
- 错误：`{"code":<code>,"msg":"..."}`

### 5. 关键端点字段示例

- `getMediaList` 成功：`{"code":0,"data":[{"schema":"rtmp",...}]}`
- `openRtpServer` 成功：`{"code":0,"data":{"port":10000,"ssrc":123456,"session_id":"..."}}`
- `startRecord` 成功：`{"code":0,"data":{"result":true,"taskId":"..."}}`
- `getSnap` 成功：HTTP 200 + `image/jpeg` 二进制，首字节 `FF D8 FF`。
- `getSnap` 非 JPEG 失败：`{"code":-400,"msg":"snapshot is not a decodable JPEG"}`。
- `loadMP4File` 成功：`{"code":0,"data":{"sessionId":"...","duration_ms":0}}`。
- 任意 capability-gated 路由：`{"code":-501,"msg":"unsupported capability: ..."}`。

### 6. 负向 golden

- 缺 `vhost`/`app`/`stream` 返回 `-300`。
- 不存在 session/stream 的查询返回 `code=0` 且空数组/ `online=false`（按 ZLM 惯例）。
- 不存在 session/stream 的修改返回 `-500`。
- 越权访问返回 `-100`/`-101`。