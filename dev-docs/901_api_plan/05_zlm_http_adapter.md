# 05. ZLMediaKit HTTP Compatibility Adapter

## 1. 目标与默认行为

该 adapter 的职责是把既有 ZLMediaKit 管理台、SDK、监控 exporter 和 webhook 调用翻译为 domain port。默认行为如下：

- 路径保持 `/index/api/*` 和 `/index/hook/*`；部署可以配置整体前缀，但默认不增加 `/api/v1`。
- 参数名、路径变量、常用 JSON 字段和 webhook 名称保持兼容。
- 兼容响应默认 HTTP 200，业务结果通过 `code`/`msg` 表示；成功为 `code=0`，失败使用稳定映射码。
- API secret、登录 cookie/digest、`enable`/`vhost`/`app`/`stream` 等历史参数在 adapter 内解析；domain 只接收已校验的 principal 和 typed request。
- adapter 只翻译一次。record module 中旧 `/zlm/*` route 在迁移期间转发到该 adapter，不得保留另一套 record 实现。

建议将兼容 profile 配置为：`enabled`、`path_prefix`、`secret`、`auth_mode`、`strict_fields`、`legacy_http_200`、`webhook_timeout`、`allow_dangerous_api`。

## 2. 响应与错误映射

兼容成功最少包含：

```json
{"code": 0, "msg": "success"}
```

必要时追加资源字段；不得把内部错误栈或 token 回显给客户端。建议保留以下稳定类别：

| domain error | legacy code | 说明 |
| --- | ---: | --- |
| Success | 0 | 成功 |
| OtherFailed | -1 | 未分类失败 |
| AuthFailed | -100 | secret/登录/权限失败 |
| StorageFailed | -200 | 文件索引或存储失败 |
| InvalidArgument | -300 | 参数错误 |
| Internal/ProtocolFailed | -400 | 执行异常或协议失败 |
| NotFound | -500 | 资源不存在 |
| Unsupported | -501 | Cheetah 尚未实现，不能伪装成功 |

兼容响应可追加 `request_id`，但不能删除旧客户端依赖的 `code`、`msg`、`data` 或既有字段。

## 3. API 全目录

### 3.1 系统与配置

- `/index/api/getThreadsLoad`
- `/index/api/getWorkThreadsLoad`
- `/index/api/getServerConfig`
- `/index/api/setServerConfig`
- `/index/api/getApiList`
- `/index/api/version`
- `/index/api/restartServer`

`getServerConfig` 和 `setServerConfig` 只允许暴露经过白名单的配置项；禁止把任意配置 key 直接写入 engine。`restartServer` 应转换为受保护的 lifecycle command；若当前进程不能安全重启，返回 `-501` 并说明 capability。

### 3.2 媒体、会话和广播

- `/index/api/getMediaList`
- `/index/api/isMediaOnline`
- `/index/api/getMediaPlayerList`
- `/index/api/getMediaInfo`
- `/index/api/close_stream`
- `/index/api/close_streams`
- `/index/api/getAllSession`
- `/index/api/kick_session`
- `/index/api/kick_sessions`
- `/index/api/broadcastMessage`

媒体查询参数至少支持 `vhost`、`app`、`stream`、`schema`、`origin`、`regist`、`secret`。`close_stream` 映射为指定媒体关闭报告；`kick_session` 映射为 session close。广播只能作用于授权的 session/channel，并明确返回发送数量。

### 3.3 拉流、推流和 FFmpeg 代理

- `/index/api/addStreamProxy`
- `/index/api/delStreamProxy`
- `/index/api/listStreamProxy`
- `/index/api/getProxyInfo`
- `/index/api/addStreamPusherProxy`
- `/index/api/delStreamPusherProxy`
- `/index/api/listStreamPusherProxy`
- `/index/api/getProxyPusherInfo`
- `/index/api/addFFmpegSource`
- `/index/api/delFFmpegSource`
- `/index/api/listFFmpegSource`

拉流 proxy 字段转换：`url/source_url`、`vhost`、`app`、`stream`、`retry`、`timeout`、`enable_hls`、`enable_mp4`、`rtp_type`、`enable_rtsp`、transcode/filter 参数进入 `PullProxyRequest`。推流 proxy 进入 `PushProxyRequest`；FFmpeg source 进入受控的 `FfmpegProxyRequest`，不接受任意 shell 参数。

### 3.4 RTP server/client

- `/index/api/getRtpInfo`
- `/index/api/openRtpServer`
- `/index/api/openRtpServerMultiplex`
- `/index/api/connectRtpServer`
- `/index/api/closeRtpServer`
- `/index/api/updateRtpServerSSRC`
- `/index/api/listRtpServer`
- `/index/api/pauseRtpCheck`
- `/index/api/resumeRtpCheck`
- `/index/api/startSendRtp`
- `/index/api/startSendRtpPassive`
- `/index/api/startSendRtpTalk`
- `/index/api/listRtpSender`
- `/index/api/stopSendRtp`

兼容输入字段包括 `vhost`、`app`、`stream_id/stream`、`port`、`port2`、`enable_tcp`、`tcp_mode`、`reuse_port`、`ssrc`、`rtcp_port`、`payload_type`、`dst_url`、`dst_port`、`is_udp`、`only_track`。必须统一转换到 RtpReceiver/ RtpSender request，并校验端口、SSRC、IP、TCP mode 的组合。实际 RTP 收发由 RTP module/driver 完成。

### 3.5 录制与文件

- `/index/api/startRecord`
- `/index/api/startRecordTask`
- `/index/api/setRecordSpeed`
- `/index/api/seekRecordStamp`
- `/index/api/stopRecord`
- `/index/api/isRecording`
- `/index/api/getMP4RecordFile`
- `/index/api/deleteRecordDirectory`
- `/index/api/loadMP4File`
- `/index/api/controlRecordPlay`

`startRecord`、`startRecordTask` 必须转换成 `StartRecordRequest`；`setRecordSpeed` 与 `seekRecordStamp` 转换成 playback command；文件查询支持按 vhost/app/stream、开始结束时间、format、目录和 file id 过滤。旧 record route 的 DTO 只能在这里定义。

### 3.6 快照与文件下载

- `/index/api/getSnap`
- `/index/api/deleteSnapDirectory`
- `/index/api/downloadFile`

`getSnap` 支持 `timeout_sec`、`expire_sec`、`scale`、`quality` 等兼容字段；必须转换为 SnapshotRequest，并受下载/存储权限保护。`downloadFile` 只接受服务端生成的 file id 或安全路径句柄，不能允许任意路径读取。

### 3.7 WebRTC/WHIP/WHEP

- `/index/api/webrtc`
- `/index/api/whip`
- `/index/api/whep`
- `/index/api/delete_webrtc`
- `/index/api/getWebrtcProxyPlayerInfo`

这些 route 只负责会话创建、SDP/ICE 透传和媒体 key 绑定。WebRTC 状态机仍由 WebRTC core/driver/module 实现；domain 只接收 typed session request 和返回 session handle。SDP 不得写入 `StreamInfo` 的通用 metadata。

### 3.8 WebRTC room keeper

- `/index/api/addWebrtcRoomKeeper`
- `/index/api/delWebrtcRoomKeeper`
- `/index/api/listWebrtcRoomKeepers`
- `/index/api/listWebrtcRooms`

若当前系统没有 room provider，返回 `-501`；不得以空列表伪装能力存在。未来实现应把 room keeper 建模为独立 capability，不将其混进普通媒体流。

### 3.9 其他可选 API

以下接口纳入目录但按 capability 开关实现：`login`、`logout`、`searchOnvifDevice`、`getStreamUrl`、`addProbe`、`stack/start`、`stack/reset`、`stack/stop`、`downloadBin`。其中 `searchOnvifDevice` 仅允许作为外部信令/设备项目的兼容占位，不在本项目实现 ONVIF。

## 4. 兼容字段规则

- 同义参数按优先级解析：显式 typed 参数 > 约定别名 > URL 推导；冲突时报 `-300`。
- 布尔值兼容 `0/1`、`true/false`，输出按旧 profile 固定一种格式。
- 缺省 vhost 使用部署配置的默认值；不得把空字符串当成任意 vhost。
- `stream_id` 与 `stream` 互为兼容别名，但内部只保留 `MediaKey.stream`。
- 所有时间字段记录单位（秒、毫秒或 RFC3339）；adapter 不得静默把毫秒当秒。

