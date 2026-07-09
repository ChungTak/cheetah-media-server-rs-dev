# Phase 03 — TS Module 播放与拉流

- **状态**: 规划中
- **范围**: 新增 `cheetah-ts-module`，接入 engine，实现本地流 HTTP(S)/WS(S)-TS 播放和远端 TS 拉流发布
- **完成标准**: RTMP/RTSP 输入流可通过 HTTP-TS/WS-TS 播放；远端 HTTP/WS TS 源可拉流发布为本地 stream

---

## 3.1 Module Factory 与配置

新增 crate：

```text
crates/protocols/ts/module
```

module manifest：

- `module_id`: `ts`
- `display_name`: `TS Module`
- `config_namespace`: `ts`
- `routes_prefix`: `/`
- capabilities: `Subscribe`、`Publish`、`BackgroundJob`

配置结构：

```rust
pub struct TsModuleConfig {
    pub enabled: bool,
    pub listen: String,
    pub tls: TsTlsConfig,
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub play_wait_source_timeout_ms: u64,
    pub subscriber_queue_capacity: usize,
    pub subscriber_backpressure: BackpressurePolicy,
    pub bootstrap_max_frames: usize,
    pub max_tracks: usize,
    pub strict_crc: bool,
    pub max_reassembly_bytes: usize,
    pub pat_pmt_interval_ms: u64,
    pub pull_jobs: Vec<TsPullJobConfig>,
}
```

默认值：

- `listen = "0.0.0.0:8082"`
- TLS listen `0.0.0.0:8444`
- `subscriber_backpressure = DropUntilNextKeyframe`
- `bootstrap_max_frames = 150`
- `max_tracks = 32`
- `strict_crc = false`
- `max_reassembly_bytes = 4 MiB`
- `pat_pmt_interval_ms = 500`

---

## 3.2 本地播放 Session

数据流：

```text
TsDriverEvent::PlayRequested
  -> TsModule::run_play_session
  -> wait_for_stream_snapshot
  -> SubscriberApi::subscribe
  -> MpegTsMuxer::push_frame
  -> TsCoreCommandSender::send_ts_bytes
```

播放启动规则：

1. 等待 stream 出现，超时关闭连接
2. 从 snapshot tracks 初始化 muxer
3. 发送 PAT/PMT
4. 有视频时等待关键帧；audio-only 直接输出
5. 订阅 bootstrap 使用 live tail，容量不小于 `bootstrap_max_frames`
6. 关键帧或 `pat_pmt_interval_ms` 到期补发 PAT/PMT
7. track list 变化时重建 muxer，并在下个关键帧恢复输出
8. unsupported frame 跳过并记录 bounded warn

慢客户端策略：

- driver write queue full 会关闭单连接
- subscriber queue 使用配置 backpressure
- TS muxer 不持有跨连接共享大 buffer

---

## 3.3 远端 Pull Job

配置：

```rust
pub struct TsPullJobConfig {
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
3. driver pull client 连接远端 TS 源
4. bytes 输入 `cheetah-codec::MpegTsDemuxer`
5. `TrackFound` 后更新 publisher tracks
6. `Frame` 后写入 publisher
7. 连接关闭释放 lease，按 backoff 重试

错误策略：

- URL 非法：配置校验失败
- 目标 stream 已被占用：job 停止并记录错误
- 远端 4xx：按配置决定停止；首版默认重试
- 远端 5xx/断线：退避重试
- demux 连续错误超过阈值：断开并重试

---

## 3.4 多轨道编排

播放方向：

- 所有支持的 audio/video track 写入同一 PMT
- PID 与 track_id 映射由 muxer 固定保存
- track 超过 `max_tracks` 时跳过超限 track 并输出 diagnostic

拉流方向：

- demux track PID 映射为新的 `TrackId`
- 首个 program 作为发布对象
- 多视频/多音频 track 全部进入 engine tracks
- 如果远端 PMT version change 导致 track 变化，调用 `update_tracks`

时间戳：

- source PTS/DTS 保存在 side data
- canonical timeline 由 `cheetah-codec` 统一展开和归一化
- discontinuity 时 frame 标记 `FrameFlags::DISCONTINUITY`

---

## 3.5 App 与 Workspace 接入

改动点：

- 根 `Cargo.toml` workspace members 加入 TS core/driver/module/property-tests
- `apps/cheetah-server/Cargo.toml` 增加 feature `ts`
- `apps/cheetah-server/src/main.rs` 在 feature `ts` 下注册 `TsModuleFactory`
- `SystemArchitecture.md` 增加 TS crate 映射和 CI/check baseline
- 相关 README / 示例配置增加 TS module 配置示例

---

## 3.6 Module 测试

测试场景：

1. module 默认配置合法
2. TLS 配置启用但缺 cert/key 时拒绝
3. pull job URL/target stream key 校验
4. HTTP-TS 播放等待 stream，超时关闭
5. RTMP/RTSP 发布后 HTTP-TS 拉到 TS bytes
6. WebSocket 播放收到 binary TS bytes
7. 远端 HTTP chunked TS 拉流发布到 engine
8. 远端 WS binary TS 拉流发布到 engine
9. 多轨 PMT 输出与 engine track 数一致
10. track change 后 PAT/PMT 更新

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-ts-module --tests
cargo test -p cheetah-ts-module
cargo clippy -p cheetah-server --features ts
cargo test -p cheetah-server --features ts
```
