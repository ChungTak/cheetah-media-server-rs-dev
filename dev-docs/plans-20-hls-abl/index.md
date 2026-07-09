# HLS 协议增强计划（对标 ABLMediaServer 工程实践）

- **状态**: 已完成
- **前置**: `dev-docs/plans-19-hls-zlm`（对标 ZLMediaKit 增强计划）
- **目标**: 参考 ABLMediaServer 的 HLS 工程实践，补齐生产级能力：HTTP Keep-Alive 长连接、大块数据分片发送、VLC/ffplay 兼容性、磁盘切片模式、HLS 代理拉流完整实现、录像回放 HLS、Webhook 事件通知、安全防护
- **方法**: 逐项对比 `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer` 中 NetServerHLS + NetClientRecvHttpHLS + MediaStreamSource 实现，补齐缺口并增加非标兼容
- **完成标准**: 所有阶段通过 `cargo fmt` + `cargo clippy` + `cargo test`，端到端验证通过

---

## 与 ABLMediaServer 对比后的增强缺口

| 能力 | ABLMediaServer 参考 | 本地状态 | 计划处理 |
|------|---------------------|----------|----------|
| HTTP Keep-Alive 长连接 | `Connection: keep-alive` + 不主动关闭 | ✅ 已实现 | Phase 01 |
| 大块数据分片发送 (128KB chunks) | `Send_TsFile_MaxPacketCount = 1024*128` | ✅ 已实现 | Phase 01 |
| VLC/ffplay 兼容（不主动断开） | 注释明确说明不能主动断开 | ✅ 已实现 | Phase 01 |
| HEAD 请求支持 | 返回 200 + Content-Length:0 | ✅ 已实现 | Phase 01 |
| 异步发送优化 | `nSyncWritePacket=0` 异步写 | ✅ 已实现（写入超时背压） | Phase 01 |
| 磁盘切片模式 (hlsCutType=1) | TS/fMP4 写入磁盘 + m3u8 文件 | ✅ 已实现 | Phase 02 |
| 磁盘切片自动清理 | 超过 `nMaxTsFileCount` 自动删除旧文件 | ✅ 已实现 | Phase 02 |
| 磁盘切片目录管理 | `{wwwPath}/{app}/{stream}/` 目录结构 | ✅ 已实现 | Phase 02 |
| HLS 代理拉流 HTTP 客户端 | `CNetClientRecvHttpHLS` 完整实现 | ✅ 已实现 | Phase 03 |
| HLS 代理拉流 m3u8 解析 | 行解析 + 序号去重 + 历史列表 | ✅ 已实现 | Phase 03 |
| HLS 代理拉流 TS demux | `ts_demuxer_create` + 回调分发 | ✅ 已实现 | Phase 03 |
| HLS 代理拉流流控 | 2s m3u8 间隔 + 6s TS 超时 + 20ms 视频节流 | ✅ 已实现 | Phase 03 |
| 录像回放 HLS 播放 | `SendRecordHLS()` 磁盘文件直读 | ⚠️ 通过引擎回放流+磁盘回退 | Phase 04 |
| Webhook 事件通知 (on_play) | 首次 m3u8 请求触发 on_play | ✅ 已实现 | Phase 04 |
| Webhook 事件通知 (on_record_ts) | 每个切片完成触发 | ✅ 已实现（on_segment） | Phase 04 |
| Webhook 事件通知 (on_stream_none_reader) | 超时无人观看触发 | ✅ 已实现 | Phase 04 |
| Cookie 会话追踪 (2分钟过期) | `AB_COOKIE` + expires 2min | ✅ 已实现（Max-Age=120） | Phase 04 |
| 安全检查：请求长度限制 | `> 4096` 字节立即断开 | ✅ 已实现 | Phase 05 |
| 安全检查：非法字符检测 | 含 `%` 字符立即断开 | ✅ 已实现 | Phase 05 |
| 安全检查：仅允许 GET/HEAD | 其他方法拒绝 | ✅ 已实现（core层405） | Phase 05 |
| HTTPS/TLS HLS 服务 | 奇数端口自动加载 SSL | ✅ 已实现（tokio-rustls） | Phase 05 |
| HLS 播放对象统计 (getOutList) | 统计 m3u8 发送对象 | ✅ 已实现（stats模块） | Phase 05 |
| 内存动态扩展 | segment buffer 不够时 +2MB 扩展 | N/A（Rust Vec 自动扩展） | Phase 01 |
| malloc_trim 内存回收 | 连接销毁时归还内存 | N/A（Rust 无需） | N/A |

---

## 已有能力（无需重复实现）

| 能力 | 状态 |
|------|------|
| TS muxer (H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1) | ✅ |
| fMP4 muxer (init + moof/mdat) | ✅ |
| 内存 segment 环形缓冲 (SegmentRing) | ✅ |
| M3U8 playlist 生成 (master + media) | ✅ |
| M3U8 解析器 (master + media) | ✅ |
| TS demux 基础 (PAT/PMT/PES) | ✅ |
| fMP4 demux 基础 | ✅ |
| Cookie 会话追踪基础 | ✅ |
| 按需生成 (hls_demand) | ✅ |
| 快速注册 (fast_register) | ✅ |
| 参数集补发 (SPS/PPS/VPS per segment) | ✅ |
| CORS 头 | ✅ |
| 多编码格式支持 | ✅ |
| Session 超时清理 | ✅ |
| ETag 缓存控制 | ✅ |
| H265 自动选择 fMP4 容器 | ✅ |

---

## 总体约束

1. 严格遵循 `core + driver + module` 三段式架构
2. HTTP Keep-Alive 和分片发送在 driver 层实现
3. 磁盘切片在 module 层编排，driver 层执行 I/O
4. HLS 代理拉流 HTTP 客户端在 driver 层，协议逻辑在 core 层
5. Webhook 事件通过 `EngineContext` 发布，不直接依赖 HTTP 框架
6. 安全检查在 driver 层 HTTP 解析阶段执行，拒绝非法请求
7. 录像回放 HLS 通过 module 层与录像系统交互
8. 所有新增能力默认禁用，通过配置开启
9. 兼容性优化（VLC/ffplay）作为默认行为，不需要配置开关
10. 非标兼容逻辑集中在 driver 层的 HTTP 处理中

---

## 参考来源

| 来源 | 路径 |
|------|------|
| ABL HLS 服务端 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/NetServerHLS.cpp` |
| ABL HLS 服务端头文件 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/NetServerHLS.h` |
| ABL HLS 代理拉流 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/NetClientRecvHttpHLS.cpp` |
| ABL HLS 代理拉流头文件 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/NetClientRecvHttpHLS.h` |
| ABL 媒体源 (HLS 切片逻辑) | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/MediaStreamSource.cpp` |
| ABL 媒体源头文件 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/MediaStreamSource.h` |
| ABL 配置定义 | `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer/stdafx.h` |
| ABL 版本信息 | `vendor-ref/ABLMediaServer-src-2026-05-09/版本信息.txt` |
| 本项目 HLS 实现 | `crates/protocols/hls/` |
| 本项目前序计划 | `dev-docs/plans-19-hls-zlm/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [phase-01-http-transport.md](phase-01-http-transport.md) | ✅ 已完成 | HTTP Keep-Alive、大块分片发送、VLC 兼容、HEAD 支持、背压控制 |
| [phase-02-disk-segment.md](phase-02-disk-segment.md) | ✅ 已完成 | 磁盘切片模式、目录管理、自动清理、混合模式 |
| [phase-03-hls-pull.md](phase-03-hls-pull.md) | ✅ 已完成 | HLS 代理拉流完整实现、m3u8 解析、TS demux、流控 |
| [phase-04-event-session.md](phase-04-event-session.md) | ✅ 已完成 | Webhook 事件、Cookie 过期策略 |
| [phase-05-security-production.md](phase-05-security-production.md) | ✅ 已完成 | 安全防护、HTTPS/TLS、播放统计 |

---

## 渐进式执行顺序

1. **Phase 01** — HTTP 传输优化：Keep-Alive 长连接、128KB 分片发送、VLC/ffplay 兼容、HEAD 请求、异步背压
2. **Phase 02** — 磁盘切片模式：文件写入、目录组织、自动清理、内存/磁盘混合
3. **Phase 03** — HLS 代理拉流：HTTP 客户端、m3u8 增量解析、TS/fMP4 demux、流控与重试
4. **Phase 04** — 事件与会话：录像回放 HLS、Webhook 事件通知、Cookie 过期与淘汰
5. **Phase 05** — 安全与生产：请求校验、HTTPS/TLS、播放统计、生产级加固
