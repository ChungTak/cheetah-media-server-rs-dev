# RTSP 协议完善设计与开发计划总索引

- 状态：已完成
- 目标：在现有 `cheetah-rtsp-core`、`cheetah-rtsp-driver-tokio`、`cheetah-rtsp-module` 基础上补齐 RTSP 互操作能力，支持 RTP over UDP、RTP over TCP interleaved、RTP over HTTP tunnel、RTP multicast 四类传输，并支持服务端推流、服务端拉流播放、远端拉流/推流转发。
- 方法：继续遵守 `core + driver + module` 三段式。RTSP core 只保留 Sans-I/O 解析、Transport/Session/Range/RTP-Info/HTTP tunnel 纯状态；driver 负责 TCP/UDP/HTTP tunnel/multicast socket、base64 tunnel、timer、channel、backpressure；module 负责 engine 接入、鉴权、发布租约、订阅播放、静态转发任务和兼容策略编排。
- 完成判定：FFmpeg/GStreamer/VLC/主流 IPC/NVR 与 `vendor-ref/simple-media-server` 常见流程可完成 RTSP 推流、播放、远端 pull、远端 push；四类 RTP 传输均有端到端测试；异常 SDP、乱序/丢包/截断/错误 Transport 输入无 panic、无 OOM、无无界缓存，module 可停止。

## 当前仓库事实

- 已有 RTSP 三段式 crate：`crates/protocols/rtsp/core`、`driver-tokio`、`module`，并已有 `testing/property-tests` 与独立 `fuzz`。
- `cheetah-rtsp-core` 已实现 RTSP request/response 编解码、interleaved frame、RTP/RTCP 包、Transport/Session/Range/RTP-Info/SDP 基础模型和连接限制。
- `cheetah-rtsp-driver-tokio` 已实现 RTSP TCP listener、连接读写队列和 direct TCP/interleaved 收发驱动。
- `cheetah-rtsp-module` 已覆盖 OPTIONS、DESCRIBE、ANNOUNCE、SETUP、PLAY、PAUSE、RECORD、TEARDOWN、GET_PARAMETER、SET_PARAMETER，支持 TCP interleaved 与 UDP unicast 的服务端 publish/play 路径。
- 现有 module 已能把 RTSP publish RTP 收敛为 `AVFrame + TrackInfo`，并能从 engine stream packetize 为 RTP 输出；已覆盖 H264/H265/H266/AV1/VP8/VP9/AAC/Opus/ADPCM/G711/MP3 的主要路径。
- 现有测试包含 RTSP capture replay、RTSP<->RTMP 桥接、UDP forwarding、play/pause/keepalive、多轨等回归，已有真实 `.rtspcap` fixture。

## 与 simple-media-server 对比后的主要缺口

| 能力 | simple-media-server 参考点 | 本地状态 | 计划处理 |
| --- | --- | --- | --- |
| RTSP server publish/play | `RtspConnection::{handleAnnounce,handleSetup,handlePlay,handleRecord}` | 已有主路径 | 保持并补兼容矩阵、超时、鉴权、Transport 选择策略 |
| RTSP client pull/push | `RtspClient` 状态机：OPTIONS -> DESCRIBE/ANNOUNCE -> SETUP -> PLAY/RECORD | 缺远端 client driver/job | Phase 04 新增 outbound client 与静态 pull/push jobs |
| RTP over UDP | `RtspRtpTransport(Transport_UDP)`、UDP 打洞、server_port/client_port | 已支持 unicast，但端口池/NAT/目的地址策略不完整 | Phase 02 标准化 UDP endpoint、端口池、打洞和测试 |
| RTP over TCP | `$` interleaved、`interleaved=x-y` | 已支持 | Phase 02 补多 Transport 候选选择、通道冲突和大包边界 |
| RTP over HTTP | SMS 有 `GET`/`POST` 入口但未实现；Axis/QuickTime 常用双连接 tunnel | 缺 | Phase 02 实现 GET/POST 双连接、`x-sessioncookie`、base64 POST、GET plaintext response/media |
| RTP multicast | SMS `Transport_MULTICAST` 枚举和 SETUP 分支预留 | 缺 | Phase 02 增加 runtime-neutral multicast socket API 与 PLAY multicast |
| 认证 | SMS 支持 Digest/Basic，publish 默认不鉴权 | 缺 server/client RTSP auth | Phase 01 增加 Basic/Digest 兼容，鉴权接入 module |
| RTP 排序/抖动 | SMS `RtpSort` 有界排序窗口 | 局部依赖 depacketizer 状态，缺通用 UDP reorder buffer | Phase 03 下沉到 `cheetah-codec` 或 RTSP media compat helper |
| SDP 兼容 | SMS 推断 payload type、替换 SDP IP、PS/huge/fastPts 变体 | 已有部分 SDP 兼容 | Phase 01/03 补 Content-Base、control URI、PS/MP2P、vendor quirks |
| 转发 | SMS MediaClient + MediaSource 可 pull/push/转发 | RTMP 已有 jobs，RTSP 缺 | Phase 04 参考 RTMP job 模型实现 RTSP pull/push/relay |

## 总体约束

- 不新增 RTSP crate；继续在现有 `core + driver-tokio + module` 中补齐能力。
- `cheetah-rtsp-core` 不依赖 Tokio、socket、HTTP 框架、engine、SDK 或系统时间。
- `cheetah-rtsp-driver-tokio` 可使用 Tokio，但公共入口继续通过 `cheetah-runtime-api` 注入；Tokio 类型不得泄漏到 module 公共接口。
- HTTP tunnel 是 RTSP 的传输承载，不是 HTTP-FLV；不得把 FLV/HTTP module 逻辑混进 RTSP module。
- RTP packetize/depacketize、参数集缓存、时间戳归一化、PS/PES 解析、RTP reorder 能力优先沉到 `cheetah-codec`，不要在 RTSP module 内复制多套媒体修正逻辑。
- 转发任务必须遵守单发布者独占语义；远端 pull 写入本地 stream 前必须 acquire publisher lease。
- 所有连接、UDP socket、multicast endpoint、HTTP tunnel registry、RTP reorder buffer、subscriber queue、转发 job 重试队列都必须有上界。
- 首版不实现 RTSPS/TLS；`rtsps://`、HTTPS tunnel 后续等 runtime-neutral TLS 抽象补齐后再纳入。

## 参考来源

- 本地参考：`vendor-ref/simple-media-server/Src/Rtsp/*`、`vendor-ref/simple-media-server/Src/Rtp/*`。
- 标准参考：RFC 2326 RTSP 1.0，RFC 3550 RTP/RTCP，RFC 7826 的 RTSP 2.0 Transport 与 multicast 示例可作为兼容参考。
- RTSP-over-HTTP 兼容参考：Axis/VAPIX 风格 `GET` + `POST` 双连接，`x-sessioncookie` 关联，POST body base64，GET connection 返回 plaintext RTSP response 和 `$` interleaved media。

## 计划文件清单

| 文件 | 状态 | 范围 |
| --- | --- | --- |
| `rtsp-architecture.md` | 已完成 | 总体架构、能力矩阵、传输模型、服务端/客户端/转发边界 |
| `rtsp-phase-01-control-plane-compat.md` | 已完成 | RTSP 控制面、Transport/Session/Auth/SDP 兼容模型 |
| `rtsp-phase-02-rtp-transport-matrix.md` | 已完成 | UDP、TCP interleaved、HTTP tunnel、multicast 四类 RTP 传输 |
| `rtsp-phase-03-server-publish-play.md` | 已完成 | 服务端 ANNOUNCE/RECORD 推流、DESCRIBE/PLAY 拉流播放和媒体鲁棒性 |
| `rtsp-phase-04-client-forwarding-jobs.md` | 已完成 | 远端 RTSP pull、远端 RTSP push、转发 job、重试监督 |
| `rtsp-phase-05-interop-robustness-fuzz.md` | 已完成 | 互操作 fixture、端到端矩阵、属性测试/fuzz、文档和 CI 收口 |

## 任务状态总表

| 阶段 | 任务 | 状态 | 计划文件 |
| --- | --- | --- | --- |
| Architecture | A.1 固定三段式边界和目标矩阵 | 已完成 | `rtsp-architecture.md` |
| Architecture | A.2 固定传输抽象、session lifecycle 和转发语义 | 已完成 | `rtsp-architecture.md` |
| Architecture | A.3 固定兼容策略、测试分层和不进入首版范围 | 已完成 | `rtsp-architecture.md` |
| Phase 01 | 1.1 统一 RTSP 控制面解析与响应模型 | 已完成 | `rtsp-phase-01-control-plane-compat.md` |
| Phase 01 | 1.2 增加 Basic/Digest auth 和 hook 接入点 | 已完成 | `rtsp-phase-01-control-plane-compat.md` |
| Phase 01 | 1.3 强化 SDP 与 Transport 兼容解析 | 已完成 | `rtsp-phase-01-control-plane-compat.md` |
| Phase 02 | 2.1 标准化 UDP unicast endpoint 和端口池 | 已完成 | `rtsp-phase-02-rtp-transport-matrix.md` |
| Phase 02 | 2.2 强化 TCP interleaved 和大包/通道边界 | 已完成 | `rtsp-phase-02-rtp-transport-matrix.md` |
| Phase 02 | 2.3 实现 RTSP-over-HTTP tunnel | 已完成 | `rtsp-phase-02-rtp-transport-matrix.md` |
| Phase 02 | 2.4 实现 RTP multicast PLAY | 已完成 | `rtsp-phase-02-rtp-transport-matrix.md` |
| Phase 03 | 3.1 完善服务端 publish ingest | 已完成 | `rtsp-phase-03-server-publish-play.md` |
| Phase 03 | 3.2 完善服务端 play egress | 已完成（6/6：DESCRIBE 可配置等待源上线、PLAY RTP-Info 真实 seq/rtptime、egress MTU 与 HTTP tunnel interleaved 约束、selected tracks 独立 keyframe gate、RTCP SR/SDES/BYE helper 统一发送、UDP/TCP/HTTP/multicast PLAY 矩阵测试） | `rtsp-phase-03-server-publish-play.md` |
| Phase 03 | 3.3 下沉 RTP reorder/PS/compat 热路径 | 已完成（5/5：`cheetah-codec` 增加 `RtpReorderBuffer` 并接入 RTSP publish UDP ingest；`ps` 增加 RTSP payload bounded demux 测试与 probe 接入；H26x 参数集补发与 RTP ingress 时间戳 normalize helper 下沉 `cheetah-codec` 并接入 RTSP publish；unsupported codec 增加 sampled warn + 按 track 跳帧计数且不影响会话；补齐 payload type fallback/missing rtpmap/missing fmtp/absolute control/bad marker 兼容回归） | `rtsp-phase-03-server-publish-play.md` |
| Phase 04 | 4.1 新增 outbound RTSP client driver | 已完成（6/6：新增 `driver-tokio/src/client` 模块与 command/event API；完成 TCP direct client connect/request/response/interleaved 主链路；实现 UDP client endpoint（端口对分配、server_port 打洞、RTP/RTCP 收包事件）；实现 HTTP tunnel client（GET/POST 双连接、cookie、POST base64、GET response/interleaved）；实现 Basic/Digest(MD5) 401 retry header hook；补齐 driver 级 TCP/UDP/HTTP/auth 与 OPTIONS->DESCRIBE->SETUP->PLAY 状态机测试） | `rtsp-phase-04-client-forwarding-jobs.md` |
| Phase 04 | 4.2 实现 RTSP pull jobs | 已完成（6/6：扩展 `RtspModuleConfig` 增加 `pull_jobs`、`RtspPullTransport` 与配置校验并补回归；module start/stop 已接入 enabled pull-job supervisor 生命周期与取消等待；pull job 已完成 OPTIONS/DESCRIBE SDP 解析、tracks 更新与 publisher lease 获取并补回归；pull job RTP 已复用 server publish depacketize/timestamp normalize 路径并补回归；指数退避+上限、session timeout keepalive、lease release 与目标占用停止策略已完成并补回归；新增远端 synthetic RTSP source -> local engine -> RTSP/RTMP play 端到端回归） | `rtsp-phase-04-client-forwarding-jobs.md` |
| Phase 04 | 4.3 实现 RTSP push jobs 和 relay jobs | 已完成（6/6：扩展 `RtspModuleConfig` 增加 `push_jobs`/`relay_jobs`、默认值与校验，覆盖 URL/重试参数/凭据组合/transport 去重/跨 job 名称冲突回归；push job 已实现 source stream 订阅与 `OPTIONS -> ANNOUNCE` SDP 控制面握手；push job 已实现 `SETUP -> RECORD` 后 interleaved RTP 发送与 RTCP SR 发送；source track 变化可触发会话重建并补回归测试；relay job 已实现展开为 pull+push、无 `local_stream_key` 时隐藏 stream key 暴露和 supervisor 生命周期管理；补齐 local->remote、remote->local->remote、远端断开重试、module stop 无悬挂任务测试） | `rtsp-phase-04-client-forwarding-jobs.md` |
| Phase 05 | 5.1 扩展真实 fixture 与互操作矩阵 | 已完成（5/5：扩展 `.rtspcap` manifest 角色/transport；补齐 H264/AAC TCP/UDP/HTTP tunnel/multicast 标准样例；补齐 H265/AAC、audio-only、PS/MP2P 与 bad-sdp/multicast/http-tunnel probe 样例；补齐 manifest 校验回归；新增 fixture 驱动的端到端矩阵测试覆盖 server publish/play 与 RTSP pull/push/relay jobs） | `rtsp-phase-05-interop-robustness-fuzz.md` |
| Phase 05 | 5.2 扩展 属性测试/fuzz 传输扰动 | 已完成（5/5：新增 transport fault view helper；新增属性测试覆盖 Transport parse roundtrip/HTTP tunnel base64 分片/RTP reorder wrap；新增 6 个 fuzz target；所有 fuzz target 统一输入上限；真实 capture prefix corpus 按 standard/probe/fault 分类并接入新旧 target） | `rtsp-phase-05-interop-robustness-fuzz.md` |
| Phase 05 | 5.3 文档、feature、CI 和 smoke 收口 | 已完成（5/5：同步 `SystemArchitecture.md` 与 `dev-docs/SystemArchitecture.md` 的 RTSP 能力描述；更新 RTSP 配置说明，明确 `multicast` 与 `pull/push/relay jobs` 默认关闭，HTTP tunnel 仅在启用相应任务并选择 transport_preference 时生效；新增 RTSP dev smoke，覆盖 TCP/UDP/HTTP tunnel/multicast 拉取前几个 RTP 包；新增 RTSP jobs smoke，覆盖 synthetic remote pull->local play 与 local publish->push remote receive；新增 `rtsp-fast-matrix` CI，提供 core/property/module 快速矩阵与可调度/夜间 transport+jobs smoke） | `rtsp-phase-05-interop-robustness-fuzz.md` |

## 渐进式执行顺序

1. 先完成 Architecture 和 Phase 01，固定兼容策略、配置项、控制面行为和测试断言，避免后续传输实现互相推翻。
2. 再完成 Phase 02，让四类 RTP 传输在 server play/publish 的最小路径可用。
3. 再完成 Phase 03，补齐媒体热路径鲁棒性、RTCP、SDP/PS/codec 兼容和服务端推拉流质量。
4. 再完成 Phase 04，新增 outbound RTSP client 和静态转发任务，实现 RTSP 远端 pull/push/relay。
5. 最后完成 Phase 05，用真实样例、属性测试/fuzz、跨协议 E2E 和 smoke 收口，并同步 `SystemArchitecture.md`、README、配置示例。

## 阶段完成后的统一检查

```bash
cargo fmt
cargo clippy -p cheetah-runtime-api
cargo test -p cheetah-runtime-api
cargo clippy -p cheetah-runtime-tokio
cargo test -p cheetah-runtime-tokio
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec rtp
cargo test -p cheetah-codec ps
cargo clippy -p cheetah-rtsp-core
cargo test -p cheetah-rtsp-core
cargo clippy -p cheetah-rtsp-driver-tokio
cargo test -p cheetah-rtsp-driver-tokio
cargo clippy -p cheetah-rtsp-module --tests
cargo test -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-property-tests
```

新增或修改 fuzz target 后执行短跑 smoke：

```bash
cd crates/protocols/rtsp/fuzz
cargo +nightly fuzz build
cargo +nightly fuzz run fuzz_rtsp_core -- -runs=128
cargo +nightly fuzz run fuzz_real_capture_mixed_transport -- -runs=128
cargo +nightly fuzz run fuzz_real_capture_udp_datagrams -- -runs=128
cargo +nightly fuzz run fuzz_rtsp_http_tunnel -- -runs=128
cargo +nightly fuzz run fuzz_rtsp_multicast_transport -- -runs=128
```
