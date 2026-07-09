# Phase 03 — TS Module 播放、拉流与多轨道编排

- **状态**: 未开始
- **范围**: `cheetah-ts-module` 的 engine 接入、本地播放会话、远端 TS 拉流发布、周期 PAT/PMT、多轨道、按需 mux、配置校验
- **完成标准**: 一个本地发布流可同时被 HTTP-TS/WS-TS 多客户端播放；一个远端 HTTP(S)/WS(S)-TS 源可拉取、demux、发布到 engine 并再被其它协议消费

---

## 3.1 本地播放会话

**ZLMediaKit 参考**:

- `TSMediaSource::getRing()->attach()`
- `TSMediaSource::onWrite()` 将 TS packet 写入 ring
- `PacketCache<TSPacket>` 提供 GOP/cache 语义

**实现要求**:

- `PlayRequested` 后等待 engine stream，超时关闭连接
- 订阅时使用有界 `SubscriberOptions.queue_capacity`
- bootstrap 策略优先从最近关键帧开始
- muxer 初始化时使用 snapshot tracks
- 新连接先发送 PAT/PMT
- 每 `pat_pmt_interval_ms` 周期补发 PAT/PMT
- 视频关键帧到达时若距离上次 PAT/PMT 超过间隔，先补 PAT/PMT
- 连接关闭后取消对应 play session 并关闭 subscriber

**测试要求**:

- 无源等待超时关闭
- 有源时先收到 PAT/PMT 再收到 PES
- 关键帧前补 PAT/PMT
- 客户端断开后 subscriber close

---

## 3.2 按需 TS mux 策略

**ZLMediaKit 参考**:

- `TSMediaSourceMuxer::onReaderChanged()` 根据 `ts_demand` 开关 `_enabled`
- 无 reader 时清 cache，有 reader 时恢复 mux

**实现要求**:

- 新增配置 `demand_mode`，默认 `false`
- `demand_mode=false` 时按现有订阅播放模式即时 mux
- `demand_mode=true` 时只有存在 TS 播放者才执行 TS mux
- 最后一个播放者断开后清理 TS session 状态
- 新播放者到来后等待下一关键帧，避免从非关键帧启动

**测试要求**:

- demand 关闭时行为不变
- demand 开启且无播放者时不保留 TS packet cache
- demand 开启后新播放者从关键帧启动

---

## 3.3 远端 TS 拉流发布

**ZLMediaKit 参考**:

- `TsPlayerImp::onResponseBody()` 将 bytes 输入 decoder
- `TsPlayerImp::onPlayResult()` 创建 HLS/TS demuxer
- `TsPlayerImp::onShutdown()` flush decoder 后再 shutdown

**实现要求**:

- pull job acquire publisher lease
- pull transport 输出 bytes 后交给 `MpegTsDemuxer`
- `TrackFound` 需要累积所有当前 tracks 后调用 `sink.update_tracks()`
- 不允许每发现一轨就覆盖前面轨道
- `Frame` 输出时保留 track_id、codec、pts/dts、keyframe、discontinuity
- remote closed 时调用 demuxer.flush()
- 出错或取消时 release publisher lease
- retry backoff 在成功收到 body 后重置为初始值

**测试要求**:

- 单视频远端 TS 拉流发布
- 音视频双轨远端 TS 拉流发布
- 多音轨拉流发布
- 远端 EOF 时 flush 尾帧
- 第二发布者冲突时 pull job 失败并重试

---

## 3.4 多轨道模式

**ZLMediaKit 参考**:

- `MpegMuxer::_tracks` 用 frame index 映射多个 track
- PMT 中每个 track 独立 PID

**实现要求**:

- 播放输出按 `TrackInfo.track_id` 映射到 mux PID
- `max_tracks` 限制总轨道数
- 超出 `max_tracks` 的轨道记录 diagnostic，不进入 PMT
- 默认轨道排序：video tracks 在前，audio tracks 在后，同类按 `TrackId` 稳定排序
- PCR PID 选择第一个 video track，无 video 选择第一个 audio track
- 拉流发现新轨道时保持旧 track_id 不变，新轨追加

**测试要求**:

- 2 video + 2 audio PMT
- audio-only PCR
- 超出 max_tracks 截断
- 拉流轨道增量更新不覆盖旧轨

---

## 3.5 配置校验与运行时约束

**本地约束**:

AGENTS.md 要求 module 不直接使用 `tokio::spawn`，公共接口不暴露 `tokio::*`。

**实现要求**:

- module 中后台任务统一用 `ctx.runtime_api.spawn(Box::pin(...))`
- config validator 校验 listen 地址
- TLS enabled 时校验证书路径和 key 路径非空
- `write_queue_capacity >= 1`
- `subscriber_queue_capacity >= bootstrap_max_frames.max(1)`
- `max_tracks >= 1`
- `max_reassembly_bytes >= 188`
- `retry_backoff_ms <= max_retry_backoff_ms`
- pull URL scheme 必须是 http/https/ws/wss
- service registry 注册 `ts-http` / `ts-https` endpoint metadata

**测试要求**:

- 无效 listen 配置被拒绝
- TLS enabled 但证书为空被拒绝
- subscriber queue 小于 bootstrap 被拒绝
- pull URL scheme 错误被拒绝
- module start 后 service registry 可见 TS endpoint

---

## 验证命令

```bash
cargo fmt
cargo clippy -p cheetah-ts-module
cargo test -p cheetah-ts-module
cargo test -p cheetah-ts-driver-tokio
cargo test -p cheetah-codec ts_
```
