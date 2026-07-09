# RTMP 协议完善设计与开发计划总索引

- 状态：已完成
- 目标：基于现有 RTMP 实现，参考 simple-media-server 工程实践，补齐 RTMPS 传输、完整编解码支持、服务端转发、协议兼容性和鉴权管理能力，使 cheetah RTMP 模块达到生产可用水平。
- 方法：逐阶段补齐能力，每阶段独立可验证，严格遵守 core + driver + module 三段式架构。
- 完成判定：所有阶段任务完成，通过互操作测试（OBS/FFmpeg/VLC/SRS），RTMPS 可用，relay 可用，编解码覆盖目标列表。

## 当前仓库事实

- `cheetah-rtmp-core`：完整 Sans-I/O 状态机，含握手（Simple 模式）、Chunk 协议（4 种头格式 + 扩展时间戳）、AMF0 + AMF3 全类型编解码、Enhanced RTMP（H.265/H.266/AV1/VP9/Opus FourCC）、消息层、URL 解析、FLV 封装/解封装。
- `cheetah-rtmp-driver-tokio`：TCP 服务端驱动（监听 + 连接管理 + 读写循环 + 背压）、TCP 客户端驱动（推流/拉流模式 + 重连基础设施）。
- `cheetah-rtmp-module`：完整模块生命周期、Ingest 管线（时间戳修复 + 参数集提取 + 编解码检测）、Egress 管线（帧转 RTMP + bootstrap + 静音注入）、Pull/Push 后台任务（指数退避重试）、会话管理（单发布者语义）。
- 测试：16 个属性测试文件、14 个 fuzz 目标、集成测试、C-API 和 WASM 绑定。
- 缺失：RTMPS 传输层、Relay 转发、VP8 编解码路径、Aggregate 消息拆分、FCPublish/FCUnpublish 处理、鉴权钩子、共享对象、带宽检测。

## 与 simple-media-server 对比后的主要缺口

| 能力 | simple-media-server 参考点 | 本地状态 | 计划处理 |
|------|---------------------------|---------|---------|
| RTMPS (TLS) | 未实现 | URL 解析支持但无传输层 | Phase 01 实现 |
| VP8 编解码 | `RtmpDecodeVPX` / `RtmpEncodeVPX` | 未实现 | Phase 02 补齐 |
| G711/MP3/Opus 完整路径 | `RtmpDecodeCommon` / `RtmpEncodeCommon` | 枚举定义但路径不完整 | Phase 02 补齐 |
| Relay 转发 | `MediaClient` 抽象 + REST API | 仅 Pull/Push，无 Relay | Phase 03 实现 |
| FCPublish/FCUnpublish | 等同 publish 处理（OBS 兼容） | 忽略 | Phase 04 处理 |
| releaseStream | no-op 响应（FFmpeg 兼容） | 忽略 | Phase 04 处理 |
| Aggregate 消息 | 类型定义但未实现 | 类型定义但未实现 | Phase 04 实现拆分 |
| 鉴权钩子 | HookManager（publish/play auth） | 无条件接受 | Phase 05 实现 |
| REST 管理 API | 完整 CRUD（server/play/publish） | 无 | Phase 05 实现 |
| 带宽检测 | 无 | 无 | Phase 04 基础实现 |
| 共享对象 | 未实现 | 类型定义但无逻辑 | 不实现（低优先级） |
| RTMPT (HTTP 隧道) | 未实现 | 未实现 | 不实现（已过时） |
| 复杂握手 (HMAC-SHA256) | 未实现 | 未实现 | Phase 04 可选实现 |

## 总体约束

- 严格遵守 `core + driver + module` 三段式，Sans-I/O 硬约束不可破坏。
- 编解码逻辑统一收敛到 `cheetah-codec`，RTMP 层只做封装/解封装映射。
- TLS 实现使用 `rustls`，不引入 OpenSSL 系统依赖。
- 新增能力必须有对应测试（单元测试 / 属性测试 / 集成测试）。
- 兼容性修复集中管理，显式命名，不散落在热路径中。
- 热路径禁止阻塞，所有队列有上界。
- 每阶段完成后必须通过 `cargo fmt` + `cargo clippy` + `cargo test`。

## 参考来源

- `vendor-ref/simple-media-server/Src/Rtmp/` — C++ 参考实现
- Adobe RTMP Specification 1.0
- Enhanced RTMP Specification (Veovera)
- RFC 8216 (HLS，用于理解编解码兼容性)
- `dev-docs/plans-8/` — RTSP 计划文档结构参考

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [rtmp-architecture.md](rtmp-architecture.md) | 已完成 | 整体架构设计、crate 边界、数据流模型 |
| [rtmp-phase-01-rtmps-transport.md](rtmp-phase-01-rtmps-transport.md) | 已完成 | RTMPS (TLS) 传输层实现 |
| [rtmp-phase-02-codec-completeness.md](rtmp-phase-02-codec-completeness.md) | 已完成 | 编解码路径补齐（VP8/G711/MP3/Opus） |
| [rtmp-phase-03-relay-and-forwarding.md](rtmp-phase-03-relay-and-forwarding.md) | 已完成 | Relay 转发、Push/Pull 增强 |
| [rtmp-phase-04-protocol-compat.md](rtmp-phase-04-protocol-compat.md) | 已完成 | 协议兼容性、鲁棒性、非标准处理 |
| [rtmp-phase-05-auth-and-api.md](rtmp-phase-05-auth-and-api.md) | 已完成 | 鉴权钩子、REST 管理 API |

## 任务状态总表

| 阶段 | 任务 | 状态 | 计划文件 |
|------|------|------|---------|
| A | A.1 RTMPS 架构设计 | 已完成 | rtmp-architecture.md |
| A | A.2 编解码分层设计 | 已完成 | rtmp-architecture.md |
| A | A.3 Relay 数据流设计 | 已完成 | rtmp-architecture.md |
| 1 | 1.1 rustls 集成到 driver | 已完成 | rtmp-phase-01 |
| 1 | 1.2 TLS 服务端 acceptor | 已完成 | rtmp-phase-01 |
| 1 | 1.3 TLS 客户端 connector | 已完成 | rtmp-phase-01 |
| 1 | 1.4 配置模型扩展 | 已完成 | rtmp-phase-01 |
| 1 | 1.5 RTMPS 集成测试 | 已完成 | rtmp-phase-01 |
| 2 | 2.1 VP8 编解码路径 | 已完成（已有实现） | rtmp-phase-02 |
| 2 | 2.2 G711 完整路径 | 已完成（已有实现） | rtmp-phase-02 |
| 2 | 2.3 MP3 完整路径 | 已完成（已有实现） | rtmp-phase-02 |
| 2 | 2.4 Opus 完整路径验证 | 已完成（已有实现） | rtmp-phase-02 |
| 2 | 2.5 编解码能力协商 | 已完成（capabilities=255） | rtmp-phase-02 |
| 2 | 2.6 不支持编解码的透传 | 已完成 | rtmp-phase-02 |
| 3 | 3.1 Relay 任务模型 | 已完成 | rtmp-phase-03 |
| 3 | 3.2 Relay 驱动实现 | 已完成 | rtmp-phase-03 |
| 3 | 3.3 Pull/Push 增强 | 已完成（RTMPS 支持） | rtmp-phase-03 |
| 3 | 3.4 跨协议转发基础 | 已完成（StreamManager 天然支持） | rtmp-phase-03 |
| 4 | 4.1 FCPublish/FCUnpublish 处理 | 已完成 | rtmp-phase-04 |
| 4 | 4.2 releaseStream 兼容 | 已完成 | rtmp-phase-04 |
| 4 | 4.3 Aggregate 消息拆分 | 已完成 | rtmp-phase-04 |
| 4 | 4.4 复杂握手支持 | 已完成（可选 feature） | rtmp-phase-04 |
| 4 | 4.5 厂商 Quirks 集中管理 | 已完成 | rtmp-phase-04 |
| 4 | 4.6 带宽检测基础 | 已完成 | rtmp-phase-04 |
| 5 | 5.1 鉴权钩子框架 | 已完成（本地 Token） | rtmp-phase-05 |
| 5 | 5.2 Publish/Play 鉴权 | 已完成 | rtmp-phase-05 |
| 5 | 5.3 REST 管理 API | 已完成 | rtmp-phase-05 |
| 5 | 5.4 统计与监控 | 已完成（基础） | rtmp-phase-05 |

## 渐进式执行顺序

1. **Phase 01 — RTMPS 传输层**：最基础的安全能力，不影响现有功能，可独立验证。
2. **Phase 02 — 编解码补齐**：扩展媒体能力，依赖 `cheetah-codec` 层，与传输层正交。
3. **Phase 03 — Relay 转发**：依赖 Phase 01（RTMPS relay 场景）和 Phase 02（多编码转发）。
4. **Phase 04 — 协议兼容性**：可与 Phase 01-03 并行，但建议在基础能力稳定后集中处理。
5. **Phase 05 — 鉴权与 API**：最后实现，依赖所有前置能力稳定。

Phase 01 和 Phase 02 可并行开发。Phase 04 中的 FCPublish/releaseStream 兼容性修复优先级高，可提前到 Phase 02 之后立即处理。

## 阶段完成后的统一检查

```bash
# 每阶段完成后执行
cargo fmt
cargo clippy -p cheetah-rtmp-core
cargo clippy -p cheetah-rtmp-driver-tokio
cargo clippy -p cheetah-rtmp-module
cargo test -p cheetah-rtmp-core
cargo test -p cheetah-rtmp-driver-tokio
cargo test -p cheetah-rtmp-module
cargo test -p cheetah-rtmp-property-tests
```

```bash
# 涉及 cheetah-codec 改动时追加
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
```

```bash
# 互操作验证（手动）
# 1. OBS 推流到 rtmp://localhost:1935/live/test
# 2. FFmpeg 推流到 rtmps://localhost:1936/live/test
# 3. FFplay 拉流验证
# 4. VLC 拉流验证
```
