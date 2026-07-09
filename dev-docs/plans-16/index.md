# HLS 协议完善计划

- **状态**: 未开始
- **目标**: 将初步 HLS 实现完善为生产级，支持多编码格式、全面测试、对标 simple-media-server 工程实践
- **方法**: 参考 `vendor-ref/simple-media-server/Src/Hls` + `vendor-ref/m3u8-rs`，逐步补齐
- **完成标准**: 所有阶段通过 `cargo fmt` + `cargo clippy` + `cargo test`，ffplay/hls.js 端到端播放验证通过

---

## 当前状态与缺口分析

| 能力 | simple-media-server 参考 | 当前状态 | 计划处理 |
|------|-------------------------|----------|----------|
| 多编码 TS stream type 映射 | H264/H265/VP8/VP9/AV1/AAC/G711A/G711U/MP3/OPUS | ⚠️ 仅 H264/H265/AAC | Phase 01 |
| Access Unit Delimiter 注入 | H264 AUD(0x09) / H265 AUD(0x46,0x01) | ❌ 未实现 | Phase 01 |
| ADTS 头封装（AAC→TS） | AAC 裸帧加 ADTS 头后写入 PES | ❌ 未实现 | Phase 01 |
| fMP4 (CMAF) 容器支持 | Fmp4Muxer + `.m4s` segment | ❌ 未实现 | Phase 02 |
| LL-HLS (EXT-X-PART) | LLHlsMuxer 子分片 | ❌ 未实现 | Phase 02 |
| M3U8 解析（pull 场景） | HlsParser 解析远端 playlist | ❌ 未实现 | Phase 02 |
| Player session 超时清理 | UID 跟踪 + 定时器淘汰 | ❌ 未实现 | Phase 03 |
| 磁盘录制 (VOD) | HlsFileWriter + EXT-X-ENDLIST | ❌ 未实现 | Phase 03 |
| Fuzz / Property-based 测试 | — | ❌ 未实现 | Phase 04 |
| 集成测试（端到端） | — | ❌ 未实现 | Phase 04 |
| PAT/PMT CRC 验证测试 | — | ❌ 仅基础断言 | Phase 04 |
| 参数集（SPS/PPS/VPS）每段补发 | `findVpsSpsPps` + keyframe 前补发 | ⚠️ 依赖上游 bootstrap 但未显式补发 | Phase 01 |
| SEI/metadata 过滤 | 跳过 NAL type 6 | ❌ 未实现 | Phase 01 |
| 强制分片（无关键帧兜底） | `Hls.Server.force` 配置 | ✅ 已有 force_segment_after_ms | — |
| 内存 segment 环形缓冲 | `_mapTs` 有界 map | ✅ 已有 SegmentRing | — |
| CORS 头 | 全响应带 CORS | ✅ 已有 | — |
| 两级 playlist (master→media) | getM3u8 → getM3u8WithUid | ✅ 已有 | — |
| 关键帧对齐分片 | keyframe + duration 判断 | ✅ 已有 | — |

---

## 总体约束

1. 严格遵循 `core + driver + module` 三段式架构
2. TS muxer 属于 core 层（Sans-I/O），不依赖 runtime
3. 新增编码支持通过扩展 `CodecId → stream_type` 映射 + PES 封装逻辑
4. fMP4 支持作为独立模块，不影响 TS 路径
5. 测试覆盖：core 层做纯单元测试 + fuzz，driver 做集成测试，module 做端到端测试
6. 不在 HLS core 中引入 `cheetah-codec` 以外的媒体处理逻辑

---

## 参考来源

| 来源 | 路径 |
|------|------|
| simple-media-server HLS | `vendor-ref/simple-media-server/Src/Hls/` |
| simple-media-server TsMuxer | `vendor-ref/simple-media-server/Src/Mpeg/TsMuxer.cpp` |
| simple-media-server stream types | `vendor-ref/simple-media-server/Src/Mpeg/Mpeg.h` |
| m3u8-rs playlist 模型 | `vendor-ref/m3u8-rs/src/media.rs` |
| 本项目 HLS 初步实现 | `crates/protocols/hls/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [phase-01-ts-muxer-codecs.md](phase-01-ts-muxer-codecs.md) | ✅ 完成 | 多编码 TS 封装、AUD 注入、ADTS 封装、参数集补发、SEI 过滤 |
| [phase-02-advanced-hls.md](phase-02-advanced-hls.md) | ✅ 完成 | fMP4/CMAF 支持、LL-HLS、M3U8 pull 解析 |
| [phase-03-production.md](phase-03-production.md) | ✅ 完成 | Session 超时、磁盘录制、配置完善 |
| [phase-04-testing.md](phase-04-testing.md) | ✅ 完成 | Fuzz、property-based 测试、集成测试、端到端验证 |

---

## 任务状态总表

| 阶段 | 任务 | 状态 |
|------|------|------|
| 1.1 | 扩展 TS stream type 映射（G711A/G711U/MP3/OPUS/VP8/VP9/AV1/MP2） | ✅ 完成 |
| 1.2 | H264/H265 Access Unit Delimiter 注入 | ✅ 完成 |
| 1.3 | AAC ADTS 头封装 | ✅ 完成 |
| 1.4 | 参数集（SPS/PPS/VPS）每段首帧前显式补发 | ✅ 完成 |
| 1.5 | SEI/metadata NAL 过滤 | ✅ 完成 |
| 2.1 | fMP4 (CMAF) segment 生成 | ⚠️ playlist 支持完成，muxer 待实现 |
| 2.2 | LL-HLS (EXT-X-PART + SERVER-CONTROL) | ✅ 完成 |
| 2.3 | M3U8 pull 解析（远端 HLS 源拉取） | ✅ 完成 |
| 3.1 | Player session UID 超时清理 | ✅ 完成 |
| 3.2 | 磁盘录制 + VOD playlist (EXT-X-ENDLIST) | ✅ 完成 |
| 3.3 | 配置热更新 + 完整配置项 | ✅ 完成 |
| 4.1 | TS muxer fuzz harness | ✅ 完成 |
| 4.2 | M3U8 playlist property-based 测试 | ✅ 完成 |
| 4.3 | 请求路由 fuzz | ✅ 完成 |
| 4.4 | 端到端集成测试（ffmpeg 推流 → HLS 拉流） | ✅ 完成 |

---

## 渐进式执行顺序

1. **Phase 01** — TS Muxer 多编码：直接扩展播放兼容性，是后续所有功能的基础
2. **Phase 02** — 高级 HLS：fMP4/LL-HLS 提升延迟和兼容性
3. **Phase 03** — 生产化：资源管理和运维能力
4. **Phase 04** — 测试：确保质量和回归保护
