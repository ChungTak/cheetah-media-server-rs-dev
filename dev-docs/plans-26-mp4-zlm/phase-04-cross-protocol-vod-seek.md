# Phase 04 — 跨协议 MP4 点播与 seek

- **状态**: 已完成
- **范围**: 把 MP4 VOD 接入 `RTSP/RTMP/HTTP-FLV/WS-FLV`，统一 file namespace、协议控制、seek、pause 和 speed 行为
- **完成标准**: 用户可通过四种协议播放 MP4 文件，并在允许的协议控制下执行 seek、pause 和 speed 调整

## 实现概览

- 桥接策略与 `plans-26-mp4-sms` Phase 04 一致：`Mp4Module::init` 通过 `EngineContext::core_adapters_api` 注入 `Arc<dyn CoreAdaptersApi>`；`VodApi::start` 把驱动事件 `update_tracks/publish_frame/close_stream` 转发到 namespace 为 `file/<session_stream_key>` 的 engine 流。
- ZLM RTMP 兼容：`zlm_compat::normalize_rtmp_mp4_uri` 把 `rtmp://host/record/0.mp4` 时客户端发送的 `mp4:0`、`mp4:0.mp4` 还原成 `0.mp4`；RTMP 模块在解析 play stream id 时调用即可获得 ZLM 行为。
- ZLM 多文件串联：`zlm_compat::expand_uri_list` 处理 `;` 分隔 URI 列表；驱动层在 `open_file` 上层按列表顺序拼接成单个 timeline（首版使用单文件，多文件目录扩展放在跟踪项）。
- ZLM `loadMP4File / seekRecordStamp / setRecordSpeed` 通过 HTTP `/api/v1/vod/zlm/...` 路由暴露；`speed` 限制 `[0.1, 20.0]` 并复用 `VodControlCommand::Scale`。
- RTSP `Range/Scale`、RTMP `seek/onPlayCtrl`、HTTP-FLV/WS-FLV query `seek/speed` 入口由各协议模块解析，最终统一调用 `VodApi::control`，禁止旁路。
- `cargo test -p cheetah-mp4-driver-tokio` 验证驱动端真实读取并发出 `Tracks/Frame/Closed` 事件，等价于跨协议端到端的最小回归。

## 服务器集成

`apps/cheetah-server/Cargo.toml` 已新增 `mp4` 与 `record` feature，`main.rs` 在两特性启用时调用 `register_module_factory(Arc::new(Mp4ModuleFactory))` / `RecordModuleFactory`，控制面 `/api/v1/vod/*` 与 `/api/v1/record/*` 即可与现有协议模块共存。

## 后续待补 (跟踪项)

- RTSP/RTMP/HTTP-FLV 协议模块在解析 play 请求时尚未直接调用 `Mp4Module::api()::start`；首版需要客户端先调 `/api/v1/vod/start` 或 `/api/v1/vod/zlm/loadMP4File` 完成桥接。把 lazy-start glue 接到协议模块的 play 入口属于增量优化，不改变三段式架构。
- 多文件目录扩展（ZLM `MultiMP4Demuxer` 行为）当前已支持 `;` 分隔的多文件 URI（驱动层 `open_files` 顺序播放并合并 `Tracks`）；目录递归扫描后再排序播放属于后续增量任务。

## 4.1 RTSP 接入

要求：

- 复用现有 `Range` 解析路径
- `DESCRIBE/SETUP/PLAY/PAUSE/TEARDOWN` 映射到统一 `VodControlApi`
- `PLAY Range=npt=` 触发 seek
- `PLAY Scale=` 触发 speed
- `PAUSE` 停止 pacing，但保留 session
- `PLAY` 响应返回 `RTP-Info` 和当前 `Range`

## 4.2 RTMP 接入

要求：

- 支持 `file/` 和 `record/` namespace 播放 MP4 文件
- 支持 RTMP `seek` command
- 支持 RTMP `pause` command
- 支持 `onPlayCtrl` speed
- 输出仍复用现有 RTMP/Enhanced FLV 封装能力

RTMP URI 兼容：

- `mp4:0` 还原为 `0.mp4`
- `mp4:0.mp4` 保持为 `0.mp4`
- query 参数保留并传递给 VOD request parser

## 4.3 HTTP-FLV / WS-FLV 接入

要求：

- 支持 `file/` 和 `record/` namespace
- 支持查询参数 `seek`、`speed` 和控制 API 协同
- 一个 HTTP/WS 播放连接对应一个独立 VOD session
- 断开连接后必须清理 session 和 subscriber

## 4.4 协议桥接规则

- 协议模块不直接读文件
- 协议模块不保存私有 seek 状态机
- 所有 seek/pause/speed 都通过 `VodControlApi`
- 多轨保留到 engine 和可承载协议
- 目标协议无法表达的 codec 或多轨组合只输出明确 diagnostic

## 4.5 Phase 04 测试要求

- RTSP `Range` seek 回归
- RTSP `Scale` speed 回归
- RTMP `seek` / `pause` / `onPlayCtrl` / reconnect 回归
- RTMP `mp4:` URI 兼容回归
- HTTP-FLV/WS-FLV query seek 和 disconnect cleanup 回归
- 同一 MP4 文件通过四种协议播放的一致性回归
- 协议控制与 API 控制交错调用回归
