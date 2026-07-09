# Phase 03: Pull Client 与 Module 集成

- 状态：计划中
- 范围：新增 HTTP-FLV module 配置、生命周期、播放输出编排、远端 HTTP-FLV/WS-FLV pull job 和重试监督。
- 完成标准：`cheetah-server` 注册 HTTP-FLV module 后，可提供 HTTP/WS FLV 播放；配置 pull job 后，可从远端 HTTP/WS FLV 源拉入本地 stream。

## 目标目录

新增 crate：

```text
crates/protocols/http-flv/module/
  Cargo.toml
  src/lib.rs
  src/config.rs
  src/module.rs
  src/route.rs
  src/session.rs
  src/pull.rs
  tests/http_flv_play.rs
  tests/http_flv_pull.rs
  tests/support/mod.rs
```

应用层修改：

```text
apps/cheetah-server/Cargo.toml
apps/cheetah-server/src/main.rs
Cargo.toml
```

## 配置模型

新增 `HttpFlvModuleConfig`：

```rust
pub struct HttpFlvModuleConfig {
    pub enabled: bool,
    pub listen: String,
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub play_wait_source_timeout_ms: u64,
    pub subscriber_queue_capacity: usize,
    pub subscriber_backpressure: BackpressurePolicy,
    pub bootstrap_max_frames: usize,
    pub enable_add_mute: bool,
    pub emit_play_metadata: bool,
    pub alert_thresholds: HttpFlvAlertThresholds,
    pub pull_jobs: Vec<HttpFlvPullJobConfig>,
}
```

pull job：

```rust
pub struct HttpFlvPullJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_stream_key: String,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}
```

默认值：

- `enabled = true`
- `listen = "0.0.0.0:8080"`
- `write_queue_capacity = 256`
- `read_buffer_size = 64 * 1024`
- `play_wait_source_timeout_ms = 15_000`
- `subscriber_queue_capacity = 256`
- `bootstrap_max_frames = 150`
- `enable_add_mute = false`
- `emit_play_metadata = true`

## Module 行为

manifest：

```rust
ModuleManifest {
    module_id: ModuleId::new("http-flv"),
    display_name: "HTTP-FLV Module",
    dependencies: Vec::new(),
    config_namespace: "http_flv",
    routes_prefix: "/",
    capabilities: vec![ModuleCapability::Subscribe, ModuleCapability::Publish, ModuleCapability::BackgroundJob],
}
```

生命周期：

- `init` 解析配置并保存 `EngineContext`。
- `start` 在 `enabled` 时启动 driver server，注册 service endpoint `http-flv://{listen}`，并启动 enabled pull job supervisor。
- `stop` 取消 driver 和 pull job，等待所有 runtime loop 结束，注销 service。
- `apply_config` 对任何配置变化返回 `ModuleRestartRequired`。

播放输出：

- `PlayRequested` 后如果源不存在，按 `play_wait_source_timeout_ms` 等待；超时返回 404 或关闭 WS。
- 源存在后订阅，并发送 FLV bootstrap。
- 运行中 track 更新时，在 keyframe 或 readiness 变化时刷新 sequence header。
- source 关闭后关闭 HTTP/WS 连接。

## Pull Client 行为

HTTP pull：

- driver client 发起 GET。
- 接受 `200 OK`；非 2xx 返回错误并退避重试。
- 支持 `Transfer-Encoding: chunked` 和普通 body。
- body bytes 进入 FLV demux，完整 tag 交给共享 ingest adapter。

WS pull：

- driver client 发起 WebSocket handshake。
- 只接受 binary message；binary payload 当作连续 FLV byte stream 输入 demux。
- close frame、网络断开、协议错误触发 job 重试。

发布到 engine：

- job 启动后先 `PublisherApi::acquire_publisher(target_stream_key, PublisherOptions::default())`。
- 目标 stream 已有 active publisher 时，记录 warn 并停止该 job，不循环抢占。
- metadata/sequence header 更新 tracks 后调用 `sink.update_tracks`。
- media frame 调用 `sink.push_frame`；`DroppedByPolicy` 累计并按阈值告警。
- 退出时关闭 sink 并 release lease。

## 具体任务

### 3.1 新增 module 配置与生命周期

- [ ] 创建 `cheetah-http-flv-module` crate。
- [ ] 实现 `HttpFlvModuleConfig`、pull job config、alert thresholds、默认 JSON、validate。
- [ ] 实现 `HttpFlvModuleFactory` 和 `HttpFlvModule` 生命周期。
- [ ] 修改 workspace 与 `cheetah-server` feature，新增可选 `http-flv` feature。
- [ ] 增加配置校验测试：非法 listen、空 job name、非法 source URL、非法 target stream key、零 backoff、零 queue capacity。

### 3.2 实现 HTTP-FLV 播放输出 module 编排

- [ ] 处理 driver `PlayRequested` 事件，映射 `StreamKey` 与 play mode。
- [ ] 实现 pending play 等待源上线，超时 HTTP 返回 404，WS 关闭连接。
- [ ] 订阅 engine stream，发送 FLV header、metadata、sequence header、media tags。
- [ ] 复用 RTMP play 的 keyframe gate、timestamp rebase/clamp、mute AAC、source end close 规则。
- [ ] 增加端到端测试：RTMP publish H264/AAC 后，HTTP GET 能读到 FLV header、metadata、video/audio tag；WS 能收到 binary FLV bytes。

### 3.3 实现远端 HTTP/WS FLV pull job

- [ ] 在 driver 或 module pull 子模块实现 HTTP client GET 和 WS client handshake。
- [ ] 支持 chunked body、普通 body、WS binary message 进入同一 FLV demux。
- [ ] 使用共享 ingest adapter 更新 tracks 和生成 `AVFrame`。
- [ ] 实现 supervisor 重试退避、目标 stream 占用停止、取消退出和 lease release。
- [ ] 增加 pull job 集成测试：本地 synthetic HTTP-FLV server 输出标准 FLV，job 拉入 engine 后 stream snapshot tracks ready；再用 RTMP 或 HTTP-FLV 播放验证有 media frame。

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-http-flv-module --tests
cargo test -p cheetah-http-flv-module
cargo clippy -p cheetah-server --features http-flv
cargo test -p cheetah-server --features http-flv
```
