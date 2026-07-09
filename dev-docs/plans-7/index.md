# HTTP-FLV 实现设计与开发计划总索引

- 状态：已完成
- 目标：新增 `cheetah-http-flv-core`、`cheetah-http-flv-driver-tokio`、`cheetah-http-flv-module`，支持 HTTP-FLV 与 WS-FLV 播放输出，并支持远端 HTTP-FLV / WS-FLV 拉流写入本地 engine stream。
- 方法：HTTP-FLV 作为独立协议三段式实现，driver 独立监听 HTTP 端口；FLV/RTMP tag payload 封装、metadata、sequence header、时间戳导出和 FLV demux 能力沉到 `cheetah-codec` / `cheetah-rtmp-core` 共享边界，避免 `http-flv-module` 直接依赖 `cheetah-rtmp-module` 或复制 RTMP module 私有逻辑。
- 完成判定：RTMP/RTSP 发布到 engine 的标准 H264/AAC、H265/AAC、audio-only 流可以通过 `http://host/{app}/{stream}.flv` 和 `ws://host/{app}/{stream}.flv` 播放；远端 HTTP-FLV/WS-FLV pull job 能发布到本地 stream 并被 RTMP/RTSP/HTTP-FLV 拉取；真实兼容 probe 与传输扰动输入无 panic、无 OOM、无无界缓存、module 可停止。

## 当前仓库事实

- `cheetah-codec/src/flv.rs` 已有 `FlvTagBody`、audio/video sequence header helper，但还缺完整 FLV header、`PreviousTagSize`、有界 demux 和完整 tag 流模型。
- `cheetah-rtmp-module/src/egress.rs` 已实现大量 RTMP/FLV payload 出站逻辑，包括 metadata、H264/H265/H266/AV1/VP8/VP9、AAC/MP3/G711/ADPCM/Opus、静音 AAC 和时间戳修正；这些能力当前是 module 私有函数，HTTP-FLV 不能直接依赖。
- 现有 `ModuleHttpService` 返回一次性 `Bytes`，`cheetah-control` fallback 会完整读取请求体并返回完整响应，不能承载 HTTP-FLV 长连接 streaming 或 WebSocket upgrade。
- SimpleMediaServer 的落地路径为 `/{app}/{stream}.flv`；HTTP GET 直接返回 `video/x-flv` 流，WebSocket 使用同一路径发送 FLV binary 数据。
- SimpleMediaServer 的 FLV mux 会在播放启动时发送 FLV header、metadata、video/audio sequence header；有视频时等待 keyframe 后再转发媒体，video-only 可补静音 AAC。
- SimpleMediaServer 的 HTTP-FLV client 支持 chunked body，FLV demux 允许 metadata 缺失，并将 FLV tag payload 当作 RTMP media payload 复用。

## 总体约束

- HTTP-FLV 必须遵守 `core + driver + module` 三段式，目录放在 `crates/protocols/http-flv/`。
- `core` 保持 Sans-I/O；不得依赖 Tokio、socket、HTTP 框架、engine 或业务状态。
- `driver-tokio` 负责 TCP/HTTP/WebSocket framing、chunked decode、timer、spawn、channel、backpressure；Tokio 类型不得泄漏到 `core` 或 module 公共接口。
- `module` 只负责 engine 接入、配置、订阅、发布租约、pull job 编排和权限预留，不重写协议状态机。
- HTTP-FLV module 不直接依赖 `cheetah-rtmp-module`；共享封装能力必须沉到 `cheetah-codec` 或 `cheetah-rtmp-core`。
- 所有 FLV 输出前优先通过共享 adapter 从 `AVFrame + TrackInfo` 导出；所有 FLV 输入进入 engine 前统一收敛为 `AVFrame + TrackInfo`。
- 缓存、tag demux remain buffer、subscriber queue、driver write queue、pull job 重试和 WebSocket frame size 都必须有上界。
- 首版不实现 TLS；`https` / `wss` 需要等 runtime-neutral TLS 抽象后再纳入。

## 计划文件清单

| 文件 | 状态 | 范围 |
| --- | --- | --- |
| `http-flv-architecture.md` | 已完成 | 总体架构、crate 边界、SimpleMediaServer 对齐点、HTTP/WS 路由和兼容策略 |
| `http-flv-phase-01-shared-flv-rtmp-adapters.md` | 已完成 | `cheetah-codec` / `cheetah-rtmp-core` 共享 FLV/RTMP adapter 抽取 |
| `http-flv-phase-02-core-driver-server.md` | 已完成 | HTTP-FLV core、Tokio driver、HTTP GET / WebSocket 播放输出 |
| `http-flv-phase-03-pull-client-and-module.md` | 已完成 | module 配置、远端 HTTP-FLV/WS-FLV pull job、engine publish 编排 |
| `http-flv-phase-04-interop-robustness-fuzz.md` | 已完成 | 互操作测试、真实样例、PBT/fuzz、故障输入和收口检查 |

## 任务状态总表

| 阶段 | 任务 | 状态 | 计划文件 |
| --- | --- | --- | --- |
| Architecture | A.1 固定协议三段式与 crate 边界 | 已完成 | `http-flv-architecture.md` |
| Architecture | A.2 固定 HTTP/WS 路由、响应和播放启动语义 | 已完成 | `http-flv-architecture.md` |
| Architecture | A.3 固定兼容策略和测试分层 | 已完成 | `http-flv-architecture.md` |
| Phase 01 | 1.1 扩展 `cheetah-codec::flv` 完整 tag 模型 | 已完成 | `http-flv-phase-01-shared-flv-rtmp-adapters.md` |
| Phase 01 | 1.2 抽取 RTMP/FLV egress adapter | 已完成 | `http-flv-phase-01-shared-flv-rtmp-adapters.md` |
| Phase 01 | 1.3 抽取 FLV ingest 到 AVFrame adapter | 已完成 | `http-flv-phase-01-shared-flv-rtmp-adapters.md` |
| Phase 02 | 2.1 新增 HTTP-FLV Sans-I/O core | 已完成 | `http-flv-phase-02-core-driver-server.md` |
| Phase 02 | 2.2 新增 Tokio HTTP/WS server driver | 已完成 | `http-flv-phase-02-core-driver-server.md` |
| Phase 02 | 2.3 实现 HTTP/WS 播放输出集成 | 已完成 | `http-flv-phase-02-core-driver-server.md` |
| Phase 03 | 3.1 新增 module 配置与生命周期 | 已完成 | `http-flv-phase-03-pull-client-and-module.md` |
| Phase 03 | 3.2 实现远端 HTTP-FLV pull client | 已完成 | `http-flv-phase-03-pull-client-and-module.md` |
| Phase 03 | 3.3 实现 WS-FLV pull client 和重试监督 | 已完成 | `http-flv-phase-03-pull-client-and-module.md` |
| Phase 04 | 4.1 建立互操作 fixture 和端到端测试 | 已完成 | `http-flv-phase-04-interop-robustness-fuzz.md` |
| Phase 04 | 4.2 建立 PBT/fuzz 传输扰动覆盖 | 已完成 | `http-flv-phase-04-interop-robustness-fuzz.md` |
| Phase 04 | 4.3 文档、feature、CI 和 smoke 收口 | 已完成 | `http-flv-phase-04-interop-robustness-fuzz.md` |

## 渐进式执行顺序

1. 先完成 Architecture，固定三段式边界、HTTP/WS 路由、SimpleMediaServer 对齐点和首版范围。
2. 再完成 Phase 01，把共享 FLV/RTMP adapter 沉到 foundation/core，先消除未来复制逻辑风险。
3. 再完成 Phase 02，落地 HTTP/WS 播放输出，让现有 engine stream 能通过 FLV 播放。
4. 再完成 Phase 03，落地远端 HTTP-FLV/WS-FLV pull job，把 FLV 输入发布到本地 engine。
5. 最后完成 Phase 04，用真实样例、PBT/fuzz 和端到端互操作测试验证鲁棒性，并同步 `SystemArchitecture.md` / README / feature 说明。

## 阶段完成后的统一检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec flv
cargo clippy -p cheetah-rtmp-core
cargo test -p cheetah-rtmp-core flv
cargo clippy -p cheetah-rtmp-module
cargo test -p cheetah-rtmp-module
cargo clippy -p cheetah-http-flv-core
cargo test -p cheetah-http-flv-core
cargo clippy -p cheetah-http-flv-driver-tokio
cargo test -p cheetah-http-flv-driver-tokio
cargo clippy -p cheetah-http-flv-module
cargo test -p cheetah-http-flv-module
```

新增 fuzz target 后还必须执行短跑 smoke：

```bash
cd crates/protocols/http-flv/fuzz
cargo +nightly fuzz build
cargo +nightly fuzz run fuzz_flv_demux -- -runs=128
cargo +nightly fuzz run fuzz_http_flv_transport -- -runs=128
cargo +nightly fuzz run fuzz_ws_flv_frames -- -runs=128
```
