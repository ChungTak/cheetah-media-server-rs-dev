# LLHLS 协议完善计划（对标 OvenMediaEngine 工程实践）

- **状态**: 规划中
- **前置**: `dev-docs/plans-21-llhls-mediamtx`（对标 mediamtx 增强计划，已完成）
- **目标**: 参考 OvenMediaEngine (v0.16+) 的 LLHLS 实现，在已有 LLHLS 基础上补齐标准协议剩余能力并新增非标兼容特性，达到生产级 LLHLS 服务器水平
- **方法**: 逐项对比 `vendor-ref/OvenMediaEngine/src/projects/publishers/llhls/` 的实现模式，在 `core + driver + module` 三段式架构下分阶段补齐
- **完成标准**: 所有阶段通过 `cargo fmt` + `cargo clippy` + `cargo test`，hls.js / Safari / VLC 端到端 LLHLS 验证通过

---

## 与 OvenMediaEngine 对比后的增强缺口

| 能力 | OME 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| Blocking Playlist Reload（完整 long-poll） | session pending_requests + OnPlaylistUpdated 释放 | ⚠️ core 有事件定义，driver/module 未实现 | Phase 01 |
| Partial Segment Blocking | GetPartial 返回 Accepted + pending | ❌ 未实现 | Phase 01 |
| MAX_PENDING_REQUESTS 限制 | 硬编码 10 | ❌ 未实现 | Phase 01 |
| Blocking 超时保护 | session_life_time + 连接断开检测 | ❌ 未实现 | Phase 01 |
| `_HLS_legacy` 模式 | playlist 退化为传统 HLS（无 PART/SERVER-CONTROL） | ❌ 未实现 | Phase 02 |
| `_HLS_rewind` 模式 | playlist 输出所有 segments（DVR 时移） | ❌ 未实现 | Phase 02 |
| Gzip 响应压缩 | Accept-Encoding 检测 + CompressGzip | ❌ 未实现 | Phase 02 |
| Playlist 缓存 | 预生成 default chunklist + gzip 版本 | ❌ 每请求重新构建 | Phase 02 |
| Cache-Control 精细控制 | 5 级 max-age 配置 | ⚠️ 仅 no-cache | Phase 02 |
| ComputeOptimalPartDuration | 基于帧率/采样率计算帧对齐 part 时长 | ❌ 固定 part_target_ms | Phase 03 |
| EXT-X-RENDITION-REPORT | cross-track LAST-MSN/LAST-PART 报告 | ❌ 未实现 | Phase 03 |
| ConcludeLive（直播结束） | 追加 EXT-X-ENDLIST 到所有 chunklist | ❌ 未实现 | Phase 03 |
| Wallclock Offset 计算 | 第一个 chunk DTS + publish time → 偏移量 | ❌ 未实现 | Phase 03 |
| EXT-X-PROGRAM-DATE-TIME | 每个 segment 开始处绝对时间 | ✅ 已实现 | — |
| Stream Key 防盗链 | segment/part URL 嵌入 stream_key 校验 | ⚠️ 有 cdn_secret 但未验证 segment 请求 | Phase 04 |
| Origin Mode（CDN 源站优化） | session 池复用，不按连接创建 session | ❌ 未实现 | Phase 04 |
| Per-Track Chunklist（多 track ABR） | 每个 track 独立 chunklist URL | ❌ 仅单 playlist | Phase 04 |
| DRM/CENC 支持 | Widevine + FairPlay + EXT-X-KEY | ❌ 未实现 | Phase 05 |
| DVR/时移（segments 持久化） | segments 写磁盘 + 旧段回放 | ⚠️ 有 file_output 但无 LLHLS DVR | Phase 05 |
| Marker/CUE 事件 | SCTE-35 广告插入标记 | ❌ 未实现 | Phase 05 |
| WebVTT 字幕 Track | subtitle track + VTT packager | ❌ 未实现 | Phase 05 |

---

## 已有能力（无需重复实现）

| 能力 | 状态 |
|------|------|
| fMP4 Part 切片 + LowLatencyState 管理 | ✅ |
| EXT-X-PART / SERVER-CONTROL / PART-INF / PRELOAD-HINT | ✅ |
| Part HTTP 端点 | ✅ |
| LL-HLS Playlist 生成 (build_media_ll) | ✅ |
| _HLS_msn / _HLS_part / _HLS_skip 请求解析 | ✅ |
| BlockingPlaylistRequested 事件 | ✅ |
| fMP4 muxer (init + moof/mdat) | ✅ |
| 内嵌 hls.js 播放页 | ✅ |
| CDN Bearer Token 配置 | ✅ |
| Session Cookie + Query Param 双模式 | ✅ |
| .mp 后缀兼容 | ✅ |
| EXT-X-PROGRAM-DATE-TIME | ✅ |
| CORS 头 | ✅ |
| HTTPS/TLS | ✅ |
| 磁盘切片模式 | ✅ |
| HLS 代理拉流 | ✅ |

---

## 总体约束

1. 严格遵循 `core + driver + module` 三段式架构
2. Blocking 等待逻辑在 driver 层实现（tokio oneshot/notify），core 层只产出事件
3. _HLS_legacy / _HLS_rewind 等非标参数解析在 core 层，行为判断在 module 层
4. Playlist 缓存在 module 层维护（每次 part/segment 更新时重建缓存）
5. Gzip 压缩在 driver 层完成（HTTP 响应构建时）
6. Cache-Control 配置在 module 层读取，driver 层写入响应头
7. ComputeOptimalPartDuration 在 module 层（需要 TrackInfo），结果注入 LowLatencyState
8. Origin Mode 在 module 层编排（session 池管理）
9. 所有 blocking 请求必须有超时上界（默认 30s），防止连接泄漏
10. pending requests 必须有数量上界（默认 10），防止内存耗尽
11. Per-track chunklist 为可选模式，默认保持单 playlist 兼容现有行为
12. DRM / DVR / CUE / WebVTT 归入 Phase 05 高级特性，按需实现

---

## 参考来源

| 来源 | 路径 |
|------|------|
| OME LLHLS Stream | `vendor-ref/OvenMediaEngine/src/projects/publishers/llhls/llhls_stream.h/cpp` |
| OME LLHLS Session | `vendor-ref/OvenMediaEngine/src/projects/publishers/llhls/llhls_session.h/cpp` |
| OME LLHLS Publisher | `vendor-ref/OvenMediaEngine/src/projects/publishers/llhls/llhls_publisher.h/cpp` |
| OME LLHLS Chunklist | `vendor-ref/OvenMediaEngine/src/projects/publishers/llhls/llhls_chunklist.h/cpp` |
| OME LLHLS Master Playlist | `vendor-ref/OvenMediaEngine/src/projects/publishers/llhls/llhls_master_playlist.h/cpp` |
| OME fMP4 Storage | `vendor-ref/OvenMediaEngine/src/projects/modules/containers/bmff/fmp4_packager/` |
| Apple LLHLS 规范 | RFC 8216bis / Apple HLS Authoring Spec |
| 本项目 HLS 实现 | `crates/protocols/hls/` |
| 前序计划 | `dev-docs/plans-21-llhls-mediamtx/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [phase-01-blocking-delivery.md](phase-01-blocking-delivery.md) | 规划中 | Blocking Playlist Reload、Partial Blocking、超时保护、pending 限制 |
| [phase-02-compat-caching.md](phase-02-compat-caching.md) | 规划中 | _HLS_legacy / _HLS_rewind、Gzip 压缩、Playlist 缓存、Cache-Control |
| [phase-03-playlist-enhancements.md](phase-03-playlist-enhancements.md) | 规划中 | ComputeOptimalPartDuration、Rendition Report、ConcludeLive、Wallclock |
| [phase-04-origin-multitrack.md](phase-04-origin-multitrack.md) | 规划中 | Origin Mode、Per-Track Chunklist、Stream Key 验证、ABR |
| [phase-05-advanced-features.md](phase-05-advanced-features.md) | 规划中 | DRM/CENC、DVR 时移、Marker/CUE、WebVTT 字幕 |

---

## 渐进式执行顺序

1. **Phase 01** — 阻塞式交付：完成 Blocking Playlist Reload / Partial Blocking 的端到端实现（从 core 事件到 driver long-poll），加入超时和 pending 上限保护
2. **Phase 02** — 兼容与缓存：实现 _HLS_legacy / _HLS_rewind 非标模式、Gzip 响应压缩、Playlist 预生成缓存、细粒度 Cache-Control
3. **Phase 03** — Playlist 增强：帧对齐 Part Duration 计算、EXT-X-RENDITION-REPORT、ConcludeLive 结束标记、Wallclock 偏移
4. **Phase 04** — Origin 与多 Track：Origin Mode session 池、Per-Track Chunklist ABR 路由、Stream Key 防盗链
5. **Phase 05** — 高级特性：DRM/CENC 加密、DVR 时移回放、SCTE-35 Marker/CUE 事件、WebVTT 字幕
