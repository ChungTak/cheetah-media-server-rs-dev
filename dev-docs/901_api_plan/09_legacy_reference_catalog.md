# 09. 已整理的参考接口目录

本章是实现所需的脱离源码目录的参考摘要。它描述兼容目标，不表示内部设计必须照搬这些 JSON。

## 1. 成熟流媒体服务器的通用能力

常见能力可以归纳为：媒体列表/详情/在线判断、播放器和 session 管理、关闭流、拉流 proxy、推流 proxy、FFmpeg source、RTP server/client、RTP sender、录制任务、MP4 文件、快照、文件下载、WebRTC/WHIP/WHEP、room keeper、线程/配置/版本和 webhook。

另一个常见的资源型 API 采用以下路径族：

- `/api/v1/record/start`、`stop`、`list`、`query`、`file/query`、`file/delete`。
- `/api/v1/rtp/server/create`、`server/stop`、`client/create`、`client/start`、`client/stop`。
- `/api/v1/gb28181/recv/create`、`recv/stop`、`send/create`、`send/stop`。
- `/api/v1/rtsp/play/*`、`publish/*`、`server/create`、`server/stop`。
- `/api/v1/rtc/play`、`rtc/publish`。
- `/api/v1/srt/pull/*`、`srt/push/*`。
- `/api/v1/jt1078/create`、`port/info`、`send/start`、`send/stop`、`server/open`、`server/close`、`talk/start`、`talk/stop`。
- `/api/v1/down/stream/*`、`down/device/*`、`media/server/*`、`signal/server/*`、`cloud/record/*`、`cloud/collect/*`。

这类接口常见字段：`appName`、`streamName`、`format`、`duration`、`segmentDuration`、`segmentCount`、`taskId`、`fileId`、`starttime`、`endtime`、`protocol`、`uri`、`vhost`、`params`、`authResult`、`stop`、`delay`、`serverId`、`playerCount`、`memUsage`。native API 使用自己的 typed 名称；compat adapter 负责别名转换。

## 2. ZLMediaKit 兼容接口目录

必须保留的 `/index/api` 族：

- 系统：`getThreadsLoad`、`getWorkThreadsLoad`、`getServerConfig`、`setServerConfig`、`getApiList`、`restartServer`、`version`。
- 媒体：`getMediaList`、`isMediaOnline`、`getMediaPlayerList`、`getMediaInfo`、`close_stream`、`close_streams`、`getAllSession`、`kick_session`、`kick_sessions`、`broadcastMessage`。
- proxy：`addStreamProxy`、`delStreamProxy`、`listStreamProxy`、`getProxyInfo`、`addStreamPusherProxy`、`delStreamPusherProxy`、`listStreamPusherProxy`、`getProxyPusherInfo`、`addFFmpegSource`、`delFFmpegSource`、`listFFmpegSource`。
- RTP：`getRtpInfo`、`openRtpServer`、`openRtpServerMultiplex`、`connectRtpServer`、`closeRtpServer`、`updateRtpServerSSRC`、`listRtpServer`、`pauseRtpCheck`、`resumeRtpCheck`、`startSendRtp`、`startSendRtpPassive`、`startSendRtpTalk`、`listRtpSender`、`stopSendRtp`。
- record：`startRecord`、`startRecordTask`、`setRecordSpeed`、`seekRecordStamp`、`stopRecord`、`isRecording`、`getMP4RecordFile`、`deleteRecordDirectory`、`loadMP4File`。
- snapshot/file：`getSnap`、`deleteSnapDirectory`、`downloadFile`。
- WebRTC：`webrtc`、`whip`、`whep`、`delete_webrtc`、`getWebrtcProxyPlayerInfo`。
- room：`addWebrtcRoomKeeper`、`delWebrtcRoomKeeper`、`listWebrtcRoomKeepers`、`listWebrtcRooms`。
- 可选：`login`、`logout`、`searchOnvifDevice`、`getStreamUrl`、`addProbe`、`stack/start`、`stack/reset`、`stack/stop`、`downloadBin`。

必须考虑的 `/index/hook` 族：`on_publish`、`on_play`、`on_flow_report`、`on_rtsp_realm`、`on_rtsp_auth`、`on_stream_changed`、`on_stream_not_found`、`on_record_mp4`、`on_record_ts`、`on_shell_login`、`on_stream_none_reader`、`on_http_access`、`on_server_started`、`on_server_exited`、`on_server_keepalive`、`on_send_rtp_stopped`、`on_rtp_server_timeout`。

## 3. 另一套兼容参考的扩展字段

代理和转码常见字段包括 `enable_mp4`、`enable_hls`、`isRtspRecordURL`、`convertOutWidth`、`convertOutHeight`、`H264DecodeEncode_enable`、`disableVideo`、`disableAudio`、`optionsHeartbeat`、`fileKeepMaxTime`、`videoFileFormat`、`G711ConvertAAC`、`clock`、`scale`、`readMp4FileCount`。

RTP 常见字段包括 `port`、`port2`、`enable_tcp`（0/1/2）、`payload`、`RtpPayloadDataType`（1-4）、`send_app`、`send_stream_id`、`dst_url`、`dst_port`、`jtt1078_version`、`detectSendAppStream`、`jtt1078_KeepOpenPortType`。这些字段必须映射到 typed policy；不允许作为未校验 map 进入 provider。

录制播放控制使用 command `pause`、`resume`、`scale`、`seek`；`scale` 和 `seek` 必须提供 value。快照查询支持 `timeout_sec`、`captureReplayType`、start/end time；响应应由 adapter 生成安全 file handle。

## 4. 参考错误和回调形状

一套兼容接口使用 `code`、`msg`，另一套使用 `code`、`memo`、`key`。兼容 adapter 必须按 profile 输出，domain 只保留稳定错误。播放/发布回调通常传 protocol、type、uri、vhost、params，响应传 auth result/code/msg；none-player 回调传 stop/delay；keepalive 传 server id、origin count、player count、memory usage；stream status 传 on/off/error code。

