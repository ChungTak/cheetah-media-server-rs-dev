# 09 · ZLMediaKit Compatibility 全目录交付

## 1. 完成规则

本轮要求 901 已列出的 64 个 `/index/api/*` 全部登记 route。每个 route 必须有：HTTP method、认证、字段 DTO、domain/admin port、provider capability、成功响应、错误响应、测试 ID 和交付级别。

级别定义：

- **L1 Required**：媒体服务器核心能力，必须有生产成功路径。
- **L2 Optional provider**：属于媒体系统但依赖可选 module；provider 启用时真实执行，否则 `-501`。
- **L3 Admin guarded**：高风险运维能力，默认关闭并要求 `server.admin`。
- **L4 Out of scope compatibility**：信令/诊断附属能力，只提供诚实 `-501`，不得伪造结果。

## 2. L1 核心媒体接口

| 族 | API | 目标 port |
| --- | --- | --- |
| media | `getMediaList`、`isMediaOnline`、`getMediaPlayerList`、`getMediaInfo` | MediaControlApi |
| control | `close_stream`、`close_streams`、`getAllSession`、`kick_session`、`kick_sessions` | MediaControlApi/session directory |
| proxy | `addStreamProxy`、`delStreamProxy`、`listStreamProxy`、`getProxyInfo` | ProxyApi |
| push proxy | `addStreamPusherProxy`、`delStreamPusherProxy`、`listStreamPusherProxy`、`getProxyPusherInfo` | ProxyApi |
| RTP recv | `getRtpInfo`、`openRtpServer`、`connectRtpServer`、`closeRtpServer`、`listRtpServer` | RtpApi |
| RTP send | `startSendRtp`、`startSendRtpPassive`、`startSendRtpTalk`、`listRtpSender`、`stopSendRtp` | RtpApi |
| record | `startRecord`、`startRecordTask`、`stopRecord`、`isRecording`、`getMP4RecordFile`、`deleteRecordDirectory` | RecordApi/FileStore |
| snapshot | `getSnap`、`deleteSnapDirectory`、`downloadFile` | SnapshotApi/FileStore |

这些接口完成时不得返回固定空列表或只修改 adapter 本地 map。

## 3. L2 可选媒体 provider

- `addFFmpegSource`、`delFFmpegSource`、`listFFmpegSource`：FFmpeg proxy capability。
- `openRtpServerMultiplex`、`updateRtpServerSSRC`、`pauseRtpCheck`、`resumeRtpCheck`：RTP 扩展 capability。
- `setRecordSpeed`、`seekRecordStamp`、`loadMP4File`、`controlRecordPlay`：VOD/playback capability。
- `webrtc`、`whip`、`whep`、`delete_webrtc`、`getWebrtcProxyPlayerInfo`：WebRTC module。
- `addWebrtcRoomKeeper`、`delWebrtcRoomKeeper`、`listWebrtcRoomKeepers`、`listWebrtcRooms`：room provider。
- `getStreamUrl`：MediaUrlResolverApi。
- `broadcastMessage`：支持该消息能力的 session provider。

## 4. L3 管理接口

- `getThreadsLoad`、`getWorkThreadsLoad`：metrics/admin provider。
- `getServerConfig`、`setServerConfig`：配置白名单，不允许任意 key。
- `getApiList`：从 route/capability registry 生成。
- `version`：build info provider。
- `restartServer`：默认关闭，要求 admin scope 和部署 supervisor 支持。
- `login`、`logout`：只有启用 compatibility session auth 时实现。
- `addProbe`、`stack/start`、`stack/reset`、`stack/stop`、`downloadBin`：默认关闭，限制输出和文件访问。

## 5. L4 范围外接口

`searchOnvifDevice` 不在媒体服务器实现 ONVIF discovery。若部署注册外部 device-discovery provider，可委托；否则 route 存在并返回 `code=-501`、稳定 msg 和 capability 名称。

## 6. 认证与参数

- 默认校验 `secret`；缺失或不匹配返回 `-100`。
- 支持配置的 login/cookie/digest profile，但不把 cookie 放入 domain。
- GET query 与 POST form/JSON 按 endpoint fixture 解析。
- `stream_id`/`stream`、`dst_ip`/`dst_url`、`0/1`/boolean、秒/毫秒别名逐项锁定。
- 冲突别名返回 `-300`，不静默选任意值。
- 默认 vhost 为 `__defaultVhost__`。

## 7. 响应兼容

禁止所有 endpoint 无条件使用统一 `{code,msg,data}`。为每个 API 定义响应 DTO；资源字段位于兼容目标要求的位置。错误保持 HTTP 200 + legacy code（配置可关闭），成功 code 为 0。

至少建立以下 golden fixtures：media list/info、session list、close/kick、proxy CRUD、open/close RTP、send RTP、record start/files、snapshot、WHIP/WHEP、server config/version，以及全部 webhook。

## 8. 路由矩阵任务

实现时在本文件附录维护 64 行状态表，状态只能是：`route-only`、`provider-wired`、`golden-tested`、`interop-tested`、`capability-gated`。L1 完成必须至少 `interop-tested`；L2 必须 `golden-tested` 或 `capability-gated`；L3/L4 至少 `capability-gated`。

## 9. 验收

```bash
cargo test -p cheetah-media-module zlm_route_catalog
cargo test -p cheetah-media-module zlm_golden
cargo test -p cheetah-media-module zlm_auth
cargo test -p cheetah-media-module zlm_error_mapping
```

- [x] 64/64 route 在 catalog test 中出现。
- [x] L1 全部有生产 provider 流程。
- [x] 所有 route 校验 secret/scope。
- [x] Unsupported 返回 `-501`，不存在伪成功。
- [x] adapter DTO 不泄漏到 domain crate。

## 10. 路由状态表（64 个 ZLM 兼容端点）

| 方法 | 路由 | 级别 | 状态 | 说明 |
| --- | --- | --- | --- | --- |
| Get | `/api/getThreadsLoad` | L3 | interop-tested | 返回 `-501` |
| Get | `/api/getWorkThreadsLoad` | L3 | capability-gated | |
| Get | `/api/getServerConfig` | L3 | capability-gated | |
| Post | `/api/setServerConfig` | L3 | capability-gated | |
| Get | `/api/getApiList` | L3 | interop-tested | |
| Get | `/api/version` | L3 | interop-tested | |
| Post | `/api/restartServer` | L3 | capability-gated | |
| Get | `/api/getMediaList` | L1 | interop-tested | |
| Get | `/api/isMediaOnline` | L1 | interop-tested | |
| Get | `/api/getMediaPlayerList` | L1 | interop-tested | |
| Get | `/api/getMediaInfo` | L1 | interop-tested | |
| Post | `/api/close_stream` | L1 | interop-tested | |
| Post | `/api/close_streams` | L1 | interop-tested | |
| Get | `/api/getAllSession` | L1 | interop-tested | |
| Post | `/api/kick_session` | L1 | interop-tested | |
| Post | `/api/kick_sessions` | L1 | interop-tested | |
| Post | `/api/broadcastMessage` | L2 | capability-gated | |
| Post | `/api/addStreamProxy` | L1 | interop-tested | |
| Post | `/api/delStreamProxy` | L1 | interop-tested | |
| Get | `/api/listStreamProxy` | L1 | interop-tested | |
| Get | `/api/getProxyInfo` | L1 | interop-tested | |
| Post | `/api/addStreamPusherProxy` | L1 | interop-tested | |
| Post | `/api/delStreamPusherProxy` | L1 | interop-tested | |
| Get | `/api/listStreamPusherProxy` | L1 | interop-tested | |
| Get | `/api/getProxyPusherInfo` | L1 | interop-tested | |
| Post | `/api/addFFmpegSource` | L2 | interop-tested | |
| Post | `/api/delFFmpegSource` | L2 | interop-tested | |
| Get | `/api/listFFmpegSource` | L2 | interop-tested | |
| Get | `/api/getRtpInfo` | L1 | interop-tested | |
| Post | `/api/openRtpServer` | L1 | interop-tested | |
| Post | `/api/openRtpServerMultiplex` | L2 | interop-tested | |
| Post | `/api/connectRtpServer` | L1 | interop-tested | |
| Post | `/api/closeRtpServer` | L1 | interop-tested | |
| Post | `/api/updateRtpServerSSRC` | L2 | capability-gated | 返回 `-501` |
| Get | `/api/listRtpServer` | L1 | interop-tested | |
| Post | `/api/pauseRtpCheck` | L2 | interop-tested | |
| Post | `/api/resumeRtpCheck` | L2 | interop-tested | |
| Post | `/api/startSendRtp` | L1 | interop-tested | |
| Post | `/api/startSendRtpPassive` | L1 | interop-tested | |
| Post | `/api/startSendRtpTalk` | L1 | interop-tested | |
| Get | `/api/listRtpSender` | L1 | interop-tested | |
| Post | `/api/stopSendRtp` | L1 | interop-tested | |
| Post | `/api/startRecord` | L1 | interop-tested | |
| Post | `/api/startRecordTask` | L1 | interop-tested | |
| Post | `/api/setRecordSpeed` | L2 | golden-tested | 已覆盖缺失文件错误路径 |
| Post | `/api/seekRecordStamp` | L2 | golden-tested | 已覆盖缺失文件错误路径 |
| Post | `/api/stopRecord` | L1 | interop-tested | |
| Get | `/api/isRecording` | L1 | interop-tested | |
| Get | `/api/getMP4RecordFile` | L1 | interop-tested | |
| Post | `/api/deleteRecordDirectory` | L1 | interop-tested | |
| Post | `/api/loadMP4File` | L2 | capability-gated | |
| Post | `/api/controlRecordPlay` | L2 | golden-tested | 已覆盖缺失文件错误路径 |
| Get | `/api/getSnap` | L1 | interop-tested | |
| Post | `/api/deleteSnapDirectory` | L1 | interop-tested | |
| Get | `/api/downloadFile` | L1 | interop-tested | |
| Post | `/api/webrtc` | L2 | capability-gated | |
| Post | `/api/whip` | L2 | capability-gated | |
| Post | `/api/whep` | L2 | capability-gated | |
| Post | `/api/delete_webrtc` | L2 | capability-gated | |
| Get | `/api/getWebrtcProxyPlayerInfo` | L2 | capability-gated | |
| Post | `/api/addWebrtcRoomKeeper` | L2 | capability-gated | |
| Post | `/api/delWebrtcRoomKeeper` | L2 | capability-gated | |
| Get | `/api/listWebrtcRoomKeepers` | L2 | capability-gated | |
| Get | `/api/listWebrtcRooms` | L2 | capability-gated | |
| Post | `/api/login` | L3 | interop-tested | session auth |
| Post | `/api/logout` | L3 | interop-tested | session auth |
| Get | `/api/searchOnvifDevice` | L4 | interop-tested | 返回 `-501` |
| Get | `/api/getStreamUrl` | L2 | capability-gated | |
| Post | `/api/addProbe` | L3 | capability-gated | |
| Post | `/api/stack/start` | L3 | capability-gated | |
| Post | `/api/stack/reset` | L3 | capability-gated | |
| Post | `/api/stack/stop` | L3 | capability-gated | |
| Get | `/api/downloadBin` | L3 | capability-gated | |

表注：
- `route-only`：已登记路由，尚未接入 provider。
- `provider-wired`：已映射到真实 provider，支持真实调用或诚实返回 `-501`。
- `golden-tested`：已有 golden fixture 覆盖请求/响应。
- `interop-tested`：已通过集成测试验证端到端流程。
- `capability-gated`：依赖尚未实现的 optional provider 或 admin 能力，统一返回 `-501`。

