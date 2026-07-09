# RTMP 协议完善设计与开发计划总索引

- **状态**: 进行中（Phase 01/03/04/05 已完成，Phase 02 未开始）
- **目标**: 对标 ZLMediaKit 工程实践，补齐 RTMP/FLV 生态缺失能力，提升协议兼容性与鲁棒性
- **方法**: 对比 `vendor-ref/ZLMediaKit/src/Rtmp/` 实现，逐项补齐缺口，遵循 `core + driver + module` 三段式架构
- **完成标准**: 所有阶段任务通过 `cargo fmt` + `cargo clippy` + `cargo test`，端到端互操作验证通过

---

## 当前仓库事实

### cheetah-rtmp-core

- Sans-I/O 状态机，`#![no_std]` 兼容
- 完整握手（简单 + 复杂 HMAC-SHA256）
- 完整 Chunk 协议（4 种头格式、扩展时间戳、动态 chunk size）
- 完整 AMF0/AMF3 编解码
- 完整命令处理（connect、createStream、publish、play、deleteStream）
- Enhanced RTMP（FourCC 视频/音频）
- FLV 封装/解封装（egress mux + ingest demux）
- 视频：H.264、H.265、H.266(VVC)、AV1
- 音频：AAC、Opus、MP3

### cheetah-rtmp-driver-tokio

- TCP 服务端 + RTMPS（rustls）
- TCP 客户端 + RTMPS 客户端
- 连接管理、背压、有界通道

### cheetah-rtmp-module

- 完整 Publish/Play 管线
- Pull/Push/Relay 后台任务（指数退避重试）
- Token 鉴权
- 静音音频注入、onMetaData 发送
- 热配置（ModuleRestartRequired）

### cheetah-http-flv（独立协议 crate）

- HTTP-FLV 播放（GET chunked）
- WebSocket-FLV 播放
- HTTP-FLV Pull 任务
- Enhanced RTMP 模式、fastPts 模式

---

## 与 ZLMediaKit 对比后的主要缺口

| 能力 | ZLMediaKit 参考点 | 本地状态 | 计划处理 |
|------|-------------------|----------|----------|
| G.711 A-law/μ-law 编解码 | `RtmpCodec` audio ID 7/8 | ❌ 未实现 | Phase 01 |
| VP8/VP9 编解码 | Enhanced FourCC `vp08`/`vp09` + 国内扩展 ID 14/15 | ❌ 未实现 | Phase 01 |
| 国内扩展 codec ID | H.265=12, AV1=13, VP8=14, VP9=15, Opus=13 | ❌ 未实现 | Phase 01 |
| FLV 文件录制 | `FlvRecorder` | ❌ 未实现 | Phase 02 |
| 断连续推 | `kContinuePushMS` 保活窗口 | ❌ 未实现 | Phase 03 |
| Paced Sender | `kPacedSenderMS` 平滑发送 | ❌ 未实现 | Phase 03 |
| 直接代理模式 | `kDirectProxy` 跳过 demux/remux | ❌ 未实现 | Phase 03 |
| RTMP 客户端 Seek/Pause/Speed | `RtmpPlayer` seek/pause/speed | ❌ 未实现 | Phase 04 |
| 聚合消息 type 22 | `RtmpProtocol` 解析 | ❌ 未实现 | Phase 04 |
| Multi-track Enhanced RTMP | Veovera 规范 multiTrack | ❌ 未实现 | Phase 04 |
| HTTP-FLV Push（POST 推流） | `HttpSession::onRecvUnlimitedContent` | ❌ 未实现 | Phase 05 |
| HTTPS-FLV（TLS） | HTTP server TLS | ❌ 未实现 | Phase 05 |
| WSS-FLV（TLS WebSocket） | WebSocket over TLS | ❌ 未实现 | Phase 05 |
| VOD Seek/Pause/Speed | `RtmpSession` onCmd_seek/pause/playCtrl | ❌ 未实现 | Phase 04 |

---

## 总体约束

1. 严格遵循 `core + driver + module` 三段式，`core` 保持 Sans-I/O、`#![no_std]`
2. 新增编解码能力统一收敛到 `cheetah-codec`，不在协议 crate 内私有实现
3. 国内扩展 codec ID 作为兼容层集中管理，通过 feature flag 或配置开关控制
4. 录制能力作为独立模块（`cheetah-record-module`），不嵌入 RTMP module
5. 所有新增能力必须补充单元测试 + 属性测试，涉及兼容性修复必须补回归测试
6. 热路径禁止阻塞，缓冲区必须有上界
7. 兼容优先：入口允许脏数据，内部规范化，出口稳定可预测

---

## 参考来源

| 来源 | 路径/链接 |
|------|-----------|
| ZLMediaKit RTMP 实现 | `vendor-ref/ZLMediaKit/src/Rtmp/` |
| ZLMediaKit Record 实现 | `vendor-ref/ZLMediaKit/src/Record/` |
| Enhanced RTMP 规范 | Veovera Enhanced RTMP v2 |
| Adobe RTMP 规范 | RTMP Specification 1.0 |
| FLV 规范 | Adobe FLV File Format Spec v10.1 |
| 本项目架构文档 | `SystemArchitecture.md`、`AGENTS.md` |
| 前序计划 | `dev-docs/plans-9/` |

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [rtmp-architecture.md](rtmp-architecture.md) | 未开始 | 架构扩展设计 |
| [rtmp-phase-01-codec-completeness.md](rtmp-phase-01-codec-completeness.md) | 已完成 | 编解码补全（G.711、VP8/VP9、国内扩展） |
| [rtmp-phase-02-recording.md](rtmp-phase-02-recording.md) | 未开始 | FLV 文件录制 |
| [rtmp-phase-03-robustness.md](rtmp-phase-03-robustness.md) | 已完成 | 断连续推、Paced Sender、直接代理 |
| [rtmp-phase-04-playback-control.md](rtmp-phase-04-playback-control.md) | 已完成 | Seek/Pause/Speed、聚合消息、Multi-track |
| [rtmp-phase-05-http-flv-enhancement.md](rtmp-phase-05-http-flv-enhancement.md) | 已完成 | HTTP-FLV Push、HTTPS-FLV、WSS-FLV |

---

## 任务状态总表

| 阶段 | 任务 | 状态 | 计划文件 |
|------|------|------|----------|
| A | 架构扩展设计 | 未开始 | rtmp-architecture.md |
| 1.1 | G.711 A-law/μ-law 支持 | 已完成 | rtmp-phase-01 |
| 1.2 | VP8 编解码支持 | 已完成 | rtmp-phase-01 |
| 1.3 | VP9 编解码支持 | 已完成 | rtmp-phase-01 |
| 1.4 | 国内扩展 codec ID 兼容层 | 已完成 | rtmp-phase-01 |
| 1.5 | 未知编码透传（转发不转协议） | 已完成 | rtmp-phase-01 |
| 2.1 | FLV 录制引擎设计 | 未开始 | rtmp-phase-02 |
| 2.2 | FLV 文件写入实现 | 未开始 | rtmp-phase-02 |
| 2.3 | 录制生命周期管理 | 未开始 | rtmp-phase-02 |
| 2.4 | 录制 API 与配置 | 未开始 | rtmp-phase-02 |
| 3.1 | 断连续推（发布保活窗口） | 已完成 | rtmp-phase-03 |
| 3.2 | Paced Sender 平滑发送 | 已完成 | rtmp-phase-03 |
| 3.3 | 直接代理模式 | 已完成 | rtmp-phase-03 |
| 4.1 | 服务端 Seek/Pause/Speed 命令 | 已完成 | rtmp-phase-04 |
| 4.2 | 客户端 Seek/Pause/Speed | 已完成 | rtmp-phase-04 |
| 4.3 | 聚合消息 type 22 解析 | 已完成 | rtmp-phase-04 |
| 4.4 | Multi-track Enhanced RTMP | 已完成 | rtmp-phase-04 |
| 5.1 | HTTP-FLV Push（POST 推流） | 已完成 | rtmp-phase-05 |
| 5.2 | HTTPS-FLV（TLS 加密） | 已完成 | rtmp-phase-05 |
| 5.3 | WSS-FLV（TLS WebSocket） | 已完成 | rtmp-phase-05 |

---

## 渐进式执行顺序

1. **Phase A — 架构扩展设计**：先确定新增能力的 crate 归属和接口边界
2. **Phase 01 — 编解码补全**：基础能力，后续阶段依赖完整的编解码支持
3. **Phase 02 — 录制**：独立模块，不阻塞其他阶段
4. **Phase 03 — 鲁棒性增强**：提升生产环境稳定性
5. **Phase 04 — 播放控制**：VOD 场景支持，依赖录制能力
6. **Phase 05 — HTTP-FLV 增强**：扩展 FLV 生态，依赖 Phase 01 编解码

---

## 阶段完成后的统一检查

```bash
# 每个阶段完成后执行
cargo fmt
cargo clippy -p cheetah-rtmp-core
cargo clippy -p cheetah-rtmp-driver-tokio
cargo clippy -p cheetah-rtmp-module
cargo clippy -p cheetah-http-flv-core
cargo clippy -p cheetah-http-flv-driver-tokio
cargo clippy -p cheetah-http-flv-module
cargo clippy -p cheetah-codec
cargo test -p cheetah-rtmp-core
cargo test -p cheetah-rtmp-driver-tokio
cargo test -p cheetah-rtmp-module
cargo test -p cheetah-rtmp-property-tests
cargo test -p cheetah-http-flv-core
cargo test -p cheetah-http-flv-driver-tokio
cargo test -p cheetah-http-flv-module
cargo test -p cheetah-codec
```
