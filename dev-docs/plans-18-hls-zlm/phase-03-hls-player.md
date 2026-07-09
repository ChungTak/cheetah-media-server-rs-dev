# Phase 03 — HLS 播放器/拉流器

- **状态**: 未开始
- **范围**: HLS 拉流、TS demux、实时 pacing、HLS→RTSP/RTMP/MP4 转发
- **完成标准**: 可从远端 HLS 源拉流并通过 RTMP/RTSP 播放

---

## 3.1 HLS 播放器框架

**ZLMediaKit 参考**: `HlsPlayer` 继承 `HttpClientImp`，循环拉取 m3u8 + 下载 segment。

**实现方案**:

```rust
// cheetah-hls-module/src/pull.rs (扩展现有框架)
pub struct HlsPullSession {
    source_url: String,
    target_stream_key: StreamKey,
    /// 已下载的 segment URI 集合（去重）
    seen_segments: HashSet<String>,
    /// 当前 media sequence
    last_sequence: u64,
    /// M3U8 刷新间隔
    refresh_interval_ms: u64,
}
```

**拉取循环**:
1. GET source_url → 判断 master/media playlist
2. 若 master → 选最高 bandwidth variant → GET media playlist URL
3. 解析 media playlist → 找出新 segment（URI 去重，忽略 query 参数）
4. 顺序下载新 segment → 送入 TS demuxer
5. 等待 `target_duration / 2` 后重新拉取 m3u8
6. 若 playlist 未变化 → 等待 `target_duration` 后重试

**重试策略** (参考 ZLMediaKit):
- m3u8 拉取失败: 最多 5 次重试，指数退避
- segment 下载失败: 最多 10 次连续失败后断开
- 超时: `2x ~ 5x segment_duration`，自适应

**HTTP 客户端**: 使用 `RuntimeApi::connect_tcp` + 手动 HTTP/1.1 请求（与 driver 层 HTTP 解析复用）

---

## 3.2 TS Demux（MPEG-TS 解封装）

**ZLMediaKit 参考**: `DecoderImp` 使用 libmpeg 的 `ts_demuxer_t`。

**实现方案**:

在 `cheetah-hls-core` 新增 `ts_demux.rs`：

```rust
pub struct TsDemuxer {
    /// PAT/PMT 解析状态
    pmt_parsed: bool,
    video_pid: Option<u16>,
    audio_pid: Option<u16>,
    video_codec: CodecId,
    audio_codec: CodecId,
    /// PES 重组缓冲
    video_pes_buf: Vec<u8>,
    audio_pes_buf: Vec<u8>,
}

pub enum TsDemuxEvent {
    TrackInfo { codec: CodecId, media_kind: MediaKind },
    Frame { media_kind: MediaKind, pts: u64, dts: u64, keyframe: bool, data: Bytes },
}

impl TsDemuxer {
    pub fn feed(&mut self, ts_packet: &[u8]) -> Vec<TsDemuxEvent>;
    pub fn feed_segment(&mut self, segment_data: &[u8]) -> Vec<TsDemuxEvent>;
}
```

**解析流程**:
1. 每 188 字节为一个 TS packet
2. PID=0x0000 → PAT → 提取 PMT PID
3. PMT PID → 解析 stream_type → 确定 video/audio PID 和 codec
4. Video/Audio PID → 收集 PES 数据 → 解析 PTS/DTS → 输出 Frame 事件

---

## 3.3 实时 Pacing（HlsDemuxer）

**ZLMediaKit 参考**: `HlsDemuxer` 50ms 定时器，按 DTS 顺序消费帧，缓冲管理。

**实现方案**:

```rust
pub struct HlsPlaybackPacer {
    buffer: VecDeque<PacedFrame>,
    play_start_time: u64,    // micros
    first_frame_dts: u64,    // ms
    max_buffer_ms: u64,      // 30000
    min_buffer_ms: u64,      // 3000
}

impl HlsPlaybackPacer {
    /// Add a demuxed frame to the buffer.
    pub fn push(&mut self, frame: AVFrame);

    /// Consume frames that should be played by now.
    /// Call every ~50ms.
    pub fn drain_ready(&mut self, now_micros: u64) -> Vec<AVFrame>;
}
```

**缓冲策略** (参考 ZLMediaKit):
- 缓冲 > 30s → 强制消费到 15s
- 缓冲 < 3s → 降速播放（拉伸时间）
- 正常: 按 DTS 实时消费

---

## 3.4 HLS→RTSP/RTMP/MP4 转发

**实现方案**:

拉取的帧通过 `EngineContext::publisher_api` 发布到引擎：

```rust
// pull.rs — 在 segment 下载 + demux 后
for event in demuxer.feed_segment(&segment_data) {
    match event {
        TsDemuxEvent::TrackInfo { codec, media_kind } => {
            publisher.update_tracks(vec![TrackInfo { codec, media_kind, .. }]);
        }
        TsDemuxEvent::Frame { pts, dts, keyframe, data, media_kind } => {
            let frame = AVFrame { pts, dts, payload: data, media_kind, .. };
            publisher.push_frame(Arc::new(frame));
        }
    }
}
```

发布后，其他协议模块（RTMP/RTSP）可自动订阅该流。

---

## 3.5 支持 Amazon Echo Show 等设备

**需求**: Echo Show 通过 RTSP[S] 播放，需要 HLS→RTSP 转换。

**实现**: 当 HLS pull job 将流发布到引擎后，RTSP module 自动可以 PLAY 该流。只需确保：
- Track info 正确（codec、extradata）
- 时间戳连续（pacer 保证）
- RTSPS 已支持（现有 RTSP module 已有 TLS）

无需额外代码，仅需配置验证。

---

## 验证方法

1. 从外部 HLS 源拉流 → 通过 RTMP 播放验证
2. ffplay rtsp://localhost/live/hls_pull → 验证 HLS→RTSP 转换
3. 缓冲测试: 模拟网络抖动 → 验证 pacer 平滑输出
4. 长时间运行: 24h 拉流无内存泄漏
