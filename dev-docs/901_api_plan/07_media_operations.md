# 07. 媒体操作专项设计

## 1. 录制

统一任务模型：

- `RecordTask`：task id、media key、format、state（pending/running/stopping/completed/failed/cancelled）、started/ended、duration、file count、error。
- `RecordFile`：file id、task id、media key、format、path handle、year/month/day、start/end、duration、size、download URL（由 adapter 生成）。
- `RecordTemplate`：continuous、segment、event；segment duration/count 必须有上限。

必须支持：开始/停止、MP4/TS/FLV/HLS/fMP4 capability 查询、按时间和媒体查询文件、删除文件/目录、倍速、seek、文件播放控制。录制帧流使用 codec 的 `AVFrame`；record module 不复制时间戳修正和参数集缓存。

幂等规则：相同 media key + template + idempotency key 的重复 start 返回原 task；stop 已完成任务返回最终状态，不重复删除或产生新文件。

## 2. 快照

抓图必须定义关键帧等待、超时、无视频轨、解码失败和存储失败的状态。`SnapshotHandle` 不直接暴露绝对路径；下载通过受保护的 file handle。快照目录清理按 media key、时间范围和保留策略执行，不能接受任意 filesystem path。

## 3. 拉流与推流代理

代理模型统一包含 source、destination media key、state、retry/heartbeat、last error、created/updated、derived sessions 和 output URLs。支持的 source scheme 与协议 capability 由 provider 声明；未知 scheme 返回 `Unsupported`。

兼容实现需要覆盖：RTSP/RTSPS、RTMP/RTMPS、HTTP/HTTPS、文件/FFmpeg source，以及推流目标。ABL 参考实现中出现的 `enable_mp4`、`enable_hls`、`disableVideo`、`disableAudio`、转码宽高、G711 转 AAC、文件格式和保留时间等字段，全部进入显式 `TranscodePolicy`、`OutputPolicy` 或 `StoragePolicy`，禁止保留一个无限扩张的 JSON options。

## 4. RTP

RTP receiver 生命周期：allocate port → bind → wait/connect → bind media → receive → timeout/close。RTP sender 生命周期：resolve media → allocate sender → active/passive/talk start → send → stop。端口、SSRC、RTCP、TCP mode、reuse-port、payload type 必须在创建时校验。

统一状态包括 `Created`、`Listening`、`Connected`、`Bound`、`Paused`、`TimedOut`、`Stopping`、`Stopped`、`Failed`。`pauseRtpCheck` 只改变健康检查策略，不应停止收包；`updateRtpServerSSRC` 记录审计并发布状态事件。

GB28181 项目可使用 RTP receiver 发布 PS/raw 轨道，也可使用 RTP sender 输出到设备。talk 模式是 RTP media capability，不是 GB 信令实现。

## 5. 播放输出与协议绑定

RTSP、RTMP、HTTP-FLV、HLS、WebRTC、WHIP、WHEP、SRT 等输出都必须以 `MediaKey + output schema + auth context` 请求媒体订阅。URL 生成是 adapter/provider 责任；核心只返回 typed `MediaUrl` 和 session handle。

## 6. 服务器运维

线程负载、工作线程负载、版本、server config、端口列表、重启/关闭属于 control capability，不应为了兼容 API 直接写进 MediaControlApi。ZLM adapter 可通过单独的 `ServerAdminApi` 组合进 facade；缺少能力时返回 unsupported。

