# 12 · 第三方信令服务器生产合同

## 1. 两层合同与公共 fixture

每类集成必须各有两套测试：A 层为同进程 runtime-neutral Rust SDK；B 层启动完整服务器，由独立客户端仅通过 native HTTP 和网络媒体端口操作。fake tests 只用于开发反馈，不计生产合同。

公共 fixture 生成或携带有效 H264/AAC/G.711/PS/MJPEG/MP4，能够由独立 parser/decoder 验证。禁止以任意字节加 codec 标签代替媒体。所有服务使用动态端口、临时 managed root、固定超时和测试专用 CIDR allowlist。

## 2. GB28181 媒体合同

- 创建动态和指定端口 receiver，覆盖 UDP、TCP passive、TCP active。
- 发送有效 RTP/PS，断言 online、video/audio TrackInfo、AVFrame 和时间戳单调。
- 查询 URL、请求关键帧、开始/停止录制并解析录制文件。
- sender 对端解析 RTP；talk 对端解析 G.711。
- 更新 SSRC/PT 后旧包不再接收、新包继续出帧。
- inactivity、DELETE、module restart 后 session、publisher、socket、port lease 全部释放。

## 3. ONVIF 媒体合同

启动本地 RTSP 源；默认 SSRF 拒绝一次，再配置测试 allowlist 成功拉流。断言 proxy Connected、媒体 track/frame、播放 URL、关键帧、JPEG 下载和录制文件。删除 proxy/record/snapshot 后检查连接、订阅、临时文件和元数据清理。

这套合同只代表设备发现/配置已由外部信令项目完成，输入是合法 RTSP URL 和凭据引用。

## 4. HomeKit 媒体合同

通过正式 `MediaDataPlaneApi` 订阅视频和音频，不直接依赖 engine 私有 subscriber。验证 TrackInfo、关键帧请求、时间戳、音视频并行、慢订阅者隔离和 session close 自动注销。外发路径创建 RTP sender，由网络对端验证 packetization；取消后不再收到数据。

## 5. Matter 媒体合同

查询 capability/details 和媒体资源；获取可用 URL；触发 snapshot、record、playback，并真实订阅 `StreamOnlineChanged`、`SnapshotCompleted`、`RecordCompleted`。逐项校验 event id、resource key、file handle 和时间，取消 subscription 后确保不再投递。

## 6. 共通负向与 DoD

每套合同覆盖无凭据、缺 scope、错误 resource grant、过期 deadline、同幂等键重复/冲突、provider unavailable、module restart。B 层不得访问进程内 provider 来完成操作，只能在结束时读取测试观测器判断资源泄漏。

任务：`SIG-01` 公共真实素材；`SIG-02` GB；`SIG-03` ONVIF；`SIG-04` HomeKit；`SIG-05` Matter；`SIG-06` native HTTP 黑盒 runner。

DoD：四类 A/B 合同全部通过，任何伪 payload、fake provider、只断言 HTTP 200 或只断言 session 存在的测试均不计入。
