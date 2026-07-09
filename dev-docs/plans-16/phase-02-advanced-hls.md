# Phase 02 — 高级 HLS 功能

- **状态**: 未开始
- **范围**: fMP4 (CMAF) 容器支持、LL-HLS、M3U8 pull 解析
- **完成标准**: hls.js 可播放 fMP4 segment；LL-HLS 延迟 < 2s；可从远端 HLS 源拉流

---

## 2.1 fMP4 (CMAF) Segment 生成

**问题**: 现代播放器（hls.js、Safari 17+）优先使用 fMP4 容器，支持更好的 codec 兼容性（H265/AV1 在 TS 中兼容性差）。

**simple-media-server 参考**: `Fmp4Muxer` + `MOV_FLAG_SEGMENT`，生成 init segment + media segment。

**实现方案**:

在 `cheetah-hls-core` 中新增 `fmp4_mux.rs` 模块：

```
core/src/
├── ts_mux.rs      (已有)
└── fmp4_mux.rs    (新增)
```

关键类型：
- `Fmp4Muxer` — 生成 init segment (ftyp+moov) 和 media segment (moof+mdat)
- `Fmp4InitSegment` — 缓存的 init segment bytes
- `Fmp4MediaSegment` — 单个 moof+mdat

**Playlist 变化**:
- Media playlist 增加 `#EXT-X-MAP:URI="init.mp4"` 指向 init segment
- Segment 文件后缀改为 `.m4s`
- `#EXT-X-VERSION` 升至 7

**配置**:
```yaml
modules:
  hls:
    container: "ts"  # "ts" | "fmp4"
```

**改动点**:
- `cheetah-hls-core`: 新增 `fmp4_mux.rs`
- `PlaylistBuilder`: 支持 fMP4 模式（EXT-X-MAP + .m4s 后缀）
- `StreamMuxer`: 根据配置选择 TsMuxer 或 Fmp4Muxer
- `SegmentRing`: 新增 init segment 存储

---

## 2.2 LL-HLS (Low-Latency HLS)

**问题**: 传统 HLS 延迟 = segment_duration × 3 ≈ 12s，LL-HLS 可降至 1-2s。

**simple-media-server 参考**: `LLHlsMuxer` 使用 `EXT-X-PART` 子分片，默认 500ms 一个 part。

**实现方案**:

新增 `ll_hls.rs` 模块处理 part 级别分片：

```rust
pub struct LowLatencyState {
    parts: Vec<Part>,
    part_target_duration_ms: u64,
    part_seq: u64,
}

pub struct Part {
    pub uri: String,
    pub duration_secs: f64,
    pub independent: bool,
    pub data: Bytes,
}
```

**Playlist 扩展** (参考 m3u8-rs `ServerControl` / `PartInf` / `Part`):
```
#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=1.0
#EXT-X-PART-INF:PART-TARGET=0.5
#EXT-X-PART:DURATION=0.5,URI="part0.m4s",INDEPENDENT=YES
```

**HTTP 扩展**:
- `_HLS_msn` / `_HLS_part` 查询参数支持阻塞请求
- 服务端 hold 请求直到新 part 可用

**改动点**:
- `cheetah-hls-core`: 新增 `ll_hls.rs`
- `PlaylistBuilder`: 支持 LL-HLS 标签生成
- `HlsCore` session: 解析 `_HLS_msn`/`_HLS_part` 参数
- Driver: 支持 long-poll（hold connection 直到新数据）

---

## 2.3 M3U8 Pull 解析

**问题**: 需要从远端 HLS 源拉取流并转发（HLS relay/pull）。

**simple-media-server 参考**: `HlsParser` 解析 `#EXTINF`、`#EXT-X-MEDIA-SEQUENCE`、`#EXT-X-STREAM-INF`。

**实现方案**:

参考 `vendor-ref/m3u8-rs` 的数据模型，在 core 层实现轻量解析器：

```rust
// cheetah-hls-core/src/parser.rs
pub struct ParsedMediaPlaylist {
    pub target_duration: u32,
    pub media_sequence: u64,
    pub segments: Vec<ParsedSegment>,
    pub end_list: bool,
}

pub struct ParsedSegment {
    pub duration: f64,
    pub uri: String,
}

pub fn parse_media_playlist(input: &str) -> Result<ParsedMediaPlaylist, HlsCoreError>;
```

**Module 层 pull job**:
- 定期 GET master playlist → 选择 variant → GET media playlist
- 检测新 segment → GET segment → TS demux → 发布到引擎

**改动点**:
- `cheetah-hls-core`: 新增 `parser.rs`
- `cheetah-hls-module`: 新增 `pull.rs`（类似 http-flv 的 pull job）
- 配置: `pull_jobs` 数组

---

## 验证方法

1. fMP4: hls.js demo 页面播放 H265 fMP4 HLS
2. LL-HLS: 测量首帧延迟 < 2s
3. Pull: 从外部 HLS 源拉流后通过本地 RTMP/HLS 播放
