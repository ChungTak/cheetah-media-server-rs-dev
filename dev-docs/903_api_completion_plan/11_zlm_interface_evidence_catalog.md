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