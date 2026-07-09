# Phase 01 — HLS 文件生成 + HTTP 服务

- **状态**: 未开始
- **范围**: TS/fMP4 文件写入磁盘、M3U8 文件管理、HTTP 静态文件服务器
- **完成标准**: ffmpeg 推流后磁盘生成 .ts + .m3u8 文件，浏览器可通过 HTTP 播放

---

## 1.1 TS Segment 文件写入

**ZLMediaKit 参考**: `HlsMakerImp::onWriteSegment()` 将 TS 数据写入磁盘文件。

**实现方案**:

新增 `HlsFileWriter` 在 driver 层：

```rust
// cheetah-hls-driver-tokio/src/file_writer.rs
pub struct HlsFileWriter {
    output_dir: PathBuf,
    current_file: Option<File>,
    current_name: String,
}

impl HlsFileWriter {
    pub async fn open_segment(&mut self, name: &str) -> io::Result<()>;
    pub async fn write_data(&mut self, data: &[u8]) -> io::Result<()>;
    pub async fn close_segment(&mut self) -> io::Result<()>;
}
```

**文件名格式** (参考 ZLMediaKit):
```
{output_dir}/{stream_key}/
├── index.m3u8
├── 2026-05-14/
│   └── 22/
│       ├── 00-00_0.ts
│       ├── 00-04_1.ts
│       └── 00-08_2.ts
```

**改动点**:
- `cheetah-hls-driver-tokio`: 新增 `file_writer.rs`
- `StreamMuxer::finalize_segment()`: 可选写入磁盘（通过回调或 channel）
- 配置: `recording.enabled`, `recording.output_dir`

---

## 1.2 fMP4 Init Segment + Media Segment 生成

**ZLMediaKit 参考**: `MP4MuxerMemory` 生成 ftyp+moov (init) 和 moof+mdat (media)。

**实现方案**:

在 `cheetah-hls-core` 新增 `fmp4_mux.rs`：

```rust
pub struct Fmp4Muxer {
    tracks: Vec<Fmp4Track>,
    sequence_number: u32,
}

impl Fmp4Muxer {
    pub fn generate_init_segment(&self) -> Bytes;  // ftyp + moov
    pub fn generate_media_segment(&mut self, samples: &[Fmp4Sample]) -> Bytes;  // moof + mdat
}

pub struct Fmp4Track {
    pub codec: CodecId,
    pub track_id: u32,
    pub timescale: u32,
    pub extradata: Bytes,
}

pub struct Fmp4Sample {
    pub track_id: u32,
    pub pts: u64,
    pub dts: u64,
    pub is_keyframe: bool,
    pub data: Bytes,
}
```

**MP4 Box 结构**:
- Init: `ftyp` + `moov` (含 `mvhd` + `trak[]` + `mvex`)
- Media: `styp` + `moof` (含 `mfhd` + `traf[]`) + `mdat`

---

## 1.3 M3U8 文件管理

**ZLMediaKit 参考**: `HlsMaker::makeIndexFile()` 生成 live/VOD 两种模式。

**实现方案**:

扩展 `PlaylistBuilder` 支持文件写入模式：

```rust
// cheetah-hls-core/src/playlist.rs
impl PlaylistBuilder {
    /// Generate a VOD playlist with EXT-X-ENDLIST.
    pub fn build_vod(segments: &[SegmentFileInfo]) -> String;

    /// Generate a live playlist with sliding window.
    pub fn build_live_file(
        segments: &[SegmentFileInfo],
        media_sequence: u64,
        target_duration: u32,
        container: HlsContainer,
    ) -> String;
}

pub struct SegmentFileInfo {
    pub filename: String,
    pub duration_secs: f64,
}
```

**Live 模式**: 保留最近 N 个 segment，`#EXT-X-MEDIA-SEQUENCE` 递增。
**VOD 模式**: 保留所有 segment，结束时追加 `#EXT-X-ENDLIST`。

---

## 1.4 HTTP 静态文件服务器

**ZLMediaKit 参考**: `HttpFileManager` 根据 URL 后缀路由到文件系统。

**实现方案**:

扩展 HLS driver 的 HTTP 处理，增加文件服务能力：

```rust
// 在 run_connection 中，对于 segment 请求：
// 1. 先查内存 SegmentRing（低延迟）
// 2. 若未命中，查磁盘文件（录制/VOD 场景）
```

**路由规则**:
- `/{app}/{stream}.m3u8` → 内存 master playlist
- `/{app}/{stream}/index.m3u8` → 内存/文件 media playlist
- `/{app}/{stream}/*.ts` → 内存 segment 或磁盘文件
- `/{app}/{stream}/*.m4s` → 内存 segment 或磁盘文件
- `/{app}/{stream}/init.mp4` → fMP4 init segment

**Content-Type 映射**:
- `.m3u8` → `application/vnd.apple.mpegurl`
- `.ts` → `video/mp2t`
- `.m4s` / `.mp4` → `video/mp4`

---

## 1.5 Segment 文件名格式

**ZLMediaKit 参考**: `YYYY-MM-DD/HH/MM-SS_<index>.ts`

**实现方案**:

```rust
fn segment_filename(seq: u64, start_time: SystemTime) -> String {
    let dt = start_time.duration_since(UNIX_EPOCH).unwrap();
    let secs = dt.as_secs();
    // Format: seg_{unix_timestamp}_{seq}.ts
    format!("seg_{}_{}.ts", secs, seq)
}
```

简化版本先用 `seg_{seq}.ts`，后续可扩展为时间目录结构。

---

## 验证方法

1. 推流 → 检查磁盘文件生成（.ts + .m3u8）
2. curl 拉取 .m3u8 → 验证内容正确
3. ffplay 通过 HTTP 播放 HLS
4. hls.js demo 页面播放验证
