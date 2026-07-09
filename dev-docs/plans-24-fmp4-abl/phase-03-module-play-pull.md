# Phase 03 — Module 播放、拉流与关键帧起播

- **状态**: 规划中
- **范围**: 对齐 ABL 风格关键帧起播、bootstrap、pull job 和 track/config 变化行为。
- **完成标准**: 本地 stream 的 HTTP/WS-fMP4 播放更贴近真实客户端需求，远端 fMP4 拉流发布更稳。

## 3.1 关键帧起播

当前 module 已在有视频时等待关键帧。后续补成更接近 ABL 的语义：

- 新连接默认等待关键帧后再输出第一个 media fragment。
- 连接建立后可以优先消费 bootstrap/live tail 中最近关键帧开始的一段 GOP，而不是只盲等未来关键帧。
- bootstrap 不应跨连接共享可变 muxer 状态。

这对应 ABL 的 `ForceSendingIFrame` 思路，但实现方式仍基于 engine snapshot/subscriber，而不是复制其私有 GOP buffer。

## 3.2 播放 bootstrap

需要明确策略：

- `bootstrap_max_frames` 继续作为上限。
- 对有视频流，bootstrap 只从最近关键帧开始截取。
- 对 audio-only 流，可直接从 live tail 起播。
- bootstrap 里的旧 fragment 不跨 track/config 变化复用。

## 3.3 track/config 变化

保留现有重建 muxer 逻辑，并补 ABL 关注场景：

- H264/H265 参数集变化。
- AAC ASC 变化。
- 新 track 出现或旧 track 消失。
- 多 audio / 多 video 轨道变化。

规则：

- 先 flush 当前 fragment。
- 重建 muxer。
- 重发 init。
- 若仍有视频，则重新等待关键帧。

## 3.4 Pull supervisor

pull job 继续保持：

- 独占发布租约。
- demux 出 `TrackInfo` 后再 `publisher.update_tracks`。
- repeated init 更新 tracks。
- 对端断线、demux 连续异常或取消时释放 lease 并 backoff 重试。

需要补的地方：

- 对 repeated init 后的 `FrameFlags::DISCONTINUITY` 标记策略。
- 对远端 track 抖动的 diagnostic。
- 对 `target_stream_key` 和 job 名的更清晰日志。

## 3.5 ABL 风格实战兼容

module 侧需要覆盖的真实行为：

- H265 流起播稳定性。
- G711 直出或上游已转 AAC 时的轨道兼容。
- 实际帧率变化时 fragment 节奏不漂移。
- 新观看者快速出画，不必等待过长 GOP。

## 3.6 验收

```bash
cargo clippy -p cheetah-fmp4-module --tests
cargo test -p cheetah-fmp4-module
```

重点场景：

- 有视频时从关键帧起播。
- audio-only 不等待关键帧。
- bootstrap 从最近关键帧开始。
- track/config 变化后重发 init。
- repeated init 的 pull job 能更新 tracks 并继续推流。
