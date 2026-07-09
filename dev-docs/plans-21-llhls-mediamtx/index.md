# LLHLS 协议完整实现计划（对标 mediamtx/gohlslib 工程实践）

- **状态**: 已完成
- **前置**: `dev-docs/plans-20-hls-abl`（对标 ABLMediaServer 增强计划，已完成）
- **目标**: 参考 mediamtx (gohlslib v2.3.2) 的 LLHLS 实现，在现有传统 HLS 基础上完成端到端 Low-Latency HLS 交付，同时实现非标准兼容特性提高鲁棒性
- **方法**: 逐项对比 `vendor-ref/mediamtx/internal/servers/hls/` 和 gohlslib 的实现模式，在 `core + driver + module` 三段式架构下分阶段补齐 LLHLS 能力
- **完成标准**: 所有阶段通过 `cargo fmt` + `cargo clippy` + `cargo test`，hls.js LLHLS 模式端到端验证通过

---

## 与 mediamtx/gohlslib 对比后的增强缺口

| 能力 | mediamtx 参考 | 本地状态 | 计划处理 |
|------|---------------|----------|----------|
| Part 级别切片（200-500ms fMP4 fragment） | gohlslib `PartMinDuration` | ❌ 仅完整 segment | Phase 01 |
| `EXT-X-PART` 标签生成 | gohlslib playlist 生成 | ⚠️ `LowLatencyState` 有模型未集成 | Phase 01 |
| `EXT-X-SERVER-CONTROL` 标签 | gohlslib `CAN-BLOCK-RELOAD=YES` | ⚠️ 有模型未集成 | Phase 01 |
| `EXT-X-PART-INF` 标签 | gohlslib `PART-TARGET` | ⚠️ 有模型未集成 | Phase 01 |
| `EXT-X-PRELOAD-HINT` 标签 | gohlslib 自动生成 | ❌ 未实现 | Phase 02 |
| Blocking Playlist Reload (`_HLS_msn`/`_HLS_part`) | gohlslib.Handle() 内部 long-poll | ❌ 未实现 | Phase 02 |
| Delta Updates (`_HLS_skip=YES`, `EXT-X-SKIP`) | gohlslib.Handle() 内部 | ❌ 未实现 | Phase 02 |
| Part 独立 HTTP 端点服务 | gohlslib segment/part 路由 | ❌ 未实现 | Phase 02 |
| fMP4 Part 级别封装（独立 moof+mdat） | gohlslib 内部 | ❌ 仅完整 segment 粒度 | Phase 01 |
| LLHLS 配置项（part_target_ms 等） | mediamtx `hlsPartDuration` | ❌ 未实现 | Phase 01 |
| CDN 兼容模式（Bearer token + no-cache 策略） | mediamtx `hlsCDNSecret` | ❌ 未实现 | Phase 03 |
| `.mp` 后缀兼容（CDN mp4 特殊处理） | mediamtx http_server.go | ❌ 未实现 | Phase 03 |
| AlwaysRemux 模式（path ready 自动创建 muxer） | mediamtx `hlsAlwaysRemux` | ⚠️ 有 `hls_demand` 但语义不同 | Phase 03 |
| Session Cookie + Query Param 双模式 | mediamtx session secret | ⚠️ 仅 Cookie | Phase 03 |
| iOS UA 检测与兼容 | mediamtx iOS cookie 强制 | ❌ 未实现 | Phase 03 |
| `EXT-X-PROGRAM-DATE-TIME` | gohlslib NTP 时间 | ❌ 未实现 | Phase 04 |
| `EXT-X-RENDITION-REPORT` | LLHLS 规范 | ❌ 未实现 | Phase 04 |
| HTTP/2 支持（LLHLS 推荐） | Apple 规范推荐 | ❌ 仅 HTTP/1.1 | Phase 04 |
| Muxer 实例崩溃自动重建 | mediamtx 10s 重建间隔 | ❌ 未实现 | Phase 05 |
| 内嵌 hls.js 播放页面 | mediamtx index.html | ❌ 未实现 | Phase 05 |
| LLHLS 播放器兼容性测试 | hls.js / Safari / VLC | ❌ 未验证 | Phase 05 |

---

## 已有能力（无需重复实现）

| 能力 | 状态 |
|------|------|
| fMP4 muxer (init + moof/mdat, 多编码) | ✅ |
| TS muxer (H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1) | ✅ |
| 内存 segment 环形缓冲 (SegmentRing) | ✅ |
| M3U8 playlist 生成 (master + media) | ✅ |
| HTTP/1.1 Keep-Alive + 分片发送 | ✅ |
| Cookie 会话追踪 | ✅ |
| 按需生成 (hls_demand) | ✅ |
| CORS 头 | ✅ |
| ETag 缓存控制 | ✅ |
| HTTPS/TLS | ✅ |
| 磁盘切片模式 | ✅ |
| HLS 代理拉流 | ✅ |
| 安全防护（路径遍历、长度限制） | ✅ |
| `LowLatencyState` 数据模型（未集成） | ⚠️ |
| `HlsPart` 结构体 | ⚠️ |

---

## 总体约束

1. 严格遵循 `core + driver + module` 三段式架构
2. LLHLS 状态机逻辑（part 切片判定、playlist 生成、blocking 等待）在 core 层实现，不依赖 runtime
3. Long-poll / HTTP 连接挂起在 driver 层实现
4. Part 生命周期管理、CDN 模式、会话策略在 module 层编排
5. fMP4 part 封装复用现有 `Fmp4Muxer`，扩展 part 级别输出接口
6. LLHLS 默认启用（variant=lowLatency），可通过配置回退到传统 HLS
7. 非标兼容逻辑集中在 driver 层或 module 层的 compat 处理中
8. 所有 LLHLS 标签生成必须通过属性测试验证格式正确性
9. Blocking request 必须有超时上界，防止连接泄漏
10. Part 缓冲必须有容量上界，慢客户端不拖累其他订阅者

---

## 参考来源

| 来源 | 路径 |
|------|------|
| mediamtx HLS Server | `vendor-ref/mediamtx/internal/servers/hls/server.go` |
| mediamtx HLS Muxer | `vendor-ref/mediamtx/internal/servers/hls/muxer.go` |
| mediamtx HLS Muxer Instance | `vendor-ref/mediamtx/internal/servers/hls/muxer_instance.go` |
| mediamtx HLS HTTP Server | `vendor-ref/mediamtx/internal/servers/hls/http_server.go` |
| mediamtx HLS Session | `vendor-ref/mediamtx/internal/servers/hls/session.go` |
| mediamtx HLS from_stream | `vendor-ref/mediamtx/internal/protocols/hls/from_stream.go` |
| mediamtx HLS 配置 | `vendor-ref/mediamtx/mediamtx.yml` (hls* 配置段) |
| gohlslib 库 | `github.com/bluenviron/gohlslib/v2 v2.3.2` |
| Apple LLHLS 规范 | RFC 8216bis / Apple HLS Authoring Spec |
| 本项目 HLS 实现 | `crates/protocols/hls/` |
| 本项目前序计划 | `dev-docs/plans-20-hls-abl/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [phase-01-part-muxing.md](phase-01-part-muxing.md) | ✅ 已完成 | Part 级别 fMP4 切片、LLHLS Playlist 标签集成、配置项 |
| [phase-02-blocking-delivery.md](phase-02-blocking-delivery.md) | ✅ 已完成 | Blocking Playlist Reload、Delta Updates、Preload Hint、Part 端点 |
| [phase-03-cdn-compat.md](phase-03-cdn-compat.md) | ✅ 已完成 | CDN 兼容模式、Session 双模式、iOS 兼容、非标特性 |
| [phase-04-advanced-features.md](phase-04-advanced-features.md) | ✅ 已完成 | PROGRAM-DATE-TIME、Rendition Report、HTTP/2 |
| [phase-05-production-hardening.md](phase-05-production-hardening.md) | ✅ 已完成 | 崩溃恢复、内嵌播放页、播放器兼容性测试、性能优化 |

---

## 渐进式执行顺序

1. **Phase 01** — Part 级别切片：fMP4 part 封装、part 切片判定、LLHLS playlist 标签集成、配置项新增
2. **Phase 02** — 阻塞式交付：Blocking Playlist Reload (long-poll)、Delta Updates、Preload Hint、Part HTTP 端点
3. **Phase 03** — CDN 与兼容性：CDN Bearer token 模式、Cache-Control 策略、Session 双模式、iOS 兼容、`.mp` 后缀
4. **Phase 04** — 高级特性：PROGRAM-DATE-TIME 绝对时间、Rendition Report、HTTP/2 支持
5. **Phase 05** — 生产加固：Muxer 崩溃恢复、内嵌 hls.js 播放页、播放器兼容性矩阵、性能基准测试
