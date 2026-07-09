# Phase 02 — Demuxed Playlist 与 URL 路由

- **目标**: 为 demuxed audio/video 生成独立 per-track chunklist，master playlist 声明 audio rendition，core/module 支持 per-track URL、blocking reload、preload part、cache 和 stream key validation
- **OME 参考**: `llhls_master_playlist.cpp::MakePlaylist`、`llhls_chunklist.cpp::MakeChunklist`、`llhls_session.cpp::ParseFileName`

---

## 1. Master Playlist

### 目标输出（video + audio）

```m3u8
#EXTM3U
#EXT-X-VERSION:9
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="audio",NAME="default",DEFAULT=YES,AUTOSELECT=YES,CHANNELS="2",URI="chunklist_audio.m3u8?uid=1"
#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1920x1080,FRAME-RATE=30.000,CODECS="avc1.64001f,mp4a.40.2",AUDIO="audio"
chunklist_video.m3u8?uid=1
```

要求：

- 有 audio lane 时必须输出 `#EXT-X-MEDIA:TYPE=AUDIO`。
- `#EXT-X-STREAM-INF` 必须通过 `AUDIO="audio"` 关联 audio group。
- `CODECS` 必须同时包含 video/audio codec string。
- 有 video metadata 时输出 `RESOLUTION` 和 `FRAME-RATE`。
- 有 audio channels 时输出 `CHANNELS`，缺省按 `"2"`。
- video-only 流不得输出空 audio rendition。
- audio-only 流生成合法 audio-only variant，URI 指向 `chunklist_audio.m3u8`。

### Core 接口

文件：`crates/protocols/hls/core/src/playlist.rs`

新增最小 builder：

```rust
pub struct DemuxedMasterPlaylist {
    pub video: Option<MediaRenditionInfo>,
    pub audio: Option<MediaRenditionInfo>,
    pub session_id: Option<u64>,
    pub include_stream_key: bool,
}

pub struct MediaRenditionInfo {
    pub lane: TrackLane,
    pub uri: String,
    pub codecs: String,
    pub bandwidth: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<f64>,
    pub channels: Option<u8>,
}
```

`TrackLane` 放在 core 层可复用模块中，供 request parser、playlist builder、module 共享。

---

## 2. Per-Track Chunklist

### Video Chunklist

```m3u8
#EXTM3U
#EXT-X-VERSION:9
#EXT-X-TARGETDURATION:4
#EXT-X-MEDIA-SEQUENCE:0
#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=0.6
#EXT-X-PART-INF:PART-TARGET=0.200
#EXT-X-MAP:URI="init_video.mp4?uid=1"
#EXT-X-PROGRAM-DATE-TIME:2026-05-19T16:00:00.000Z
#EXT-X-PART:DURATION=0.200,URI="video_part_0.m4s?uid=1",INDEPENDENT=YES
#EXT-X-PART:DURATION=0.200,URI="video_part_1.m4s?uid=1"
#EXTINF:4.000,
video_seg_0.m4s?uid=1
#EXT-X-PRELOAD-HINT:TYPE=PART,URI="video_part_12.m4s?uid=1"
#EXT-X-RENDITION-REPORT:URI="chunklist_audio.m3u8?uid=1",LAST-MSN=0,LAST-PART=8
```

### Audio Chunklist

```m3u8
#EXTM3U
#EXT-X-VERSION:9
#EXT-X-TARGETDURATION:4
#EXT-X-MEDIA-SEQUENCE:0
#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=0.6
#EXT-X-PART-INF:PART-TARGET=0.209
#EXT-X-MAP:URI="init_audio.mp4?uid=1"
#EXT-X-PROGRAM-DATE-TIME:2026-05-19T16:00:00.000Z
#EXT-X-PART:DURATION=0.209,URI="audio_part_0.m4s?uid=1",INDEPENDENT=YES
#EXTINF:4.000,
audio_seg_0.m4s?uid=1
#EXT-X-PRELOAD-HINT:TYPE=PART,URI="audio_part_19.m4s?uid=1"
#EXT-X-RENDITION-REPORT:URI="chunklist_video.m3u8?uid=1",LAST-MSN=0,LAST-PART=12
```

要求：

- 每个 chunklist 的 `EXT-X-MAP` 指向自己的 init segment。
- 每个 lane 的 part/segment 序号独立。
- Audio parts 对 AAC/Opus 可标记 `INDEPENDENT=YES`；video 只有 part 第一帧是 keyframe 时标记。
- `RENDITION-REPORT` 报告其他 lane 当前最新 `LAST-MSN` / `LAST-PART`，不要报告自己。
- Stream conclude 后不输出 `PRELOAD-HINT`，两个 lane 都追加 `EXT-X-ENDLIST`。

---

## 3. URL 路由

### v1 URL 模式

| URL | 含义 |
|-----|------|
| `/{ns}/{stream}.m3u8` | Master playlist |
| `/{ns}/{stream}/chunklist_video.m3u8` | Video media playlist |
| `/{ns}/{stream}/chunklist_audio.m3u8` | Audio media playlist |
| `/{ns}/{stream}/init_video.mp4` | Video init segment |
| `/{ns}/{stream}/init_audio.mp4` | Audio init segment |
| `/{ns}/{stream}/video_part_N.m4s` | Video part |
| `/{ns}/{stream}/audio_part_N.m4s` | Audio part |
| `/{ns}/{stream}/video_seg_N.m4s` | Video segment |
| `/{ns}/{stream}/audio_seg_N.m4s` | Audio segment |

兼容 URL：

- `/{ns}/{stream}/index.m3u8` 返回 video chunklist，或在 legacy/video-only 模式返回旧 playlist。
- `/{ns}/{stream}/init.mp4` 返回 video init segment。
- `/{ns}/{stream}/part_N.m4s` 返回 video part。
- `/{ns}/{stream}/seg_N.m4s` 返回 video segment。

### Request Parser

文件：`crates/protocols/hls/core/src/request.rs`

新增请求类型：

```rust
pub enum HlsRequestKind {
    TrackMediaPlaylist {
        stream_key: StreamKeyParts,
        lane: TrackLane,
        session_id: Option<u64>,
        blocking: Option<BlockingParams>,
        skip: Option<SkipMode>,
        legacy: bool,
        rewind: bool,
        key_token: Option<String>,
    },
    TrackInitSegment {
        stream_key: StreamKeyParts,
        lane: TrackLane,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    TrackPart {
        stream_key: StreamKeyParts,
        lane: TrackLane,
        part_name: String,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
    TrackSegment {
        stream_key: StreamKeyParts,
        lane: TrackLane,
        segment_name: String,
        session_id: Option<u64>,
        key_token: Option<String>,
    },
}
```

`HlsCoreEvent` 增加对应事件，字段必须携带 `lane`。Core 仍只做解析和事件输出，不读取 muxer。

---

## 4. Module Pending 与 Blocking 行为

现有 pending map 以 stream 为核心。demuxed 模式必须扩展到 per-lane：

```rust
struct PendingKey {
    stream_key: String,
    lane: Option<TrackLane>, // None 仅用于 master 级等待；track 请求必须 Some
}
```

规则：

- `chunklist_video.m3u8?_HLS_msn=M&_HLS_part=P` 只等待 video lane。
- `chunklist_audio.m3u8?_HLS_msn=M&_HLS_part=P` 只等待 audio lane。
- `video_part_N.m4s` 只等待 video lane part；`audio_part_N.m4s` 只等待 audio lane part。
- content notification 必须携带 `{stream_key, lane}`，只释放对应 lane pending。
- stream conclude 必须释放该 stream 所有 lane pending。
- muxer 被移除时，所有 lane pending 都必须返回当前 playlist 或 404，不允许连接悬挂。
- `blocking_timeout_ms` 的 module timer 继续生效，driver 不抢先 503。

注意：URL 中 `video_part_N` 的 `N` 是该 lane 全局 part 序号；playlist blocking query `_HLS_part` 是目标 MSN 内 part index。实现必须区分这两种编号。

---

## 5. Cache、Gzip、Origin 和 Stream Key

- Master playlist 使用 `master_playlist_max_age`。
- 普通 chunklist 使用 `chunklist_max_age`。
- 带 `_HLS_msn` / `_HLS_part` / `_HLS_skip` 的 blocking chunklist 使用 `chunklist_with_directives_max_age`。
- Segment 使用 `segment_max_age`。
- Part 使用 `partial_segment_max_age`。
- 所有 gzip playlist 响应必须设置 `Content-Encoding: gzip` 和 `Vary: Accept-Encoding`。
- `origin_mode=true` 时不设置 `Set-Cookie`，playlist/segment/part URL 不强制 `uid`。
- `stream_key_validation=true` 时，master/chunklist 中所有 init/segment/part/preload hint URL 都追加 `k=<stream_key>`；请求校验失败返回 404。

---

## 6. Module 事件处理

文件：`crates/protocols/hls/module/src/module.rs`

新增 lane-aware 分支：

```rust
HlsCoreEvent::TrackInitSegmentRequested { stream_key, lane, key_token, .. } => {
    // validate stream key, serve mux.track_init_segment(lane)
}

HlsCoreEvent::TrackPartRequested { stream_key, lane, part_name, key_token, .. } => {
    // parse lane global seq, serve or queue pending for mux.track_part(lane, seq)
}

HlsCoreEvent::TrackMediaPlaylistRequested { stream_key, lane, blocking, .. } => {
    // non-blocking: serve mux.track_playlist(lane, ...)
    // blocking: queue/release by stream + lane + msn + part
}
```

旧 `InitSegmentRequested` / `PartRequested` / `SegmentRequested` 在 demuxed 模式下映射到 video lane，保持现有客户端兼容。

---

## 7. 测试计划

| 测试 | 验证点 |
|------|--------|
| `parse_track_media_playlist_urls` | 解析 `chunklist_video.m3u8` / `chunklist_audio.m3u8` |
| `parse_track_init_part_segment_urls` | 解析 `init_*`、`*_part_N`、`*_seg_N` |
| `master_playlist_has_audio_rendition` | master 包含 `EXT-X-MEDIA:TYPE=AUDIO` 和 `AUDIO="audio"` |
| `master_playlist_video_only_has_no_audio_group` | video-only 流不输出 audio group |
| `audio_only_master_points_to_audio_chunklist` | audio-only 流生成合法 variant |
| `video_chunklist_has_audio_rendition_report` | video chunklist 报告 audio state |
| `audio_chunklist_has_video_rendition_report` | audio chunklist 报告 video state |
| `track_playlist_map_uses_track_init` | video/audio chunklist 分别引用自己的 init |
| `track_pending_releases_only_matching_lane` | video notification 不释放 audio pending，反之亦然 |
| `stream_conclude_releases_all_track_pending` | 流结束释放所有 lane pending |
| `track_stream_key_validation_rejects_bad_key` | per-track media URL key 校验失败返回 404 |
| `legacy_urls_map_to_video_lane` | `init.mp4` / `part_N.m4s` / `index.m3u8` 兼容 |
