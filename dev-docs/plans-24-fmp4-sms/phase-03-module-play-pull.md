# Phase 03 — fMP4 Module 播放与拉流

- **状态**: 规划中
- **范围**: 新增 `cheetah-fmp4-module`，接入 engine，实现本地流 HTTP(S)/WS(S)-fMP4 播放和远端 fMP4 拉流发布
- **完成标准**: RTMP/RTSP 输入流可通过 HTTP-fMP4/WS-fMP4 播放；远端 HTTP/WS fMP4 源可拉流发布为本地 stream

---

## 3.1 Module Factory 与配置

新增 crate：

```text
crates/protocols/fmp4/module
```

module manifest：

- `module_id`: `fmp4`
- `display_name`: `fMP4 Module`
- `config_namespace`: `fmp4`
- `routes_prefix`: `/`
- capabilities: `Subscribe`、`Publish`、`BackgroundJob`

配置结构：

```rust
pub struct Fmp4ModuleConfig {
    pub enabled: bool,
    pub listen: String,
    pub tls: Fmp4TlsConfig,
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub play_wait_source_timeout_ms: u64,
    pub subscriber_queue_capacity: usize,
    pub subscriber_backpressure: BackpressurePolicy,
    pub bootstrap_max_frames: usize,
    pub max_tracks: usize,
    pub max_box_bytes: usize,
    pub max_fragment_duration_ms: u64,
    pub force_fragment_on_keyframe: bool,
    pub include_styp: bool,
    pub include_sidx: bool,
    pub demand_mode: bool,
    pub pull_jobs: Vec<Fmp4PullJobConfig>,
}
```

默认值：

- `listen = "0.0.0.0:8083"`
- TLS listen `0.0.0.0:8445`
- `subscriber_backpressure = DropUntilNextKeyframe`
- `bootstrap_max_frames = 150`
- `max_tracks = 32`
- `max_box_bytes = 4 MiB`
- `max_fragment_duration_ms = 1000`
- `force_fragment_on_keyframe = true`
- `include_styp = true`
- `include_sidx = true`

---

## 3.2 本地播放 Session

数据流：

```text
Fmp4DriverEvent::PlayRequested
  -> Fmp4Module::run_play_session
  -> wait_for_stream_snapshot
  -> SubscriberApi::subscribe
  -> Fmp4Muxer::init_segment / push_frame / flush
  -> Fmp4CoreCommandSender::send_fmp4_bytes
```

播放启动规则：

1. 等待 stream 出现，超时关闭连接
2. 从 snapshot tracks 初始化 muxer
3. 发送 init segment
4. 有视频时等待关键帧；audio-only 直接输出
5. 订阅 bootstrap 使用 live tail，容量不小于 `bootstrap_max_frames`
6. 关键帧或 `max_fragment_duration_ms` 到期 flush media segment
7. track list 或 codec config 变化时重建 muxer，并在下个关键帧发送新 init segment
8. unsupported frame 跳过并记录 bounded warn

慢客户端策略：

- driver write queue full 会关闭单连接
- subscriber queue 使用配置 backpressure
- fMP4 muxer 不持有跨连接共享大 buffer
- 每连接独立 muxer，避免一个客户端 fragment 状态污染其他客户端

---

## 3.3 远端 Pull Job

配置：

```rust
pub struct Fmp4PullJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_stream_key: String,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
    pub insecure_tls: bool,
}
```

拉流流程：

1. module start 时为每个 enabled job 启动 supervisor
2. supervisor 获取 target stream 独占 publisher lease
3. driver pull client 连接远端 fMP4 源
4. bytes 输入 `cheetah-codec::Fmp4Demuxer`
5. `TrackInfo` 后更新 publisher tracks
6. `Frame` 后写入 publisher
7. 连接关闭释放 lease，按 backoff 重试

错误策略：

- URL 非法：配置校验失败
- 目标 stream 已被占用：job 停止并记录错误
- 远端 4xx：默认重试，配置层可后续扩展为停止
- 远端 5xx/断线：退避重试
- demux 连续错误超过阈值：断开并重试
- 远端重复 init：更新 tracks 并标记 discontinuity

---

## 3.4 多轨道编排

播放方向：

- 所有支持的 audio/video track 写入同一 `moov`
- `TrackInfo.track_id` 到 MP4 track id 的映射由 muxer 固定保存
- track 超过 `max_tracks` 时跳过超限 track 并输出 diagnostic
- media fragment 可包含多个 `traf`，每个 `traf` 对应一个有样本的 track

拉流方向：

- demux track id 映射为新的 `TrackId`
- 多视频/多音频 track 全部进入 engine tracks
- 如果远端重复 init 或 track 变化，调用 `update_tracks`
- 不支持的 track 不阻塞其他 track

时间戳：

- source `tfdt/trun` timestamp 保存在 side data
- canonical timeline 由 `cheetah-codec` 统一展开和归一化
- discontinuity 时 frame 标记 `FrameFlags::DISCONTINUITY`

---

## 3.5 App 与 Workspace 接入

改动点：

- 根 `Cargo.toml` workspace members 加入 fMP4 core/driver/module/property-tests
- `apps/cheetah-server/Cargo.toml` 增加 feature `fmp4`
- `apps/cheetah-server/src/main.rs` 在 feature `fmp4` 下注册 `Fmp4ModuleFactory`
- `SystemArchitecture.md` 增加 fMP4 crate 映射和 CI/check baseline
- README / 示例配置增加 fMP4 module 配置示例

---

## 3.6 Module 测试

测试场景：

1. module 默认配置合法
2. TLS 配置启用但缺 cert/key 时拒绝
3. pull job URL/target stream key 校验
4. HTTP-fMP4 播放等待 stream，超时关闭
5. RTMP/RTSP 发布后 HTTP-fMP4 拉到 init + media segment
6. WebSocket 播放收到 binary init + media segment
7. 远端 HTTP chunked fMP4 拉流发布到 engine
8. 远端 WS binary fMP4 拉流发布到 engine
9. 多轨 `moov` 输出与 engine track 数一致
10. track change 后重发 init segment
11. unsupported codec 被跳过且不影响其他 track

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-fmp4-module --tests
cargo test -p cheetah-fmp4-module
cargo clippy -p cheetah-server --features fmp4
cargo test -p cheetah-server --features fmp4
```
