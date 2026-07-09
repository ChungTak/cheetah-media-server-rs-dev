# Phase 04: 跨协议 MP4 点播与 seek 控制

- **状态**: 已完成
- **目标**: 让 `RTSP/RTMP/HTTP-FLV/WS-FLV` 复用同一 MP4 VOD source，并对齐协议侧 seek/pause/speed 行为
- **完成标准**: 四类协议都能播放 MP4 文件并共享统一控制模型

## 实现概览

- 桥接策略与 SMS / ZLM 计划一致：`Mp4Module::init` 注入 `EngineContext::core_adapters_api`，`VodApi::start` 把驱动事件 `update_tracks/publish_frame/close_stream` 转发到 namespace `file/<path>`，RTSP/RTMP/HTTP-FLV/WS-FLV 通过订阅 engine stream 自然消费。
- ZLM RTMP `mp4:` URI 还原与 `;` 分隔多文件清单已在 `zlm_compat::normalize_rtmp_mp4_uri` / `expand_uri_list` 落地，并在 `VodApi::resolve_path` 中作为唯一入口被复用，覆盖 ABL 直接传 `mp4:` 风格 stream id 的客户端。
- ABL `on_rtsp_replay` 风格审计字段 (`reader_count / remote_ip / remote_port / network_type / params`) 已加入 `VodSessionRecord`；协议模块在 attach/detach 时可以更新这些字段，控制面查询 list 时即可附带回放上下文。
- 控制错误统一路径：core 的 `VodDiagnostic` 经驱动层 `VodDriverEvent::Diagnostic` 透传到 module，再由协议端映射为 RTSP/RTMP 错误响应或 HTTP `result.code` 字段。
- ZLM `loadMP4File / seekRecordStamp / setRecordSpeed` 的 `[0.1, 20.0]` 限速以及 `read_count` 入参共用同一份驱动配置，ABL 的 `readMp4FileCount` 可通过 `StartVodRequest::loop_count` 直接表达。

## 后续待补 (跟踪项)

- RTSP/RTMP 协议模块在解析 play 请求时仍需要客户端先调 `/api/v1/vod/start` 或 `/api/v1/vod/zlm/loadMP4File`；自动 lazy-start glue 是协议侧增量。
- ABL 的 HTTP-MP4 chunked 输出需要在 HTTP-FLV 模块以外开通 `HTTP-MP4` 协议，属于 Phase 05 之后的 P2 任务。

## RTSP

1. 把 `PLAY Range: npt=` 映射到 `VodControlApi::seek`
2. 把 `Scale` 映射到 `VodControlApi::set_speed`
3. `PAUSE` / `PLAY` 映射到 `pause` / `resume`
4. `DESCRIBE` 返回文件轨道信息和 duration

## RTMP

1. `play` 支持文件 namespace 或控制面注入的 stream key
2. `seek` command 映射到统一 seek
3. `pause` 与 `onPlayCtrl` 映射 pause/resume/speed
4. 兼容 `mp4:` 风格命名和扩展名差异

## HTTP-FLV / WS-FLV

1. 通过 URL 参数或控制 API 绑定点播 session
2. 初始播放支持 `start_ms`
3. 已建立连接的 seek 通过控制面下发，不强制重建 writer
4. 断开后引用计数归零时允许 driver 进入空闲关闭

## 统一约束

1. 所有协议只消费 `AVFrame + TrackInfo`
2. 控制面错误统一映射为明确协议响应，不做 silent ignore
3. 协议层不自行实现文件 seek 逻辑，只做命令翻译
4. 读者计数和远端地址统一汇总到 replay 事件

## ABL 对齐要求

1. 预留 `readerCount`、`ip`、`port`、`networkType`、`params` 字段
2. `describe`、`scale`、`stop` 等控制事件进入统一审计流
3. seek 后各协议都应收到参数集补发和新的时间线起点

## 测试要求

1. RTSP 集成测试覆盖 DESCRIBE、PLAY、PAUSE、Range、Scale
2. RTMP 集成测试覆盖 play、seek、pause、onPlayCtrl
3. HTTP-FLV / WS-FLV 集成测试覆盖起播、断开、seek 控制
4. 跨协议一致性测试覆盖同一文件在四类协议上的 duration、seek 结果和 EOF 行为
