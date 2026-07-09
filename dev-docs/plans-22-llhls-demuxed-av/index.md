# LLHLS 音视频分离打包计划（对标 OvenMediaEngine Per-Track 架构）

- **状态**: 规划中
- **前置**: `dev-docs/plans-21-llhls-ome`（LLHLS 协议完善计划）
- **目标**: 将当前 LLHLS fMP4 的 video-only workaround 升级为完整 demuxed audio/video 输出，解决 Chrome/hls.js 下 muxed CMAF `audiovideo` SourceBuffer 不稳定的问题，实现完整音视频 LLHLS 播放
- **方法**: 参考 OvenMediaEngine 的 per-track packager/storage/chunklist 架构，为 video/audio lane 分别生成 init segment、parts、segments、chunklist，并通过 master playlist 声明独立 audio rendition
- **完成标准**: hls.js + Chrome 能自动创建独立 video/audio SourceBuffer 并正常播放音视频，`ffplay` 可播放主 HLS URL，端到端延迟目标 < 2s

---

## 问题背景

当前实现已经不是完整 muxed A/V LLHLS：为了规避 Chrome MSE 对 muxed CMAF fragments 的兼容问题，`StreamMuxer` 在 LLHLS fMP4 模式下跳过 audio track 和 audio frame，只输出 video-only fMP4。这个 workaround 能让画面播放，但不满足完整音视频 LLHLS。

如果恢复单一 `Fmp4Muxer` 同时封装 audio/video，init segment 会包含两个 `trak`，part/segment 中可能出现多个 `traf`。hls.js 会创建 muxed `audiovideo` SourceBuffer，Chrome MSE 在该路径下容易触发 `bufferAppendError`。

OvenMediaEngine 的工程做法是：**每个 media track 独立打包**。每个 track 有自己的 packager、storage、chunklist 和资源 URL；master playlist 用 `#EXT-X-MEDIA:TYPE=AUDIO` 声明独立 audio rendition，hls.js 因此创建独立 video/audio SourceBuffer。

---

## 架构对比

### 当前架构（LLHLS fMP4 video-only workaround）

```text
RTSP/RTMP Push -> Engine -> HLS Subscriber
                              |
                         StreamMuxer
                              |
                init.mp4       只包含 video trak
                part_N.m4s     只包含 video traf
                index.m3u8     单 video playlist
```

### 目标架构（Demuxed A/V，对标 OME）

```text
RTSP/RTMP Push -> Engine -> HLS Subscriber
                              |
                    DemuxedStreamMuxer
                       /              \
             Video TrackMuxer      Audio TrackMuxer
                  |                    |
           init_video.mp4       init_audio.mp4
           video_part_N.m4s     audio_part_N.m4s
           video_seg_N.m4s      audio_seg_N.m4s
           chunklist_video.m3u8 chunklist_audio.m3u8
                       \              /
                         master.m3u8
                 EXT-X-MEDIA:TYPE=AUDIO
```

---

## OME 关键实现参考

| 组件 | OME 源码 | 本项目参考点 |
|------|----------|--------------|
| Per-track 创建 | `llhls_stream.cpp::AddPackager()` | 每个 supported track 创建独立 packager/storage/chunklist |
| 文件命名 | `llhls_stream.cpp::GetChunklistName/GetInitializationSegmentName/GetSegmentName/GetPartialSegmentName` | URL 必须携带 lane/track 语义，segment/part 可独立路由 |
| Per-track fMP4 | `fmp4_packager/fmp4_packager.cpp` | 每个 packager 只写一个 track 的 init/moof/mdat |
| Per-track storage | `fmp4_packager/fmp4_storage.cpp` | 每个 track 独立 segment/partial 查询 |
| Chunklist | `llhls_chunklist.cpp::MakeChunklist` | `EXT-X-MAP`、`EXT-X-PART`、`PRELOAD-HINT`、`RENDITION-REPORT`、`ENDLIST` |
| Master playlist | `llhls_master_playlist.cpp::MakePlaylist` | `EXT-X-MEDIA`、`STREAM-INF`、`CODECS`、`AUDIO` group |
| Blocking 请求 | `llhls_stream.cpp::GetChunklist/GetPartial` | pending 条件按 track + msn + part 判断 |

---

## 计划文件清单

| 文件 | 范围 |
|------|------|
| [phase-01-per-track-muxer.md](phase-01-per-track-muxer.md) | 建立 per-track muxer，移除 video-only workaround，生成独立 init/part/segment |
| [phase-02-demuxed-playlist.md](phase-02-demuxed-playlist.md) | 生成 demuxed master/chunklist，扩展 core 路由和 module pending 处理 |
| [phase-03-hlsjs-integration.md](phase-03-hlsjs-integration.md) | hls.js/ffplay/ffprobe 端到端验证和回归测试 |

---

## 总体约束

1. 严格遵守 `core + driver + module` 三段式架构。
2. `cheetah-hls-core` 只扩展 Sans-I/O 请求解析、playlist 构建和 fMP4 单 track 封装能力，不接入 engine 或 runtime。
3. Per-track 编排在 `cheetah-hls-module`，不要把业务状态放进 driver。
4. 新增实现应拆出 focused module（如 `track_muxer.rs`、`demuxed_muxer.rs`），不要继续膨胀 `muxer.rs`。
5. 默认目标为单 video + 单 audio 完整 LLHLS；多码率 ABR、多语言音轨、字幕、DRM/DVR 只保留扩展点。
6. TS 容器和非 LLHLS fMP4 保持现有行为；浏览器 LLHLS 默认使用 demuxed A/V。
7. 配置向后兼容：新增 `ll_hls_packaging_mode`，默认 `demuxed-av`，可回退到当前 `video-only`。
8. Origin mode、stream key validation、gzip、cache-control、blocking timeout 必须覆盖 per-track URL。

---

## 渐进式执行顺序

1. **Phase 01** — Per-Track Muxer：建立 video/audio 独立 muxer 和资源存储，恢复 audio frame 进入 LLHLS。
2. **Phase 02** — Demuxed Playlist 与路由：生成 master/audio/video chunklist，扩展 per-track URL、pending 请求和 cache/security 行为。
3. **Phase 03** — hls.js 集成验证：用浏览器、ffprobe、ffplay 验证 SourceBuffer、音视频 packets、低延迟和回归场景。
