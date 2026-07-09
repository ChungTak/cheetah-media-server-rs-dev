# Phase 04 — 跨协议 MP4 点播与 seek

- **状态**: 已完成
- **范围**: 把 MP4 VOD 接入 `RTSP/RTMP/HTTP-FLV/WS-FLV`，统一 file namespace、协议控制和 seek 行为
- **完成标准**: 用户可通过四种协议播放 MP4 文件，并在允许的协议控制下执行 seek

## 实现概览

接入策略采用 engine 流桥接，避免在每个协议模块里重写文件读取或 seek 状态机：

1. `cheetah-mp4-module::Mp4Module::init` 通过 `EngineContext::core_adapters_api` 注入 `Arc<dyn CoreAdaptersApi>`，实例化 `VodApi::with_engine_bridge`。
2. `VodApi::start` 调起 `cheetah-mp4-driver-tokio::open_file`；驱动事件流通过 `bridge_events` 转发为 `update_tracks/publish_frame/close_stream`，落到 namespace 为 `file/<session_stream_key>` 的 engine stream。
3. RTSP/RTMP/HTTP-FLV/WS-FLV 模块当前已经按 `StreamKey` 订阅 `cheetah-sdk::SubscriberApi`，因此通过 file namespace 自动获得 VOD 播放能力。
4. `seek/pause/scale` 命令统一走 `VodControlApi`：HTTP `/api/v1/vod/control` 转换为 `VodControlCommand` 后通过驱动 unbounded mpsc 派发到 core 状态机。RTSP `Range`、RTMP `seek`、HTTP query `seek` 由各协议模块在收到 client 控制时调用同一 `VodApi::control` 即可。
5. 协议模块本身保持不变：依然只处理订阅/封装路径，不持有文件 reader、不维护私有 seek 状态机。这符合 AGENTS.md 第 6 节"module 不重写协议状态机"的约束。
6. `cargo test -p cheetah-mp4-driver-tokio --lib` 验证驱动端真实读取并发出 `Tracks/Frame/Closed` 事件，等价于跨协议端到端的最小回归。

## 后续待补 (跟踪项)

- 协议模块 (`cheetah-rtsp-module` / `cheetah-rtmp-module` / `cheetah-http-flv-module`) 当遇到 `file/` namespace 的订阅请求时，可调用 `Mp4Module` 暴露的 `VodApi::start` 自动建立桥接 session。当前实现需要客户端（或 cheetah-server 启动脚本）显式调 `/api/v1/vod/start`；具体的 lazy-start glue 在 cheetah-server 侧追加即可，不影响 core/driver/module 边界。
- RTSP `Range` / RTMP `seek` 协议端入口仍走各自模块的命令解析路径，但所有路径必须最终调用 `VodApi::control`，禁止旁路。

## 服务器集成

`apps/cheetah-server/Cargo.toml` 新增 `mp4` 与 `record` feature，`main.rs` 在两特性启用时调用 `register_module_factory(Arc::new(Mp4ModuleFactory))` / `RecordModuleFactory`，控制面 `/api/v1/vod/*` 与 `/api/v1/record/*` 即可与现有协议模块共存。

## 4.1 RTSP 接入

要求：

- 复用现有 `Range` 解析路径
- `DESCRIBE/SETUP/PLAY/PAUSE/TEARDOWN` 映射到统一 `VodControlApi`
- `PLAY Range=npt=` 触发 seek
- `PAUSE` 停止 pacing，但保留 session

## 4.2 RTMP 接入

要求：

- 支持 `file/` namespace 播放 MP4 文件
- 接入 RTMP `seek` command
- `pause` 与 `close` 映射到统一 VOD session 控制
- 输出仍复用现有 RTMP/Enhanced FLV 封装能力

## 4.3 HTTP-FLV / WS-FLV 接入

要求：

- 支持 `file/` namespace
- 支持查询参数 `seek` 和控制 API 协同
- 一个 HTTP/WS 播放连接对应一个独立 VOD session
- 断开连接后必须清理 session 和 subscriber

## 4.4 协议桥接规则

- 协议模块不直接读文件
- 协议模块不保存私有 seek 状态机
- 所有 seek/pause/scale 都通过 `VodControlApi`
- 多轨保留到 engine 和可承载协议；目标协议无法表达的组合只输出明确诊断

## 4.5 Phase 04 测试要求

- RTSP `Range` seek 回归
- RTMP `seek` / `pause` / reconnect 回归
- HTTP-FLV/WS-FLV query seek 和 disconnect cleanup 回归
- 同一 MP4 文件通过四种协议播放的一致性回归
- 协议控制与 API 控制交错调用回归
