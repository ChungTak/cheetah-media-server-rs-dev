# Cheetah Media Server 全面代码审查计划

> 本计划按“先架构正确性、再基础共享层、最后各协议模块”的顺序展开，目标是建立一套可执行、可验收、可复盘的审查流程。

---

## 1. 审查目标与总体原则

### 1.1 审查目标

1. **架构正确性**：确认仓库严格遵循六层架构与三段式协议拆分，无跨层依赖反向、无协议层职责越界。
2. **基础共享层正确性**：确认 `cheetah-codec`、`cheetah-sdk`、`cheetah-runtime-api`、`cheetah-engine`、`cheetah-config`、`cheetah-control` 等核心 crate 的接口、模型、生命周期实现正确。
3. **协议实现正确性**：逐个审查 RTMP、RTSP、HTTP-FLV、HLS、fMP4、TS、RTP、GB28181、SRT、MP4、WebRTC 等协议的 core / driver / module 实现。
4. **测试与CI质量**：确认每层的单元测试、属性测试、集成测试、互操作测试覆盖充分，CI 基线可执行。
5. **安全、性能与可维护性**：确认输入边界、内存上限、并发模型、热路径性能、代码风格符合 `AGENTS.md` 与 `SystemArchitecture.md` 的约束。

### 1.2 审查原则

- **自上而下、由抽象到具体**：先审查架构与公共层，再进入具体协议。
- **分层切片**：每个 crate 单独检查，重点关注跨 crate 边界。
- **文档驱动**：以 `SystemArchitecture.md`、`AGENTS.md`、`README.md` 为基准，发现实现与文档冲突时优先修正实现并同步文档。
- **风险优先**：规模大、协议复杂、涉及多协议桥接的模块优先审查。
- **可执行可验收**：每个阶段给出明确的审查对象、检查清单、输出物、通过标准。

---

## 2. 仓库规模与优先级

按协议目录统计代码量（含注释、测试，近似行数）：

| 协议/模块 | 文件数 | 行数 | 复杂度/风险等级 | 建议审查优先级 |
|---|---:|---:|---|---:|
| WebRTC | ~70 | ~77k | 极高 | P0 |
| RTSP | ~49 | ~40k | 高 | P0 |
| RTMP | ~47 | ~35k | 高 | P0 |
| HLS | ~29 | ~25k | 高 | P0 |
| SRT | ~19 | ~8.7k | 中 | P1 |
| HTTP-FLV | ~19 | ~8.4k | 中 | P1 |
| RTP | ~15 | ~8.2k | 高 | P1 |
| TS | ~11 | ~8.1k | 中 | P1 |
| fMP4 | ~15 | ~6.5k | 中 | P1 |
| GB28181 | ~12 | ~6.3k | 中 | P1 |
| MP4 | ~12 | ~4.7k | 中 | P2 |
| `cheetah-codec` | - | ~16.8k | 极高 | P0 |
| `cheetah-engine` | - | ~数k | 高 | P0 |
| `cheetah-sdk` / `cheetah-runtime-api` | - | ~数k | 高 | P0 |

> 建议按 P0 → P1 → P2 顺序执行，P0 模块必须全量深度审查，P1/P2 可先做关键路径审查再按需补全。

---

## 3. 第一阶段：架构与全局正确性

### 3.1 审查对象

- `Cargo.toml` 工作区结构
- `apps/cheetah-server/src/main.rs`
- `SystemArchitecture.md`
- `AGENTS.md`
- `README.md` 中配置与运行章节

### 3.2 审查要点

1. **六层架构与依赖方向**
   - 检查 `apps` → `system` → `sdk` / `runtime-api` → `protocol-module` → `protocol-driver` → `protocol-core` / `codec` 的依赖是否单向。
   - 使用 `cargo tree --invert` 抽查可疑反向依赖。
   - 检查 `cheetah-codec` 是否不依赖 engine、tokio、HTTP、数据库。
   - 检查 `cheetah-sdk` 是否不依赖具体协议模块。
   - 检查 `runtime-api` / `sdk` / `engine` / `*-module` 公共接口是否不暴露 `tokio::*` / `tokio_util::*` 类型。

2. **三段式协议拆分**
   - 每个协议是否具备 `core`、`driver-tokio`、`module` 三个 crate。
   - crate 命名是否遵循 `cheetah-<proto>-core` / `cheetah-<proto>-driver-tokio` / `cheetah-<proto>-module`。
   - 目录组织是否按 `crates/protocols/<proto>/{core, driver-tokio, module, testing, fuzz, bindings}` 分组。
   - 检查是否存在把 `core` 逻辑混入 `driver`/`module` 或把 `module` 编排逻辑下沉到 `core` 的情况。

3. **Sans-I/O 硬约束**
   - 每个 `protocol-core` 是否不依赖 Tokio、不持有 socket、不启动线程、不创建异步任务、`async fn` 不成为核心状态机接口。
   - 核心状态机是否通过显式的 `Input / Output / Event / Timer` 模型推进。
   - 是否避免在 `core` 中直接调用 `Instant::now()` 或访问系统时间。
   - 是否不在 `core` 中直接访问 `EngineContext`、`StreamManager`、`RoomService`、数据库。

4. **Feature 与模块启用**
   - `cheetah-server` 的 `Cargo.toml` feature 是否清晰映射到各模块 crate。
   - `main.rs` 中 `cfg(feature = ...)` 注册是否完整、是否遗漏模块工厂。
   - 默认 feature 为 `rtmp` 的合理性，以及 `--features` 组合说明是否与文档一致。

5. **配置与生命周期**
   - 配置加载顺序（默认值 → `CHEETAH_CONFIG` YAML → `M7S_` 环境变量）是否实现正确。
   - `ConfigEffect::{Immediate, NewSessionsOnly, ModuleRestartRequired, EngineRestartRequired}` 语义是否被 module 正确实现。
   - `ModuleManagerApi::restart_module / restart_modules` 是否只接受 `Running` 状态。
   - 模块生命周期 `create -> init -> start -> stop` 是否清晰。

### 3.3 输出物

- 架构依赖图（文本或 dot）
- 跨层违规列表
- 文档与实现不一致项
- 第一阶段审查结论

---

## 4. 第二阶段：基础共享层与系统层

### 4.1 `cheetah-codec` 媒体内核

**审查目标**：确认 `cheetah-codec` 是统一媒体基础层，不实现协议状态机，也不泄漏 FFmpeg 类型。

**审查要点**：

1. **媒体模型**
   - `AVFrame`、`FrameFlags`、`TrackInfo`、`CodecId`、`MediaKind` 等定义是否清晰。
   - `AVFrame.pts/dts/duration` 是否严格表示 canonical timeline。
   - `FrameFlags::KEY` 与 `FrameFlags::B_FRAME` 的使用是否符合 `SystemArchitecture.md` 规则。
   - `FrameFlags::DISCONTINUITY` 是否正确用于重连/切源/seek 边界。

2. **时间线统一**
   - `time.rs` 中 `TimestampNormalizer`、`DtsGenerator`、`WrapUnwrapper` 是否正确处理回绕、不连续、timebase 转换。
   - 三层时间线（source / canonical / egress）是否被显式区分。
   - B 帧 `pts < dts` 是否显式标记 `B_FRAME` 并在后续处理中正确排序。
   - 修复事件的分类（`source_repair_events` / `canonical_repair_events` / `egress_repair_events`）是否实现。

3. **封装与解封装**
   - `flv.rs`、`ts_demux.rs`、`ts_mux.rs`、`fmp4_mux.rs`、`fmp4_demux.rs`、`ps.rs`、`rtp.rs`、`mp4.rs` 是否独立、清晰。
   - 是否优先通过 `cheetah-codec` 导出目标封装视图，而不是在各协议中重复实现。
   - 参数集缓存（`ParameterSetCache`）与补发逻辑是否统一在 `video.rs`/`frame_view.rs` 中实现。
   - `AccessUnitAssembler` 是否正确处理边界和缺失。

4. **兼容层**
   - `compat.rs` 是否集中管理厂商兼容逻辑（如 RTMP 国内 codec id、FourCC 等）。
   - 是否避免把兼容分支打散到协议热路径。

5. **性能与安全边界**
   - 所有缓存、队列、重排窗口是否有上界。
   - `RtpReorderBuffer`、`PsDemuxer` 等缓冲区是否设置 `max_reassembly_bytes` 之类的硬上限。
   - 是否使用 `Arc<AVFrame>`、`Bytes` 原地处理，避免不必要的 clone / memcpy。

6. **测试**
   - 是否有 `media_kernel_matrix.rs`、`ts_codec_matrix.rs`、`parameter_set_cache_checklist.rs` 等回归测试。
   - 属性测试是否覆盖关键封装/解封装边界。

### 4.2 `cheetah-sdk` 模块契约

**审查目标**：确认 SDK 提供 runtime-neutral、框架无关的模块接入能力。

**审查要点**：

1. **模块接口**
   - `Module` trait 是否完整（生命周期、HTTP 服务、能力声明）。
   - `ModuleFactory` 是否允许无状态构造与引擎注册。
   - `ModuleHttpService` 是否不直接依赖 Axum。
   - `HttpRouteMount` / `HttpRouteDescriptor` 是否支持路径与方法的正确匹配。

2. **ID 与键模型**
   - `StreamKey`、`StreamId`、`ModuleId`、`SessionId`、`PublisherId`、`SubscriberId` 是否不可误用。
   - `StreamKey` 是否默认单发布者独占，是否有防止多发布者并写的机制。

3. **引擎交互抽象**
   - `EngineContext` 是否暴露正确的能力（发布、订阅、任务、HTTP 路由等）。
   - `StreamManagerApi`、`PublisherApi`、`SubscriberApi` 是否清晰。
   - `BootstrapPolicy`、`BackpressurePolicy` 是否在正确位置定义。

4. **事件总线**
   - `EventBus` / `EventSubscriber` 是否支持模块间事件通信。
   - 事件类型 `StreamEvent`、`ModuleEvent`、`SystemEvent` 是否覆盖主要生命周期。

5. **任务系统**
   - `TaskSystemApi` 是否支持任务的层级、状态、取消、结果。
   - `CancellationToken` 是否支持父子级联取消。

### 4.3 `cheetah-runtime-api` / `cheetah-runtime-tokio`

**审查目标**：确认 runtime 抽象不暴露 Tokio 类型，并满足多线程高性能目标。

**审查要点**：

1. **RuntimeApi 抽象**
   - `Runtime` trait 与 `RuntimeApi` trait 是否覆盖 spawn、timer、TCP/UDP、oneshot、取消等。
   - `AsyncTcpSocket`、`AsyncTcpListener`、`AsyncUdpSocket`、`AsyncTimer` 是否为 trait 对象友好。
   - `MonoTime` 是否作为统一时间源，避免各模块直接使用系统时间。

2. **Tokio 实现**
   - `cheetah-runtime-tokio` 是否正确封装 `tokio::net` / `tokio::time`。
   - 是否支持 `spawn`（Send）和 `spawn_local`（非 Send）双通道。
   - 取消机制是否与 `CancellationToken` 正确集成。

3. **边界检查**
   - 使用 `dev-scripts/check_runtime_boundaries.sh` 验证 `tokio` 类型未泄漏到非 driver 层。

### 4.4 `cheetah-engine` 引擎与编排

**审查目标**：确认引擎是模块生命周期、流管理、调度的中枢，不实现协议细节。

**审查要点**：

1. **模块管理**
   - `ModuleManager` 是否正确加载、初始化、启动、停止模块。
   - 配置变化时是否触发 `ModuleRestartRequired` 并重建模块。
   - 模块 HTTP 路由挂载与分发是否正确。

2. **流管理**
   - `StreamManager` / `Dispatcher` 是否实现单发布者多订阅者模型。
   - `Subscriber` queue 是否“慢订阅者不拖累其他订阅者”。
   - Bootstrap/GOP 缓存是否在 `bootstrap_max_frames` 限制内。
   - 发布者租约模型是否清晰，是否防止 module 侧多发布者并写。

3. **调度模式**
   - `DispatcherMode::{PerStream, Global}` 是否正确实现。
   - 热路径是否优先单线程分片与所有权局部化。

4. **辅助服务**
   - `LocalCluster`、`InMemoryDatabase`、`InMemoryServiceRegistry`、`RoomService`、`TaskSystem`、`LocalProxyManager`、`MetricsRegistry`、`HealthService` 是否职责清晰。
   - `ffmpeg` 集成是否被隔离在 `LocalFfmpegService` 中，不污染核心层。

### 4.5 `cheetah-config` 配置系统

**审查目标**：确认配置加载、校验、应用、回滚、环境变量覆盖正确。

**审查要点**：

1. 配置加载顺序是否严格：代码默认值 → `CHEETAH_CONFIG` YAML → `M7S_` 环境变量。
2. `ConfigSchema` 宏与注册机制是否支持 global 与 module 两级 schema。
3. 配置 patch 是否支持 `Immediate`、`NewSessionsOnly`、`ModuleRestartRequired`、`EngineRestartRequired` 四种效果。
4. 回滚 token 是否在线程安全与失败回滚时正确传递。
5. 环境变量 `M7S_` 双下划线层级解析是否健壮，类型转换是否正确。

### 4.6 `cheetah-control` 控制面

**审查目标**：确认 HTTP 控制面作为 REST API 入口，正确代理到模块与引擎。

**审查要点**：

1. `/healthz`、`/readyz`、`/metrics`、`/api/v1/modules`、`/api/v1/streams`、`/api/v1/tasks`、`/api/v1/services`、`/api/v1/config` 是否正确实现。
2. 模块 HTTP 路由挂载（`handle_module_http`）是否正确匹配路径、方法、前缀优先级。
3. 配置 patch 接口是否正确处理 `effect` 参数与回滚。
4. 请求体大小限制（`to_bytes` 限制 `8 * 1024 * 1024`）是否合理。
5. 是否依赖 `axum` 仅在此处，未渗透到 SDK/模块公共接口。

### 4.7 输出物

- 各 crate 的接口边界图
- 公共模型/时间线/配置/控制面问题清单
- 基础层审查结论

---

## 5. 第三阶段：协议模块逐个审查

### 5.1 通用协议审查清单

每个协议审查时均按以下子项展开：

| 审查维度 | 检查内容 |
|---|---|
| **crate 拆分** | core / driver-tokio / module 是否拆分；命名是否符合 `AGENTS.md`；目录组织是否按 `crates/protocols/<proto>/` 分组。 |
| **Sans-I/O** | `core` 是否不依赖 Tokio、socket、线程、async、系统时间、数据库、HTTP、EngineContext。 |
| **I/O 驱动** | `driver-tokio` 是否只负责 socket、framing、timer、spawn、channel、backpressure，不持有业务状态。 |
| **模块编排** | `module` 是否负责 engine 接入、鉴权、session、资源分配、路由、业务映射，不重复实现协议状态机。 |
| **媒体收敛** | ingress 是否统一收敛为 `AVFrame + TrackInfo`；egress 是否通过 `cheetah-codec` 导出。 |
| **时间线** | 是否区分 source / canonical / egress 三层时间线；时间戳回绕、B 帧、DTS 生成是否由 `cheetah-codec` 处理。 |
| **兼容互操作** | 是否对齐 simple-media-server / ZLMediaKit / SMS 等成熟实现；兼容逻辑是否集中、显式命名。 |
| **性能** | 热路径是否避免阻塞与竞争锁；是否使用 `Arc<AVFrame>` / `Bytes`；队列与缓存是否有上界。 |
| **测试** | core 是否有单元/属性/fuzz 测试；driver 是否有集成测试；module 是否有端到端/互操作测试。 |
| **安全** | 输入长度、缓冲区大小、帧大小是否有上限；鉴权是否正确；TLS 配置是否合理。 |

### 5.2 RTMP 模块

**重点**：RTMP 是默认启用协议，代码量大，涉及推流、拉流、静态 pull/push 任务，且是其他协议的常见来源。

**审查要点**：

1. **core**
   - `#![no_std]` 是否保持（`cheetah-rtmp-core`）。
   - `CoreInput::{Bytes, Timeout, Command}` / `CoreOutput::{Write, Event, SetTimer, CancelTimer}` 是否实现正确。
   - 是否已移除 `RtmpServerConnection` / `RtmpMessageChannel` 等 facade。
   - chunk / message / AMF0/AMF3 解析是否正确，是否能处理畸形/不完整的 chunk 边界。
   - 窗口确认大小、带宽限制、流控制实现是否正确。
   - 推流（publish）与拉流（play）的 FCPATH / stream key 解析是否健壮。

2. **driver-tokio**
   - `start_server` 与 `start_client` 是否正确封装 inbound/outbound。
   - TCP 连接/分帧/读写循环是否正确处理背压与取消。
   - pull/push 任务的 TCP 连接与重试是否健壮。

3. **module**
   - 静态 pull/push 任务的生命周期与重试逻辑。
   - 发布租约与订阅者队列管理。
   - `enable_add_mute`、`bootstrap_max_frames`、`emit_play_metadata` 等配置是否正确实现。
   - 与 `cheetah-codec` 的 `flv` 模块协作是否正确。

### 5.3 RTSP 模块

**重点**：RTSP 是当前最复杂协议之一，支持 OPTIONS/DESCRIBE/ANNOUNCE/SETUP/PLAY/PAUSE/RECORD/TEARDOWN/GET/SET_PARAMETER，以及 UDP/TCP/HTTP tunnel/multicast 多种传输。

**审查要点**：

1. **core**
   - 请求/响应解析是否健壮。
   - interleaved framing（`$` 通道）与 RTP/RTCP 包模型。
   - Transport/Session/Range/RTP-Info/SDP 解析。
   - 服务器端 publish ingest + play egress 状态机。
   -  outbound pull/push/relay 任务逻辑。

2. **driver-tokio**
   - TCP/UDP I/O、HTTP tunnel 连接配对、multicast endpoint 操作。
   - 会话超时、保活、TEARDOWN 清理。

3. **module**
   - 发布租约、会话生命周期、静态 pull/push/relay 任务监督。
   - RTP over UDP/TCP 到 `cheetah-codec` 的 `rtp`/`sdp` 模块衔接。
   - 时间戳是否从 RTP timestamp 经 `cheetah-codec` 归一化到 canonical timeline。

### 5.4 HTTP-FLV 模块

**重点**：HTTP 与 WebSocket 播发 FLV，通常作为 RTMP 推流的输出协议。

**审查要点**：

1. **core**：HTTP 路由、WebSocket upgrade、CORS、session 状态机、URL 解析（`/{app}/{stream}.flv`）。
2. **driver-tokio**：HTTP/1.1 chunked 响应、WebSocket 二进制帧、写队列、读缓冲区。
3. **module**：引擎订阅、播放会话生命周期、pull job 监督、配置校验、与 `flv.rs` 的集成。

### 5.5 HLS 模块

**重点**：包括普通 HLS 与 LL-HLS（fMP4 + part），文件数较多，切片、playlist、session 管理复杂。

**审查要点**：

1. **core**
   - playlist 生成（m3u8 master/media）、segment/part 切片、LLHLS 标签（`EXT-X-PART`、`EXT-X-PRELOAD-HINT`、`EXT-X-SERVER-CONTROL` 等）。
   - URL 路由（`/{app}/{stream}.m3u8`、`/{app}/{stream}/index.m3u8`、`/{app}/{stream}/{seg}.ts`、part、init）。
   - container 切换（TS vs fMP4）与 `ll_hls_enabled` 开关。
   - CDN 模式 `cdn_secret` 与 `Authorization` 头。

2. **driver-tokio**：HTTP 服务、磁盘 segment 或内存缓存、TLS 支持。

3. **module**
   - `StreamMuxer` 端到端集成。
   - session 标识（Cookie / URL 参数）。
   - 与 `ts_mux.rs` / `fmp4_mux.rs` 的集成。

### 5.6 fMP4 模块

**重点**：HTTP/WebSocket 实时 fMP4 播发，也支持远程 fMP4 pull。

**审查要点**：

1. **core**：HTTP 路由 `.mp4` / `.live.mp4`、WebSocket upgrade、CORS、session 状态机。
2. **driver-tokio**：chunked 响应、WebSocket 二进制帧、TLS。
3. **module**：
   - 初始化片段（`ftyp` + `moov`）与媒体片段（`moof` + `mdat`）的实时生成。
   - Bootstrap GOP 缓存。
   - `max_box_bytes`、`max_fragment_duration_ms`、`force_fragment_on_keyframe` 等配置。
   - 与 `fmp4_mux.rs` 的集成。

### 5.7 TS 模块

**重点**：MPEG-TS 复用与播发，以及 RTP-TS ingest 桥接。

**审查要点**：

1. **core**：HTTP 路由 `.ts` / `.live.ts`、CORS、WebSocket、session 状态机。
2. **driver-tokio**：HTTP/WS 传输、读缓冲、写队列。
3. **module**：
   - 188 字节 TS 包动态复用、PAT/PMT 周期性注入。
   - `pat_pmt_interval_ms`、`strict_crc`、`max_reassembly_bytes`。
   - `RtpTsIngest` 桥接与 `ts_demux.rs` 的衔接。

### 5.8 RTP 模块

**重点**：RTP 接收分发，支持多种 payload（MPEG-TS、PS、ES、Ehome、XHB、JT/T 1078）与 TCP/UDP 传输。

**审查要点**：

1. **core**
   - `RtpCore` 状态机、SSRC 到 stream key 映射、RTP/RTCP 包解析。
   - payload 探测（auto probe）是否覆盖 MPEG-TS、PS、Raw ES、Ehome。
   - jitter buffer 重排窗口与 `max_reassembly_bytes` 上限。

2. **driver-tokio**
   - UDP/TCP bind、读循环、TCP 2 字节/4 字节分帧自动检测、RTCP 端口。
   - 单端口多流、SSRC lock、上下文恢复。

3. **module**
   - stream key 映射（如 `/live/{ssrc}`）、鉴权、session 生命周期。
   - 与 `cheetah-codec` 的 `ps.rs`、`ts_demux.rs`、`rtp.rs`、`rtp_reorder.rs` 的衔接。

### 5.9 GB28181 模块

**重点**：国标 SIP 信令服务器，摄像头注册、心跳、Invite、Bye、语音对讲。

**审查要点**：

1. **core**
   - SIP 消息解析（REGISTER/MESSAGE/INVITE/ACK/BYE）。
   - 注册 MD5 挑战鉴权、Digest 参数解析。
   - 设备目录、心跳保活、Invite/Bye 会话状态。
   - SDP 生成与解析（`GbSdp`）。

2. **driver-tokio**
   - UDP/TCP SIP 消息循环、TCP 连接状态、离线检测定时器。
   - 与 RTP driver 协同分配媒体端口。

3. **module**
   - RESTful 接口（catalog、invite、bye、talkback）。
   - 设备在线/离线映射、SSRC 分配、发布租约。
   - 与 RTP 模块的联动。

### 5.10 SRT / MP4 / WebRTC 模块

**SRT**：
- 检查 `shiguredo_srt` 依赖的使用边界。
- 检查 SRT 协议 core 的封装与 `cheetah-sdk` 的集成。

**MP4（VOD/录制）**：
- 检查 `mp4` 协议 core 与 `cheetah-codec` 的 `mp4.rs` 读写器集成。
- 检查跨协议 VOD seek 与录制多格式支持。

**WebRTC**：
- 代码量最大（约 77k 行），建议作为单独深度审查子计划。
- 重点检查 SDP/ICE/DTLS/SRTP 状态机、与 `str0m` 的集成、信令与媒体通道分离、安全边界。

### 5.11 输出物

- 每个协议的审查报告
- 跨协议桥接与时间戳一致性问题清单
- 协议模块优先级风险矩阵

---

## 6. 第四阶段：测试、CI 与质量

### 6.1 测试策略审查

| 层次 | 审查内容 |
|---|---|
| `core` | 单元测试覆盖、属性测试（proptest）、fuzz 目标是否可构建、是否依赖真实网络。 |
| `driver-tokio` | 集成测试是否覆盖 I/O 错误、超时、取消、背压。 |
| `module` | 端到端测试是否覆盖 publish→subscribe、跨协议播放、静态任务。 |
| `cheetah-codec` | 矩阵测试是否覆盖多种 codec/container 组合；回归测试是否覆盖参数集补发。 |

### 6.2 CI 与脚本审查

- 检查 `dev-scripts/` 下脚本是否最新、是否可执行。
- 检查 `check_runtime_boundaries.sh`、`check_rtmp_core_no_std.sh` 等是否通过。
- 检查 `cargo fmt`、`cargo clippy`、测试命令是否覆盖每个协议。
- 检查 fuzz 是否使用 `cargo +nightly` 可构建。
- 检查是否避免默认使用 `--all-features` 作为最低门槛。

### 6.3 输出物

- 测试覆盖率与缺口清单
- CI 脚本问题清单
- 建议补充的回归测试

---

## 7. 第五阶段：安全、性能与文档

### 7.1 安全审查

- 所有输入解析（长度、大小、计数）是否有上界检查。
- 缓冲区、队列、重传窗口、jitter buffer 是否设置上限。
- 鉴权与 TLS 配置是否正确（`rustls` 默认 provider 安装、证书/私钥路径）。
- 控制面 HTTP API 是否暴露未授权操作。
- 环境变量、配置是否可能泄露敏感信息。

### 7.2 性能审查

- 热路径是否避免互斥锁和阻塞。
- 是否使用 `Bytes` 和 `Arc<AVFrame>` 减少拷贝。
- 订阅者队列是否独立，慢订阅者是否被隔离。
- 是否避免在热路径分配动态内存。
- 单线程分片与所有权局部化是否被利用。

### 7.3 文档同步审查

- 检查 `README.md` 与 `SystemArchitecture.md` 是否与实现一致。
- 检查新增配置项是否补充到 `config.example.yaml`。
- 检查 `AGENTS.md` 是否需要更新（命名、分层、约束）。
- 检查每个协议 README 是否准确（URL、配置、示例命令）。

### 7.4 输出物

- 安全与性能问题清单
- 文档同步建议
- 最终审查总报告

---

## 8. 审查执行方式与建议节奏

### 8.1 建议阶段

| 阶段 | 天数 | 关键输出 |
|---|---|---|
| 第 1 阶段：架构与全局 | 1-2 天 | 架构依赖图、跨层违规、文档不一致项 |
| 第 2 阶段：基础共享层 | 3-5 天 | 基础层审查报告、接口边界图 |
| 第 3 阶段：P0 协议审查 | 7-10 天 | RTMP / RTSP / HLS / WebRTC / codec 报告 |
| 第 4 阶段：P1 协议审查 | 5-7 天 | HTTP-FLV / TS / fMP4 / RTP / GB28181 报告 |
| 第 5 阶段：P2 与测试/CI | 2-3 天 | SRT / MP4 / 测试 / 安全 / 性能 / 文档报告 |
| 第 6 阶段：汇总与修复 | 3-5 天 | 问题优先级清单、修复计划、回归测试 |

### 8.2 建议工具

- 依赖分析：`cargo tree`、`cargo tree --invert`
- 静态检查：`cargo fmt`、`cargo clippy`、自定义 `dev-scripts/check_*.sh`
- 测试：`cargo test`、property tests、cargo-fuzz
- 代码度量：`wc -l`、`tokei`、循环/嵌套复杂度
- 文档：`mermaid` 或 `graphviz` 绘制架构图

### 8.3 验收标准

- 所有 P0 协议和 `cheetah-codec` 完成审查并输出报告。
- 跨层依赖违规全部清零或记录明确例外。
- `cargo clippy` 与 `cargo test` 在工作区成员中通过（含默认 feature 与关键 feature 组合）。
- 发现的问题按优先级（P0/P1/P2）分类，并给出修复 owner 与时间点。

---

## 9. 附录：协议审查速查表

```
RTMP:
  入口: crates/protocols/rtmp/{core, driver-tokio, module, testing/property-tests, fuzz, bindings/c-api, bindings/wasm}
  关键: no_std, chunk/amf, pull/push, publish/play, bootstrap

RTSP:
  入口: crates/protocols/rtsp/{core, driver-tokio, module, testing/property-tests, fuzz}
  关键: request/response, SDP/Transport, RTP/RTCP, UDP/TCP/tunnel/multicast, pull/push/relay

HTTP-FLV:
  入口: crates/protocols/http-flv/{core, driver-tokio, module, fuzz}
  关键: HTTP/WS route, FLV mux, session, pull jobs

HLS:
  入口: crates/protocols/hls/{core, driver-tokio, module, testing/property-tests, fuzz}
  关键: TS/fMP4 container, LLHLS part, playlist, session, CDN auth

fMP4:
  入口: crates/protocols/fmp4/{core, driver-tokio, module, testing/property-tests, fuzz}
  关键: init/fragment, WS/HTTP, TLS, bootstrap, max fragment

TS:
  入口: crates/protocols/ts/{core, driver-tokio, module}
  关键: 188-byte packet, PAT/PMT, RTP-TS ingest, CRC

RTP:
  入口: crates/protocols/rtp/{core, driver-tokio, module, testing/property-tests}
  关键: payload probe, jitter buffer, SSRC, UDP/TCP, RTCP

GB28181:
  入口: crates/protocols/gb28181/{core, driver-tokio, module, testing/property-tests}
  关键: SIP, REGISTER/INVITE/BYE, SDP, MD5 auth, RTP port

SRT:
  入口: crates/protocols/srt/{core, driver-tokio, module, testing/property-tests, fuzz}
  关键: shiguredo_srt, ingest/egress

MP4:
  入口: crates/protocols/mp4/{core, driver-tokio, module, testing/property-tests, fuzz}
  关键: VOD/record, seek, writer/reader

WebRTC:
  入口: crates/protocols/webrtc/{core, driver-tokio, module, testing/property-tests, fuzz}
  关键: str0m, SDP/ICE/DTLS/SRTP, signaling, 单独深度审查
```

---

## 10. 结论

本计划从架构正确性、基础共享层、各协议模块、测试 CI、安全性能与文档六个维度给出全面审查路线。执行时建议按阶段推进，P0 模块优先，确保基础层与主要协议（RTMP、RTSP、HLS、WebRTC、`cheetah-codec`）先达到可发布质量，再扩展到 P1/P2 协议。每个阶段结束时输出问题清单与修复计划，最终汇总为一份完整审查报告。
