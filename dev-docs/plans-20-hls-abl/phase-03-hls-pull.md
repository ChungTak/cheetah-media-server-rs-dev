# Phase 03 — HLS 代理拉流完整实现

- **状态**: 未开始
- **目标**: 对标 ABLMediaServer `CNetClientRecvHttpHLS` 实现完整的 HLS 代理拉流功能：HTTP 客户端、m3u8 增量解析、TS/fMP4 segment 下载与 demux、流控与重试、发布到引擎
- **影响 crate**: `cheetah-hls-core`（协议逻辑）、`cheetah-hls-driver-tokio`（HTTP 客户端）、`cheetah-hls-module`（任务编排）
- **参考源**: `NetClientRecvHttpHLS.cpp`

---

## 1. ABL 实现分析

### 架构

```
[Remote HLS Server] → HTTP GET → [CNetClientRecvHttpHLS]
                                        ↓
                              m3u8 解析 → TS 文件列表
                                        ↓
                              HTTP GET → TS 文件下载
                                        ↓
                              ts_demuxer → H264/H265 + AAC
                                        ↓
                              PushVideo/PushAudio → MediaStreamSource
```

### 关键流控参数

| 参数 | 值 | 说明 |
|------|-----|------|
| m3u8 请求间隔 | ≥ 2 秒 | 避免过于频繁请求 |
| TS 文件超时 | 6 秒 | 超时则跳过当前 TS，重新请求 m3u8 |
| 视频回调节流 | 20ms | 避免瞬间推送大量帧 |
| 音频批量发送 | 3 帧/批 | 减少调度开销 |
| 历史列表 TTL | 30 秒 | 去重窗口 |
| 请求状态机 | NoRequest → SendRequest → RecvHttpHead → RequestSuccess | 顺序请求 |

### 去重机制

维护已下载 TS 文件的历史列表（文件名 + 时间戳），新 m3u8 中的 segment 与历史列表比对：
- 已存在 → 跳过
- 不存在 → 加入请求队列
- 历史列表中超过 30 秒的条目自动清除

---

## 2. 实现方案

### 2.1 Core 层：HLS Pull 协议状态机

```rust
// core/pull.rs (新增/重写)

pub struct HlsPullState {
    /// 已知的最新 media sequence
    last_media_sequence: u64,
    /// 已下载 segment 的去重集合（name → download_time）
    downloaded_segments: HashMap<String, Instant>,
    /// 待下载队列
    pending_segments: VecDeque<SegmentInfo>,
    /// 当前状态
    state: PullState,
    /// 流控计时
    last_playlist_request: Option<Instant>,
}

pub enum PullState {
    /// 等待下一次 m3u8 请求
    WaitingForPlaylist,
    /// 正在下载 segment
    DownloadingSegment { name: String, started_at: Instant },
    /// 错误重试等待
    RetryWait { until: Instant },
}

pub enum PullInput {
    /// 收到 m3u8 响应
    PlaylistReceived(String),
    /// 收到 segment 数据
    SegmentReceived { name: String, data: Bytes },
    /// 请求超时
    Timeout,
    /// 时钟 tick
    Tick(Instant),
}

pub enum PullOutput {
    /// 请求 m3u8
    FetchPlaylist { url: String },
    /// 请求 segment
    FetchSegment { url: String, name: String },
    /// demux 后的媒体帧
    MediaFrame(AVFrame),
    /// 错误
    Error(PullError),
}
```

**去重逻辑：**
```rust
fn process_playlist(&mut self, content: &str, now: Instant) -> Vec<PullOutput> {
    // 清理过期历史（30秒）
    self.downloaded_segments.retain(|_, t| now.duration_since(*t) < Duration::from_secs(30));
    
    // 解析 m3u8
    let playlist = parse_media_playlist(content);
    
    // 过滤已下载的 segment
    for seg in playlist.segments {
        if !self.downloaded_segments.contains_key(&seg.uri) {
            self.pending_segments.push_back(seg);
        }
    }
    
    // 更新 media sequence
    self.last_media_sequence = playlist.media_sequence;
    
    // 输出下一个待下载 segment
    self.next_action(now)
}
```

### 2.2 Driver 层：HTTP 客户端

```rust
// driver/http_client.rs (新增)

pub struct HlsHttpClient {
    client: reqwest::Client,  // 或自实现轻量 HTTP/1.1 客户端
    base_url: String,
    timeout: Duration,
}

impl HlsHttpClient {
    pub async fn fetch_playlist(&self, url: &str) -> Result<String, HlsPullError>;
    pub async fn fetch_segment(&self, url: &str) -> Result<Bytes, HlsPullError>;
}
```

**注意**：为避免引入 `reqwest` 重依赖，可自实现轻量 HTTP/1.1 客户端：
- TCP 连接 + 手动构造 GET 请求
- 解析 `Content-Length` 或 `Transfer-Encoding: chunked`
- 支持 HTTP 重定向（301/302）
- 连接复用（Keep-Alive）

### 2.3 Module 层：Pull Job 编排

```rust
// module/pull.rs (重写)

pub struct HlsPullJob {
    config: HlsPullJobConfig,
    state: HlsPullState,
    http_client: HlsHttpClient,
    ts_demuxer: TsDemuxer,      // 或 Fmp4Demuxer
    fmp4_demuxer: Fmp4Demuxer,
    publish_handle: Option<PublishHandle>,
    retry_count: u32,
}

impl HlsPullJob {
    pub async fn run(&mut self, engine: &EngineContext) {
        loop {
            match self.state.state {
                PullState::WaitingForPlaylist => {
                    // 等待间隔
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    // 请求 m3u8
                    match self.http_client.fetch_playlist(&self.config.source_url).await {
                        Ok(content) => self.process_playlist(content).await,
                        Err(e) => self.handle_error(e).await,
                    }
                }
                PullState::DownloadingSegment { .. } => {
                    self.download_next_segment().await;
                }
                PullState::RetryWait { until } => {
                    tokio::time::sleep_until(until.into()).await;
                    self.state.state = PullState::WaitingForPlaylist;
                }
            }
        }
    }
}
```

### 2.4 TS/fMP4 Demux 与发布

下载的 segment 数据通过已有的 `TsDemuxer` 或 `Fmp4Demuxer` 解析：

```rust
async fn process_segment(&mut self, name: &str, data: Bytes) {
    if name.ends_with(".ts") {
        let frames = self.ts_demuxer.demux(&data);
        for frame in frames {
            self.publish_frame(frame).await;
        }
    } else if name.ends_with(".m4s") || name.ends_with(".mp4") {
        let frames = self.fmp4_demuxer.demux(&data);
        for frame in frames {
            self.publish_frame(frame).await;
        }
    }
}

async fn publish_frame(&mut self, frame: AVFrame) {
    if let Some(handle) = &self.publish_handle {
        handle.publish(frame).await;
    }
}
```

### 2.5 视频帧速度检测

参考 ABL 的帧速度计算（从 PTS 差值推算）：

```rust
fn detect_frame_rate(&mut self, pts: u64, timebase: u32) -> Option<u32> {
    if let Some(last_pts) = self.last_video_pts {
        let diff = pts.saturating_sub(last_pts);
        if diff > 0 {
            let fps = timebase / diff as u32;
            self.fps_samples.push(fps);
            if self.fps_samples.len() >= 25 {
                let avg = self.fps_samples.iter().sum::<u32>() / self.fps_samples.len() as u32;
                return Some(avg);
            }
        }
    }
    self.last_video_pts = Some(pts);
    None
}
```

---

## 3. 配置

```yaml
hls:
  pull_jobs:
    - name: "remote_hls"
      enabled: true
      source_url: "http://remote-server/live/stream.m3u8"
      target_stream_key: "live/remote_stream"
      playlist_interval_ms: 2000       # m3u8 请求间隔
      segment_timeout_ms: 6000         # segment 下载超时
      retry_backoff_ms: 1000           # 重试退避
      max_retry_backoff_ms: 30000      # 最大退避
      dedup_window_secs: 30            # 去重窗口
```

---

## 4. 错误处理与重试

| 错误类型 | 处理策略 |
|----------|----------|
| m3u8 请求失败 (网络错误) | 指数退避重试 |
| m3u8 返回 404 | 等待 `playlist_interval_ms` 后重试 |
| segment 下载超时 | 跳过当前 segment，重新请求 m3u8 |
| segment demux 失败 | 记录错误，继续下一个 segment |
| 连接被拒绝 | 指数退避重试 |
| 流已存在 (publish 冲突) | 报错退出 |

---

## 验收标准

- [ ] 从远程 HLS 服务器拉取流并发布到本地引擎
- [ ] m3u8 增量解析，不重复下载已有 segment
- [ ] TS segment 正确 demux 为 H264 + AAC
- [ ] fMP4 segment 正确 demux
- [ ] 网络中断后自动重连重试
- [ ] 帧速度正确检测并设置到 TrackInfo
- [ ] 拉取的流可被其他协议（RTMP/RTSP/HTTP-FLV）订阅

---

## 测试计划

```bash
# 单元测试
cargo test -p cheetah-hls-core  # pull state machine
cargo test -p cheetah-hls-driver-tokio  # HTTP client

# 集成测试
cargo test -p cheetah-hls-module  # pull job

# 端到端测试
# 1. 启动一个 HLS 源（ffmpeg 推流 + 本地 HLS 输出）
# 2. 配置 pull job 拉取该源
# 3. 用 ffplay 播放拉取后的流
```
