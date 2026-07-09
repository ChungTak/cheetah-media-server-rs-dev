# HLS 协议增强计划（对标 ZLMediaKit 完整实现）

- **状态**: 未开始
- **前置**: `dev-docs/plans-18-hls-zlm`（基础 HLS 能力已完成）
- **目标**: 在现有 HLS 基础上，对标 ZLMediaKit 补齐生产级增强能力：fMP4 封装、非标兼容、HTTP 文件服务增强、Cookie 长连接追踪、HLS 播放器完整实现、多轨道、协议互转
- **方法**: 逐项对比 `vendor-ref/ZLMediaKit/src` 中 Record/HlsMaker* + Http/HlsPlayer* + FMP4/* 实现，补齐缺口并增加非标兼容
- **完成标准**: 所有阶段通过 `cargo fmt` + `cargo clippy` + `cargo test`，端到端验证通过

---

## 与 ZLMediaKit 对比后的增强缺口

| 能力 | ZLMediaKit 参考 | 本地状态 | 计划处理 |
|------|----------------|----------|----------|
| fMP4 muxer (init + moof/mdat) | `MP4MuxerMemory` + `HlsFMP4Recorder` | ❌ 仅 playlist 预留 container=fmp4 | Phase 01 |
| fMP4 init segment 缓存与补发 | `HlsFMP4Recorder::addTrackCompleted()` | ❌ 未实现 | Phase 01 |
| fMP4 CMAF 低延迟切片 | `onSegmentData` 按 GOP 输出 | ❌ 未实现 | Phase 01 |
| HTTP Range 请求支持 | `HttpFileManager` Range/ETag | ❌ 未实现 | Phase 02 |
| HTTP 条件请求 (If-None-Match) | `HttpSession` ETag 304 | ❌ 未实现 | Phase 02 |
| 跨域预检 OPTIONS 完整实现 | `HttpSession::Handle_Req_OPTIONS` | ⚠️ 仅基础 CORS 头 | Phase 02 |
| Cookie 多设备限制 | `HttpCookieManager::max_client` | ❌ 未实现 | Phase 02 |
| Cookie 异地挤占登录 | `getOldestCookie` 淘汰策略 | ❌ 未实现 | Phase 02 |
| HLS 播放器 HTTP 客户端 | `HttpClientImp` 完整 HTTP/1.1 | ⚠️ 框架就绪，缺 HTTP 客户端 | Phase 03 |
| HLS 播放器自适应码率选择 | `HlsPlayer` bandwidth 选择 | ❌ 未实现 | Phase 03 |
| HLS 播放器 redirect 跟随 | `onRedirectUrl` 302 处理 | ❌ 未实现 | Phase 03 |
| HLS 播放器 playlist 变化检测 | `_playlist_reload_changed` | ❌ 未实现 | Phase 03 |
| TS demux 多 PID 多轨道 | `TSDemuxer` 多 track map | ⚠️ 单 video+audio | Phase 03 |
| fMP4 demux (moof/mdat 解析) | 未直接实现，走 MP4Demuxer | ❌ 未实现 | Phase 03 |
| HLS→RTSP/RTMP/MP4 完整转发 | `HlsPlayerImp` → `MediaSource` | ⚠️ 框架就绪 | Phase 03 |
| 多轨道 fMP4 (多 traf) | `MP4MuxerMemory` 多 track | ❌ 未实现 | Phase 04 |
| MP2 编码支持 | MPEG-1 Audio Layer II | ❌ 未实现 | Phase 04 |
| 非标 TS 容错 (sync byte 搜索) | 实际工程容错 | ❌ 未实现 | Phase 04 |
| 非标 PES 长度为 0 处理 | 视频 PES unbounded | ❌ 未实现 | Phase 04 |
| 非标时间戳跳变平滑 | `Stamp` 平滑器 | ⚠️ 有回退检测，缺平滑 | Phase 04 |
| Segment 文件名时间目录组织 | `YYYY-MM-DD/HH/MM-SS_idx.ts` | ❌ 仅 seq 编号 | Phase 05 |
| VOD playlist (ENDLIST) | `HlsMaker::makeIndexFile(eof=true)` | ⚠️ 有 ENDLIST 但无 VOD 模式 | Phase 05 |
| 延迟 playlist 增强 | `_delay.m3u8` + `segDelay` | ⚠️ 有基础实现 | Phase 05 |
| HLS 录制模式 (seg_keep=true) | `isKeep()` 不删除 segment | ❌ 未实现 | Phase 05 |
| HTTPS/TLS HLS 服务 | TLS listener | ❌ 未实现 | Phase 05 |
| Master playlist 多码率 | `#EXT-X-STREAM-INF` 多 variant | ❌ 未实现 | Phase 05 |

---

## 已有能力（无需重复实现）

| 能力 | 状态 |
|------|------|
| TS muxer (H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1) | ✅ |
| 多轨道 TS muxer (TsMuxerMulti) | ✅ |
| AUD 注入 (H264/H265) | ✅ |
| ADTS 封装 (AAC) | ✅ |
| 参数集补发 (SPS/PPS/VPS per segment) | ✅ |
| SEI 过滤 | ✅ |
| 内存 segment 环形缓冲 (SegmentRing) | ✅ |
| M3U8 解析器 (master + media) | ✅ |
| TS demux 基础 (PAT/PMT/PES) | ✅ |
| HLS playback pacer | ✅ |
| Cookie 会话追踪基础 | ✅ |
| 按需生成 (hls_demand) | ✅ |
| 播放统计 (bytes/segments) | ✅ |
| 快速注册 (fast_register) | ✅ |
| 时间戳回退检测 | ✅ |
| HTTP 文件服务基础 | ✅ |
| CORS 头 | ✅ |
| Segment 保留 + 删除延迟 | ✅ |
| HLS pull job 框架 | ✅ |

---

## 总体约束

1. 严格遵循 `core + driver + module` 三段式架构
2. fMP4 muxer 在 core 层实现（纯数据结构生成，无 I/O）
3. HTTP 增强在 driver 层实现
4. Cookie 增强在 driver HTTP 层实现，module 层管理会话生命周期
5. HLS 播放器 HTTP 客户端在 driver 层，协议逻辑在 core 层
6. 非标兼容逻辑集中在 core 层的 compat 子模块
7. 多轨道通过扩展 fMP4 muxer 的 traf 生成实现
8. 所有新增能力默认禁用，通过配置开启
9. MP2 编码支持在 `cheetah-codec` 层添加

---

## 参考来源

| 来源 | 路径 |
|------|------|
| ZLMediaKit HLS 生成 | `vendor-ref/ZLMediaKit/src/Record/HlsMaker*.cpp/h` |
| ZLMediaKit HLS 录制器 | `vendor-ref/ZLMediaKit/src/Record/HlsRecorder.h` |
| ZLMediaKit HLS 媒体源 | `vendor-ref/ZLMediaKit/src/Record/HlsMediaSource.*` |
| ZLMediaKit fMP4 源 | `vendor-ref/ZLMediaKit/src/FMP4/FMP4MediaSource.h` |
| ZLMediaKit HLS 播放器 | `vendor-ref/ZLMediaKit/src/Http/HlsPlayer*.cpp/h` |
| ZLMediaKit HLS 解析器 | `vendor-ref/ZLMediaKit/src/Http/HlsParser.*` |
| ZLMediaKit HTTP Cookie | `vendor-ref/ZLMediaKit/src/Http/HttpCookieManager.*` |
| ZLMediaKit HTTP 文件管理 | `vendor-ref/ZLMediaKit/src/Http/HttpFileManager.cpp` |
| ZLMediaKit 时间戳平滑 | `vendor-ref/ZLMediaKit/src/Common/Stamp.*` |
| 本项目 HLS 实现 | `crates/protocols/hls/` |
| 本项目前序计划 | `dev-docs/plans-18-hls-zlm/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [phase-01-fmp4-muxer.md](phase-01-fmp4-muxer.md) | 未开始 | fMP4 muxer 实现、init segment、CMAF 低延迟 |
| [phase-02-http-enhance.md](phase-02-http-enhance.md) | 未开始 | HTTP Range/ETag、OPTIONS 预检、Cookie 多设备限制 |
| [phase-03-hls-player-full.md](phase-03-hls-player-full.md) | 未开始 | HLS 播放器完整实现、自适应码率、fMP4 demux、协议互转 |
| [phase-04-compat-robustness.md](phase-04-compat-robustness.md) | 未开始 | 非标兼容、容错、时间戳平滑、MP2 编码 |
| [phase-05-production.md](phase-05-production.md) | 未开始 | 录制模式、VOD、HTTPS、Master playlist 多码率 |

---

## 渐进式执行顺序

1. **Phase 01** — fMP4 Muxer：补齐 fMP4 封装能力，支持 CMAF 低延迟 HLS
2. **Phase 02** — HTTP 增强：Range 请求、条件请求、Cookie 高级策略
3. **Phase 03** — HLS 播放器完整实现：HTTP 客户端、自适应码率、fMP4 demux、协议互转
4. **Phase 04** — 非标兼容与鲁棒性：TS 容错、PES 容错、时间戳平滑、MP2
5. **Phase 05** — 生产级特性：录制模式、VOD、HTTPS、多码率 Master playlist
