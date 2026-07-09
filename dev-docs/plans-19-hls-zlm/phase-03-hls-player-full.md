# Phase 03 — HLS 播放器完整实现

- **状态**: 未开始
- **范围**: HTTP 客户端实现、自适应码率选择、redirect 跟随、playlist 变化检测、fMP4 demux、多轨道 demux、HLS→RTSP/RTMP/MP4 完整转发
- **完成标准**: 可从远端 HLS 源（TS 和 fMP4）拉流，自动选择码率，通过 RTMP/RTSP 播放

---

## 3.1 HTTP 客户端实现

**ZLMediaKit 参考**: `HttpClientImp` 继承 `TcpClient`，支持 HTTP/1.1 持久连接、chunked transfer、redirect。

**实现方案**:

在 driver 层新增 HTTP 客户端：

```rust
// cheetah-hls-driver-tokio/src/http_client.rs

pub struct HttpClient {
    stream: Option<TcpStream>,
    host: String,
    port: u16,
    /// Reuse connection for same host.
    keep_alive: bool,
}

pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Bytes,
}

impl HttpClient {
    pub async fn get(&mut self, url: &str) -> Result<HttpResponse, HttpClientError>;
    pub async fn get_streaming(
        &mut self,
        url: &str,
        on_chunk: impl FnMut(&[u8]),
    ) -> Result<HttpResponse, HttpClientError>;
}

pub enum HttpClientError {
    Connect(io::Error),
    Timeout,
    InvalidResponse,
    TooManyRedirects,
    Status(u16),
}
```

**特性**:
- HTTP/1.1 持久连接（同 host 复用 TCP）
- Chunked transfer-encoding 解码
- Content-Length 模式
- 超时控制（connect + read）
- 最大响应体限制（防 OOM）

---

## 3.2 HTTP Redirect 跟随

**ZLMediaKit 参考**: `HlsPlayer::onRedirectUrl()` 处理 301/302/307/308。

**实现方案**:

```rust
impl HttpClient {
    const MAX_REDIRECTS: u8 = 5;

    async fn get_with_redirect(&mut self, url: &str) -> Result<HttpResponse, HttpClientError> {
        let mut current_url = url.to_string();
        for _ in 0..Self::MAX_REDIRECTS {
            let resp = self.get_raw(&current_url).await?;
            match resp.status {
                301 | 302 | 307 | 308 => {
                    let location = resp.headers.get("Location")
                        .ok_or(HttpClientError::InvalidResponse)?;
                    current_url = resolve_url(&current_url, location);
                }
                _ => return Ok(resp),
            }
        }
        Err(HttpClientError::TooManyRedirects)
    }
}
```

---

## 3.3 自适应码率选择

**ZLMediaKit 参考**: `HlsPlayer::onParsed()` 解析 master playlist 后选择 variant。

**实现方案**:

```rust
// cheetah-hls-core/src/player.rs (新增)

pub struct HlsPlayerState {
    /// Parsed master playlist variants (sorted by bandwidth).
    variants: Vec<ParsedVariant>,
    /// Currently selected variant index.
    selected_variant: usize,
    /// Bandwidth estimation (bytes/sec).
    estimated_bandwidth: u64,
}

pub enum BandwidthStrategy {
    /// Always select highest bandwidth variant.
    Highest,
    /// Always select lowest bandwidth variant.
    Lowest,
    /// Auto-select based on measured download speed.
    Auto { safety_factor: f64 },
}

impl HlsPlayerState {
    pub fn select_variant(&mut self, strategy: &BandwidthStrategy) -> &ParsedVariant {
        match strategy {
            BandwidthStrategy::Highest => self.variants.last().unwrap(),
            BandwidthStrategy::Lowest => self.variants.first().unwrap(),
            BandwidthStrategy::Auto { safety_factor } => {
                // Select highest variant whose bandwidth < estimated * safety_factor
                let threshold = (self.estimated_bandwidth as f64 * safety_factor) as u64;
                self.variants.iter().rev()
                    .find(|v| v.bandwidth <= threshold)
                    .unwrap_or(&self.variants[0])
            }
        }
    }

    pub fn update_bandwidth(&mut self, bytes: u64, duration_ms: u64) {
        let bps = bytes * 1000 / duration_ms.max(1);
        // EWMA smoothing
        self.estimated_bandwidth = (self.estimated_bandwidth * 7 + bps) / 8;
    }
}
```

**配置**:
```yaml
modules:
  hls:
    pull_jobs:
      - name: remote
        bandwidth_strategy: "auto"  # "highest" | "lowest" | "auto"
```

---

## 3.4 Playlist 变化检测与刷新策略

**ZLMediaKit 参考**: `HlsPlayer` 跟踪 `_last_sequence`，playlist 未变化时增加 `_timeout_multiple`。

**实现方案**:

```rust
// cheetah-hls-core/src/player.rs

pub struct PlaylistRefreshState {
    last_sequence: i64,
    target_duration_ms: u64,
    /// Multiplier for reload interval when playlist unchanged.
    timeout_multiple: u8,
    /// Consecutive unchanged reloads.
    unchanged_count: u8,
}

impl PlaylistRefreshState {
    pub fn on_playlist_loaded(&mut self, new_sequence: i64) -> RefreshAction {
        if new_sequence > self.last_sequence {
            self.last_sequence = new_sequence;
            self.timeout_multiple = MIN_TIMEOUT_MULTIPLE;
            self.unchanged_count = 0;
            RefreshAction::FetchNewSegments
        } else {
            self.unchanged_count += 1;
            self.timeout_multiple = (self.timeout_multiple + 1).min(MAX_TIMEOUT_MULTIPLE);
            RefreshAction::WaitAndRetry {
                delay_ms: self.target_duration_ms / self.timeout_multiple as u64,
            }
        }
    }
}

const MIN_TIMEOUT_MULTIPLE: u8 = 2;
const MAX_TIMEOUT_MULTIPLE: u8 = 5;
```

**重试策略** (对齐 ZLMediaKit):
- m3u8 拉取失败: 最多 `MAX_TRY_FETCH_INDEX_TIMES=5` 次
- segment 下载失败: 最多 `MAX_TS_DOWNLOAD_FAILED_COUNT=10` 次连续失败后断开
- 超时: `timeout_multiple * target_duration`

---

## 3.5 fMP4 Demux (moof/mdat 解析)

**实现方案**:

在 `cheetah-hls-core` 新增 `fmp4_demux.rs`：

```rust
// cheetah-hls-core/src/fmp4_demux.rs

pub struct Fmp4Demuxer {
    /// Track info from init segment (moov).
    tracks: Vec<Fmp4DemuxTrack>,
    init_parsed: bool,
}

pub struct Fmp4DemuxTrack {
    pub track_id: u32,
    pub codec: CodecId,
    pub media_kind: MediaKind,
    pub timescale: u32,
    pub extradata: Bytes,
}

pub enum Fmp4DemuxEvent {
    TrackInfo(Vec<Fmp4DemuxTrack>),
    Frame {
        track_id: u32,
        media_kind: MediaKind,
        pts_ms: u64,
        dts_ms: u64,
        keyframe: bool,
        data: Bytes,
    },
}

impl Fmp4Demuxer {
    /// Parse init segment (ftyp + moov) to extract track info.
    pub fn parse_init(&mut self, data: &[u8]) -> Result<Vec<Fmp4DemuxEvent>, Fmp4DemuxError>;

    /// Parse media segment (moof + mdat) to extract frames.
    pub fn parse_segment(&mut self, data: &[u8]) -> Result<Vec<Fmp4DemuxEvent>, Fmp4DemuxError>;
}
```

**解析流程**:
1. Init segment: 遍历 moov → trak[] → stsd 提取 codec + extradata
2. Media segment: 解析 moof → traf[] → trun 获取 sample 元数据
3. 根据 trun 的 data_offset + sample_size 从 mdat 中切割 sample data
4. 将 timescale 时间戳转换为毫秒

---

## 3.6 多轨道 TS Demux 增强

**ZLMediaKit 参考**: `TSDemuxer` 支持多 video/audio PID。

**实现方案**:

扩展现有 `TsDemuxer`：

```rust
// 扩展 cheetah-hls-core/src/ts_demux.rs

pub struct TsDemuxer {
    /// All discovered tracks (multiple video/audio possible).
    tracks: Vec<TsDemuxTrack>,
    /// PES reassembly buffers per PID.
    pes_buffers: HashMap<u16, PesBuffer>,
}

struct TsDemuxTrack {
    pid: u16,
    stream_type: u8,
    codec: CodecId,
    media_kind: MediaKind,
}
```

**改动**: 从固定 `video_pid/audio_pid` 改为动态 `tracks: Vec<TsDemuxTrack>`，支持多音频轨（如多语言）。

---

## 3.7 HLS→RTSP/RTMP/MP4 完整转发

**ZLMediaKit 参考**: `HlsPlayerImp` 将 demux 后的帧通过 `MediaSource` 发布，其他协议自动订阅。

**实现方案**:

```rust
// cheetah-hls-module/src/pull.rs — 完整拉流循环

pub async fn run_hls_pull_job(
    config: &HlsPullJobConfig,
    engine: &EngineContext,
    http_client: &mut HttpClient,
) {
    // 1. Fetch master/media playlist
    // 2. Select variant (if master)
    // 3. Parse media playlist → get segment list
    // 4. Download new segments
    // 5. Demux (TS or fMP4 based on playlist content)
    // 6. Publish frames to engine via publisher_api
    // 7. Wait and refresh playlist
    // 8. Repeat until cancelled

    let publisher = engine.publisher_api().create_publisher(
        &config.target_stream_key,
        PublisherOptions::default(),
    ).await?;

    loop {
        let playlist = fetch_and_parse_playlist(http_client, &media_url).await?;
        let new_segments = filter_new_segments(&playlist, &seen_uris);

        for seg_info in new_segments {
            let data = http_client.get(&seg_info.url).await?.body;
            let events = demuxer.feed_segment(&data);
            for event in events {
                match event {
                    DemuxEvent::TrackInfo(tracks) => {
                        publisher.update_tracks(tracks.into_iter().map(to_track_info).collect());
                    }
                    DemuxEvent::Frame { pts_ms, dts_ms, keyframe, data, media_kind, .. } => {
                        let frame = AVFrame { pts: pts_ms, dts: dts_ms, payload: data, .. };
                        publisher.push_frame(Arc::new(frame));
                    }
                }
            }
            // Update bandwidth estimation
            player_state.update_bandwidth(data.len() as u64, download_duration_ms);
        }

        // Wait before next refresh
        let delay = refresh_state.compute_delay();
        sleep(delay).await;
    }
}
```

**协议互转路径**:
- HLS → Engine (publish) → RTMP subscriber → RTMP 播放
- HLS → Engine (publish) → RTSP subscriber → RTSP/RTSPS 播放
- HLS → Engine (publish) → MP4 recorder → MP4 文件

---

## 3.8 fMP4 vs TS 自动检测

**实现方案**:

```rust
fn detect_container(playlist: &ParsedMediaPlaylist) -> HlsContainer {
    if playlist.map_uri.is_some() {
        // Has #EXT-X-MAP → fMP4
        HlsContainer::Fmp4
    } else {
        // Default to TS
        HlsContainer::MpegTs
    }
}
```

播放器根据 playlist 中是否有 `#EXT-X-MAP` 自动选择 TS demuxer 或 fMP4 demuxer。

---

## 验证方法

1. 从外部 TS HLS 源拉流 → `ffplay rtmp://localhost/live/hls_pull` 验证
2. 从外部 fMP4 HLS 源拉流 → 验证 init segment 解析 + frame 输出
3. Master playlist 多码率 → 验证自动选择最高可用码率
4. 302 redirect → 验证跟随到最终 URL
5. Playlist 未变化 → 验证退避刷新间隔增长
6. 多音频轨 TS → 验证所有轨道被 demux
7. 长时间运行 24h → 无内存泄漏，无 segment 遗漏
