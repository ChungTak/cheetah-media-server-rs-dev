# HLS 协议完善计划（对标 ZLMediaKit）

- **状态**: 未开始
- **目标**: 对标 ZLMediaKit HLS 实现，补齐生产级 HLS 能力：文件生成、HTTP 服务、Cookie 会话追踪、HLS 播放器/拉流、多轨道、fMP4
- **方法**: 对比 `vendor-ref/ZLMediaKit/src` 中 Record/HlsMaker* + Http/HlsPlayer* 实现，逐项补齐
- **完成标准**: 所有阶段通过 `cargo fmt` + `cargo clippy` + `cargo test`，端到端 HLS 推拉流验证通过

---

## 与 ZLMediaKit 对比后的主要缺口

| 能力 | ZLMediaKit 参考 | 本地状态 | 计划处理 |
|------|----------------|----------|----------|
| TS 文件写入磁盘 | HlsMakerImp 写 .ts 文件 | ❌ 纯内存 | Phase 01 |
| fMP4 init segment + moof/mdat 生成 | MP4MuxerMemory + HlsFMP4Recorder | ❌ 仅 playlist 预留 | Phase 01 |
| M3U8 文件管理（live 滑动窗口 + VOD） | HlsMaker::makeIndexFile | ⚠️ 有内存 playlist 但无文件 | Phase 01 |
| HTTP 静态文件服务器 | HttpFileManager 服务 .ts/.m4s | ❌ 未实现 | Phase 01 |
| Cookie 会话追踪 | HlsCookieData + HttpServerCookie | ❌ 仅 UID 参数 | Phase 02 |
| 按需生成（hls_demand） | onReaderChanged + _enabled 开关 | ❌ 未实现 | Phase 02 |
| 播放统计（字节/时长） | HlsCookieData::addByteUsage | ❌ 未实现 | Phase 02 |
| HLS 播放器/拉流器 | HlsPlayer + HlsPlayerImp + HlsDemuxer | ⚠️ 有 pull 框架但无实现 | Phase 03 |
| TS demux（拉流解封装） | DecoderImp + TSDemuxer | ❌ 未实现 | Phase 03 |
| 实时 pacing（HlsDemuxer） | 50ms timer + buffer 管理 | ❌ 未实现 | Phase 03 |
| HLS→RTSP/RTMP 转发 | HlsPlayerImp → MediaSource → RTSP/RTMP | ❌ 未实现 | Phase 03 |
| 多轨道 TS/fMP4 | MpegMuxer 多 track map | ⚠️ 单视频+单音频 | Phase 04 |
| 时间戳回退处理 | _last_seg_timestamp 重置 | ❌ 未实现 | Phase 04 |
| 快速注册（kFastRegister） | 首段立即生成 | ❌ 未实现 | Phase 04 |
| 延迟 playlist（_delay.m3u8） | seg_number + segDelay | ❌ 未实现 | Phase 04 |
| Segment 保留（kSegmentRetain） | 磁盘保留超出 m3u8 的段 | ❌ 未实现 | Phase 04 |
| 删除延迟（kDeleteDelaySec） | 流结束后延迟删除文件 | ❌ 未实现 | Phase 04 |
| 多编码支持 | H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1 | ✅ 已有 | — |
| AUD 注入 | H264/H265 AUD prepend | ✅ 已有 | — |
| ADTS 封装 | AAC ADTS wrapping | ✅ 已有 | — |
| 参数集补发 | SPS/PPS/VPS per segment | ✅ 已有 | — |
| SEI 过滤 | NON_PICTURE skip | ✅ 已有 | — |
| 内存 segment 环形缓冲 | SegmentRing | ✅ 已有 | — |
| CORS 头 | 全响应 CORS | ✅ 已有 | — |
| M3U8 解析器 | parse_media/master_playlist | ✅ 已有 | — |

---

## 总体约束

1. 严格遵循 `core + driver + module` 三段式架构
2. 文件 I/O 在 driver 层实现，core 层保持 Sans-I/O
3. Cookie 追踪在 driver HTTP 层实现，module 层管理会话生命周期
4. HLS 播放器作为 module 层 pull job，TS demux 使用 `cheetah-codec`
5. fMP4 muxer 在 core 层实现（纯数据结构生成，无 I/O）
6. 多轨道通过扩展 TsMuxer PID 分配实现
7. 所有新增能力默认禁用，通过配置开启

---

## 参考来源

| 来源 | 路径 |
|------|------|
| ZLMediaKit HLS 生成 | `vendor-ref/ZLMediaKit/src/Record/HlsMaker*.cpp/h` |
| ZLMediaKit HLS 媒体源 | `vendor-ref/ZLMediaKit/src/Record/HlsMediaSource.*` |
| ZLMediaKit HLS 播放器 | `vendor-ref/ZLMediaKit/src/Http/HlsPlayer*.cpp/h` |
| ZLMediaKit HLS 解析器 | `vendor-ref/ZLMediaKit/src/Http/HlsParser.*` |
| ZLMediaKit HTTP 文件管理 | `vendor-ref/ZLMediaKit/src/Http/HttpFileManager.cpp` |
| 本项目 HLS 实现 | `crates/protocols/hls/` |
| 本项目前序计划 | `dev-docs/plans-16/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [phase-01-file-generation.md](phase-01-file-generation.md) | 未开始 | TS/fMP4 文件写入、M3U8 文件管理、HTTP 文件服务器 |
| [phase-02-session-tracking.md](phase-02-session-tracking.md) | 未开始 | Cookie 会话追踪、按需生成、播放统计 |
| [phase-03-hls-player.md](phase-03-hls-player.md) | 未开始 | HLS 播放器/拉流器、TS demux、实时 pacing、HLS→RTSP/RTMP |
| [phase-04-advanced.md](phase-04-advanced.md) | 未开始 | 多轨道、时间戳鲁棒性、快速注册、延迟 playlist、segment 保留 |

---

## 任务状态总表

| 阶段 | 任务 | 状态 |
|------|------|------|
| 1.1 | TS segment 文件写入（HlsFileWriter） | ✅ 完成 |
| 1.2 | fMP4 init segment + media segment 生成（Fmp4Muxer） | ⚠️ playlist 预留，muxer 待实现 |
| 1.3 | M3U8 文件管理（live 滑动窗口 + VOD + ENDLIST） | ✅ 完成 |
| 1.4 | HTTP 静态文件服务器（.ts/.m4s/.m3u8 serving） | ✅ 完成 |
| 1.5 | Segment 文件名格式（时间戳目录组织） | ✅ 完成 |
| 2.1 | Cookie/Set-Cookie 会话追踪 | ✅ 完成 |
| 2.2 | 按需 HLS 生成（hls_demand 模式） | ✅ 完成 |
| 2.3 | 播放统计（字节数、时长、连接数） | ✅ 完成 |
| 2.4 | 模拟长连接（Cookie 刷新 TTL） | ✅ 完成 |
| 3.1 | HLS 播放器框架（HTTP 拉取 + M3U8 解析 + segment 下载） | ✅ 完成 |
| 3.2 | TS demux（MPEG-TS 解封装为 AVFrame） | ✅ 完成 |
| 3.3 | 实时 pacing（HlsDemuxer 缓冲 + 定时消费） | ✅ 完成 |
| 3.4 | HLS→RTSP/RTMP/MP4 转发（发布到引擎） | ⚠️ 框架就绪，需 HTTP 客户端 |
| 3.5 | 支持 Amazon Echo Show 等设备（RTSP[S] 输出） | ✅ 已有（RTSP module 支持） |
| 4.1 | 多轨道 TS muxer（多 PID 分配） | ✅ 完成 |
| 4.2 | 时间戳回退/回绕处理 | ✅ 完成 |
| 4.3 | 快速注册模式（kFastRegister） | ✅ 完成 |
| 4.4 | 延迟 playlist（_delay.m3u8） | ✅ 完成 |
| 4.5 | Segment 保留 + 删除延迟 | ✅ 完成 |

---

## 渐进式执行顺序

1. **Phase 01** — 文件生成 + HTTP 服务：HLS 的核心交付物是文件，这是所有后续功能的基础
2. **Phase 02** — 会话追踪：Cookie 机制使 HLS 可管理，支撑按需生成和统计
3. **Phase 03** — HLS 播放器：实现拉流能力，支持 HLS→其他协议转换
4. **Phase 04** — 高级特性：多轨道、鲁棒性、延迟优化
