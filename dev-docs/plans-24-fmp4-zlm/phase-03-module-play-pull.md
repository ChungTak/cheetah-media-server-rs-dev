# Phase 03 — Module 播放、拉流与 ZLM demand mode

- **状态**: 规划中
- **范围**: 完善 `cheetah-fmp4-module` 的本地播放、远端拉流、多轨、track/config 变化、demand mode 和错误恢复。
- **完成标准**: 本地 RTMP/RTSP/其他 engine stream 可通过 HTTP(S)/WS(S)-fMP4 播放；远端 HTTP(S)/WS(S)-fMP4 可拉流发布为本地 stream。

## 3.1 播放 session

流程：

```text
Fmp4DriverEvent::PlayRequested
  -> wait_for_stream_snapshot
  -> SubscriberApi::subscribe
  -> Fmp4Muxer::init_segment
  -> Fmp4DriverCommand::SendData
  -> frame loop
  -> Fmp4Muxer::write_segment / flush
```

规则：

- 等待 stream 超时后关闭连接。
- 仅使用 audio/video track。
- track 数超过 `max_tracks` 时跳过超限 track 并记录 diagnostic。
- 发送 init segment 后再发送 media segment。
- 有视频时等待关键帧起播。
- audio-only 时所有 frame 可作为 fragment 起点。
- 关键帧、最大 fragment 时长、样本数量上限均可触发 flush。
- unsupported frame 跳过，不关闭整个 session。

## 3.2 track/config 变化

必须处理：

- 新 track 出现。
- track 消失。
- H264/H265 参数集变化。
- AAC ASC 变化。
- Opus/AV1/VP9 config 变化。

策略：

- 当前 fragment 先 flush。
- 重建 muxer。
- 发送新 init segment。
- 有视频时等待下一个关键帧恢复输出。
- 输出 diagnostic，包含 stream key、connection id、旧/新 track 列表摘要。

## 3.3 demand mode

ZLM 行为：

- `fmp4_demand=false` 时持续生成 fMP4。
- `fmp4_demand=true` 时无人观看可停止或延后生成。
- 无人观看后重新有观看者时清旧缓存。

Cheetah 实现：

- `demand_mode=false` 保持当前每连接即时 mux 行为。
- `demand_mode=true` 时仅有播放 session 时订阅 stream 和运行 muxer。
- 新观看者连接后从 live tail 订阅，丢弃旧 fragment，等待新关键帧发送 init + media。
- 不创建跨连接共享可变 muxer，避免连接之间状态污染。

## 3.4 pull job

流程：

```text
pull supervisor
  -> acquire_publisher
  -> connect_pull
  -> Fmp4Demuxer::push
  -> publisher.update_tracks
  -> publisher.push_frame
  -> close/release/retry
```

规则：

- URL scheme 只允许 `http/https/ws/wss`。
- target stream key 必须能解析为 namespace/path。
- publisher lease 获取失败时按 backoff 重试或停止，策略写入日志。
- `TrackInfo` 未发布前丢弃 frame。
- repeated init 更新 tracks。
- demux 连续错误超过阈值后断开并重试。
- 关闭或错误时必须释放 publisher lease。

## 3.5 多轨道

播放方向：

- 所有支持 track 写入同一个 `moov`。
- 每个 fragment 可包含多个 `traf`。
- 不支持 track 不阻塞其他 track。

拉流方向：

- 远端 track id 映射为 engine `TrackId`。
- 多 video/audio track 全部发布到 engine。
- track 变化调用 `publisher.update_tracks`。
- discontinuity 使用 `FrameFlags::DISCONTINUITY` 或等价 side data 标记。

## 3.6 配置

保留当前字段并补齐语义：

- `enabled`
- `listen`
- `tls`
- `write_queue_capacity`
- `read_buffer_size`
- `subscriber_queue_capacity`
- `bootstrap_max_frames`
- `play_wait_source_timeout_ms`
- `max_tracks`
- `max_box_bytes`
- `max_fragment_duration_ms`
- `force_fragment_on_keyframe`
- `include_styp`
- `include_sidx`
- `demand_mode`
- `pull_jobs`

所有影响 listener、TLS、queue、fragment、pull job、box 上限的变化返回 `ModuleRestartRequired`。

## 3.7 测试

```bash
cargo fmt
cargo clippy -p cheetah-fmp4-module --tests
cargo test -p cheetah-fmp4-module
cargo clippy -p cheetah-server --features fmp4
cargo test -p cheetah-server --features fmp4
```

场景：

- 默认配置合法。
- TLS 启用但缺 cert/key 时拒绝。
- pull job URL/target stream key 校验。
- 播放等待 stream 超时关闭。
- 有视频时等待关键帧。
- audio-only 可立即输出。
- track/config 变化重发 init。
- demand mode 重新观看等待新关键帧。
- pull job 断线释放 lease 并重试。
- 多轨输出 track 数与 snapshot 一致。

