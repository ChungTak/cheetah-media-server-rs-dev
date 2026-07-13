# 08. 面向信令项目的媒体调用契约

## 1. 总原则

本项目提供“信令完成后如何操作媒体”的端口，不实现信令本身。外部项目负责设备/用户身份、SIP/HAP/Matter/ONVIF 状态机和协议消息；调用 Cheetah 时使用 native domain facade 或 SDK facade，不依赖 ZLM JSON。

## 2. GB28181

外部 GB28181 项目需要的最小流程：

1. SIP/设备协商完成后，调用 `open_rtp_receiver`，指定 `MediaKey`、端口策略、SSRC、payload、TCP mode 和 codec hints。
2. RTP 收到后由 RTP module 进入 engine，统一产出 `AVFrame + TrackInfo`。
3. 外部项目调用 `get_media`/`is_media_online` 获取状态，并通过输出订阅生成 RTSP/HTTP-FLV/WebRTC/HLS 等播放地址。
4. 设备注销、超时或 BYE 时调用 `stop_rtp_session` 或 `close_handle`。
5. 设备回放使用 record file query + playback control；语音对讲使用 RTP sender talk capability。

项目必须能关联 `device_id/channel_id`，但该字段只作为 `MediaRequestContext` 或受限 metadata，不把 SIP 目录模型塞进 MediaKey。

## 3. ONVIF

外部 ONVIF 项目负责设备发现、Profile、PTZ、事件和鉴权。媒体侧只提供：

- 根据 device/channel 选定或创建 MediaKey 的约定。
- 查询在线媒体和输出 URL。
- 创建/停止 RTP receiver 或 pull proxy。
- 请求快照、查询快照、下载句柄。
- 请求关键帧和关闭媒体会话。

`searchOnvifDevice` 只能作为兼容占位，不能让 Cheetah 依赖 ONVIF SOAP crate。

## 4. Apple HomeKit

外部 HAP 项目负责配对、Accessory、SRTP 参数、摄像头配置和双向音频协商。媒体侧需要：

- 使用 MediaKey 打开视频/音频 subscriber。
- 声明 H264/H265/AAC/Opus 等输出轨道能力并请求 keyframe。
- 提供有界的 RTP/SRTP packetization 输入输出桥接句柄；SRTP 密钥和 HAP session 只保留在外部项目或专用 adapter。
- 会话结束时关闭 subscriber，慢消费者触发有界丢弃/断开。

domain 不暴露 HomeKit characteristic 或 HAP JSON。

## 5. Matter

外部 Matter 项目负责 endpoint、cluster、commissioning 和用户控制。媒体侧只提供：

- 媒体资源发现和能力查询。
- 创建/停止播放订阅、抓图、录制任务和事件订阅。
- 通过统一 metadata/health 查询状态，不引入 Matter cluster 类型。

## 6. 外部项目接入约定

- 外部项目优先使用 native/domain API；只有需要复用现有 ZLM 管理台时使用 compatibility API。
- 所有创建命令携带 idempotency key 和 correlation id。
- 外部项目不得直接访问 engine internals、codec 私有缓存或 RTP socket。
- API 版本与 capability 先协商；unsupported 必须可区分于 transient unavailable。
- 事件消费采用 event id 去重，并在信令侧维护自己的状态机。

