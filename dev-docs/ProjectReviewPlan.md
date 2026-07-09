# Cheetah Media Server 全面审查计划

> 目的：为 `cheetah` 流媒体服务器提供一份可执行、可追踪的全面代码审查计划。审查顺序严格遵循
> **正确架构 → 基础模块 → 各协议**，确保先锁定分层与依赖方向的正确性，再逐层向下验证共享基础
> 设施，最后按协议逐个核对 `core + driver + module` 三段式实现。
>
> 权威依据：`AGENTS.md`（工程约束）、`SystemArchitecture.md`（分层与协议映射）。二者与实现冲突时，
> 以文档约束为“应然”基准，用于判定实现是否需要修正或文档是否需要同步更新。

---

## 0. 审查范围与总体方法

### 0.1 审查对象总览

工作区共 55 个 crate，按职责分组：

| 分组 | 目录 | crate | 代码量(约) |
|------|------|-------|-----------|
| Foundation（基础/媒体内核） | `crates/foundation/` | `cheetah-codec` | ~24.6k 行 |
| Runtime（运行时抽象/实现） | `crates/runtime/` | `cheetah-runtime-api`、`cheetah-runtime-tokio` | ~0.8k 行 |
| SDK（契约层） | `crates/sdk/` | `cheetah-sdk`、`cheetah-sdk-macros` | ~1.1k 行 |
| System（系统编排层） | `crates/system/` | `cheetah-config`、`cheetah-control`、`cheetah-engine`、`cheetah-record-module` | ~8.6k 行 |
| Application（应用层） | `apps/` | `cheetah-server` | — |
| Protocols（协议三段式） | `crates/protocols/<proto>/` | 见 §4 | — |

各协议规模（含 core/driver/module/testing/fuzz/bindings 的 `.rs` 总量，约值）：

| 协议 | 代码量(约) | 文档映射 | 备注 |
|------|-----------|----------|------|
| webrtc | ~44.9k 行 | 未在 SystemArchitecture 记录 | 规模最大，需重点审查 |
| rtsp | ~50.6k 行 | 已记录 | 传输矩阵复杂 |
| rtmp | ~30.0k 行 | 已记录 | 含 c-api / wasm 绑定 |
| hls | ~12.5k 行 | 未在 SystemArchitecture 记录 | 含 LLHLS |
| srt | ~4.4k 行 | 未在 SystemArchitecture 记录 | 新增协议 |
| rtp | ~4.4k 行 | 已记录 | — |
| http-flv | ~4.4k 行 | 已记录 | — |
| ts | ~4.0k 行 | 已记录 | 无 property-tests |
| fmp4 | ~3.5k 行 | 已记录 | — |
| gb28181 | ~3.3k 行 | 已记录 | — |
| mp4 | ~2.5k 行 | 未在 SystemArchitecture 记录 | 新增协议 |

> **预置发现（待审查确认）：** `hls`、`ts(部分)`、`mp4`、`srt`、`webrtc` 已在 workspace 与 README 出现，
> 但 `SystemArchitecture.md` 的“Reference Mapping”仅覆盖 RTMP/RTSP/HTTP-FLV/fMP4/RTP/GB28181。按
> `AGENTS.md` §13 文档同步规则，这属于必须补齐的文档缺口，纳入 §1 架构审查项。

### 0.2 审查方法

对每一层/每一个 crate，统一走以下五个动作：

1. **边界核对**：依赖方向、命名、目录组织是否符合 `AGENTS.md` §1.1/§1.2/§2/§3。
2. **约束核对**：该层专属硬约束（Sans-I/O、runtime 中立、单发布者租约、上界缓冲等）。
3. **热路径核对**：是否存在阻塞、竞争锁、无界缓冲、多余 clone/memcpy。
4. **测试核对**：`core` 单测/属性/fuzz；`driver` I/O 集成；`module` 端到端与互操作，是否达到 §3.x 基线。
5. **一致性核对**：实现 vs `SystemArchitecture.md` / README / `config.example.yaml` 是否一致。

每项产出统一记录为：`结论(通过/风险/违规) + 证据(文件:行) + 建议动作`。

### 0.3 阶段划分与出口标准

| 阶段 | 内容 | 出口标准 |
|------|------|----------|
| P0 | §1 架构与依赖方向审查 | 分层依赖图闭合、无跨层偷依赖、命名/目录合规 |
| P1 | §2 基础模块（codec/runtime）审查 | AVFrame/TrackInfo/时间线模型自洽、runtime 中立 |
| P2 | §3 系统层（sdk/config/control/engine/record）审查 | 契约框架无关、生命周期与租约语义正确 |
| P3 | §4 各协议逐个审查（按依赖与风险排序） | 每协议三段式边界清晰、测试基线达标 |
| P4 | §5 跨协议一致性与全局非功能性审查 | 跨协议桥接、观测性、性能与安全达标 |
| P5 | §6 文档同步与收尾 | 文档与实现一致、审查报告归档 |

---

## 1. 阶段 P0：架构正确性与依赖方向审查（最先做）

先确认“骨架”正确，后续逐层审查才有基准。

### 1.1 六层分层与单向依赖

依据 `SystemArchitecture.md` §1，验证依赖只能单向向下：

```
apps/*
  → cheetah-engine, cheetah-control                (Engine orchestration)
    → cheetah-sdk, cheetah-runtime-api             (SDK & contracts)
      → cheetah-*-module                           (Module integration)
        → cheetah-*-driver-tokio, cheetah-runtime-tokio (Driver/runtime)
          → cheetah-*-core, cheetah-codec          (Foundation)
```

审查项：

- [x] 用 `cargo tree`/`cargo metadata` 生成实际依赖图，比对期望分层，标记任何反向或跨层依赖。
- [x] `cheetah-codec` 不依赖 engine/module/HTTP/DB/具体 runtime（`AGENTS.md` §2）。
- [x] `cheetah-sdk` 不反向依赖任何具体协议 module；HTTP module 契约不绑定 Axum/Tide/Actix。
- [x] feature module 仅通过 `cheetah-sdk` + `cheetah-codec` 与系统交互，无“偷依赖”。
- [x] `cheetah-runtime-api` / `cheetah-sdk` / `cheetah-engine` / `*-module` 公共接口不出现 `tokio::*`、
      `tokio_util::*` 类型（`AGENTS.md` §5）。

工具与命令：

```bash
cargo tree -p cheetah-server -e normal
bash dev-scripts/check_runtime_boundaries.sh   # runtime 边界 CI guard
```

### 1.2 crate 命名与目录组织

依据 `AGENTS.md` §1.1/§1.2：

- [x] 协议主路径命名为 `cheetah-<proto>-core/driver-tokio/module`，磁盘目录为 `core/`、`driver-tokio/`、`module/`。
- [x] 属性测试为 `cheetah-<proto>-property-tests`（禁止 `pbt` 缩写）；绑定为 `-c-api`/`-wasm`。
- [x] fuzz 为独立 cargo-fuzz workspace，默认不在根 workspace members 中。
- [x] 全仓无 `plugin` 命名残留（类型名/模块名/目录名统一 `module`）。

命令：

```bash
grep -rniE '\bplugin\b' crates apps --include='*.rs' --include='*.toml'
```

### 1.3 三段式完整性矩阵

对每个协议核对是否齐备 `core / driver-tokio / module`，以及 testing/fuzz/bindings 的存在性：

- [x] rtmp / rtsp / http-flv / hls / ts / fmp4 / mp4 / rtp / gb28181 / srt / webrtc 均具备三段式。
- [x] `ts` 缺少 `testing/property-tests`（对比其他协议）→ 已补齐（F-08）。
- [x] 各 fuzz 目录是否游离于根 workspace（符合约定）。

### 1.4 架构文档一致性（P0 关键交付）

- [x] `SystemArchitecture.md` 补齐 hls/ts/mp4/srt/webrtc 的 Reference Mapping 与 capability snapshot（F-07）。
- [x] README 的 feature 列表、端口、URL 与 `config.example.yaml`、`apps/cheetah-server` 默认 feature 一致。

**P0 出口：** 输出一张“实际依赖图 vs 期望分层”对照表 + 命名/目录/文档缺口清单。

---

## 2. 阶段 P1：基础模块审查（`cheetah-codec` + runtime）

基础层是所有协议的公共地基，必须在协议审查前锁定正确。

### 2.1 `cheetah-codec` 媒体内核（Foundation）

重点文件（按规模，`crates/foundation/cheetah-codec/src/`）：
`ts_demux.rs(2298)`、`fmp4_demux.rs(1245)`、`fmp4_mux.rs(1138)`、`ps.rs(1101)`、`video.rs(1076)`、
`time.rs(1065)`、`egress.rs(998)`、`jtt1078.rs(921)`、`rtp.rs(914)`、`mp4/reader.rs(889)`、`flv.rs(788)`、
`transcode.rs(710)`、`ts_mux.rs(597)`、`track.rs(532)`、`adapter.rs(519)`、`frame.rs(425)`、`compat.rs(458)`、
`mp4/{writer,reader,sample_table}.rs`、`record/`。

审查项（依据 `AGENTS.md` §7 + `SystemArchitecture.md` §4）：

- [x] **统一媒体模型**：所有协议入口收敛为 `AVFrame + TrackInfo`；无协议自带私有 frame 模型。
- [x] **三级时间线**（`time.rs`）：source / canonical / egress 边界正确；
      - source 时序仅作元数据，不得覆盖 canonical 顺序；
      - egress 时间戳修复不得回写 canonical 时间线。
- [x] **AVFrame 语义**（`frame.rs`）：`pts/dts/duration` 同时携带 timebase ticks 与 `*_us`；
      `FrameFlags::KEY / DISCONTINUITY / B_FRAME` 语义正确；`pts < dts` 必须显式 `B_FRAME`。
- [x] **归一化职责集中**：时间戳归一化、timebase 转换、DTS 生成、回绕、断流标记、Access Unit 拼装、
      参数集缓存/补发全部在 codec，不散落到协议（对照 §3 协议审查交叉验证）。
- [x] **封装/解封装**：`flv/ts_mux/ts_demux/fmp4_mux/fmp4_demux/ps/rtp/jtt1078/mp4` 的 box/包上限
      （如 `max_box_bytes`、`max_reassembly_bytes`）是否都有硬上界（`AGENTS.md` §9）。
- [x] **无协议状态机**：codec 不实现 RTMP/RTSP/WebRTC/SIP 协议状态机；无 FFmpeg 类型泄漏到公共接口。
- [x] **兼容层集中**（`compat.rs`）：厂商 quirks 显式命名、集中管理，未打散到热路径。
- [x] **codec 矩阵一致性**：README 声明的编解码矩阵（H264/H265/H266/VP8/VP9/AV1/MJPEG；AAC/G711/Opus/MP3/MP2）
      与 `video.rs/track.rs/egress.rs` 实际支持一致。

命令：

```bash
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo test -p cheetah-codec -- fmp4
cargo test -p cheetah-codec --test ts_codec_matrix
```

### 2.2 Runtime 抽象与实现

`crates/runtime/cheetah-runtime-api/src/lib.rs(445)`、`cheetah-runtime-tokio/src/lib.rs(354)`。

审查项（`AGENTS.md` §5）：

- [x] `RuntimeApi` 双通道任务模型：`spawn`（`Send` 主路径）+ `spawn_local`（browser/WASI），
      未为 wasm 削弱多线程主路径。
- [x] 公共接口 runtime 中立；`tokio`/`tokio-util` 仅存在于 `cheetah-runtime-tokio`。
- [x] 取消（`CancellationToken`）、任务句柄、完成通知等原语抽象完整，供 module 使用。

**P1 出口：** 媒体模型与时间线自洽性结论 + codec 上界/兼容层清单 + runtime 中立性结论。

---

## 3. 阶段 P2：系统编排层审查（SDK / Config / Control / Engine / Record）

### 3.1 `cheetah-sdk` + `cheetah-sdk-macros`（契约层）

文件：`module.rs(184)`、`stream.rs(257)`、`service.rs(126)`、`config.rs(174)`、`event.rs(108)`、`task.rs(73)`。

- [x] module 契约、引擎注入能力定义清晰；不反向依赖具体协议 module（`AGENTS.md` §2）。
- [x] HTTP module 契约框架无关（无 Axum/Tide/Actix 泄漏）。
- [x] `EngineContext` / `StreamKey` / 发布租约 / 生命周期（`create -> init -> start`）契约完整。
- [x] 宏（`sdk-macros`）生成代码不引入跨层依赖或 runtime 绑定。

### 3.2 `cheetah-config`

`src/lib.rs(582)`。审查项（对照 README §2）：

- [x] 配置加载顺序：默认值 → `CHEETAH_CONFIG` YAML → `M7S_` 前缀环境变量。
- [x] `M7S_GLOBAL__...` / `M7S_MODULE__<module>__...` 双下划线分层解析；bool/int/float/string 类型解析正确。
- [x] 配置模型与 `config.example.yaml`、各 module 配置字段一致（端口、队列容量、超时、TLS 等）。

### 3.3 `cheetah-engine`

文件：`stream.rs(1770)`、`engine.rs(936)`、`module_manager.rs(694)`、`task.rs(451)`、`core_adapters.rs(304)`、
`room.rs(112)`、`event.rs(100)`。

审查项（`AGENTS.md` §6/§9）：

- [x] **单发布者租约**：同一 `StreamKey` 默认单发布者独占语义，无绕过路径。
- [x] **模块生命周期**：`ModuleRestartRequired` 由基础层执行 `create->init->start`；module 不自维护私有重启。
- [x] `ModuleManagerApi::restart_module/restart_modules` 只接受 `Running`，否则返回 `Conflict`。
- [x] **Dispatcher / RingBuffer / subscriber queue**：慢订阅者不拖累其他订阅者；所有队列有上界。
- [x] 热路径无阻塞、无 contended mutex；`Arc<AVFrame>`/`Bytes`/有界缓冲使用得当。
- [x] `core_adapters.rs` 是否把 core 的 Sans-I/O 输出正确桥接到引擎，无业务逻辑下沉到 core。

### 3.4 `cheetah-control`

`src/lib.rs(989)`。

- [x] 控制面 API（REST）非阻塞；与各 module 暴露的 REST 端点风格统一（ZLM/SMS 兼容点）。
- [x] 鉴权、路由、错误语义一致。

### 3.5 `cheetah-record-module`

文件：`executor.rs(611)`、`zlm_compat.rs(396)`、`api.rs(381)`、`module.rs(346)`、`registry.rs(227)`、`config.rs(196)`、`metadata.rs(111)`。

- [x] 作为 system 层 module，遵守 module 约束（走 `EngineContext`，不直接依赖 tokio net/time/sync）。
- [x] 录制走 codec 导出视图，不复制时间戳/NALU/参数集逻辑。
- [x] `zlm_compat` 兼容逻辑集中、显式命名。

**P2 出口：** 契约框架无关性结论 + 生命周期/租约语义结论 + 配置一致性结论。

---

## 4. 阶段 P3：各协议逐个审查

**审查顺序（按“依赖基础程度 + 风险/规模”排序）：**
`rtp → ts → http-flv → fmp4 → mp4 → hls → rtmp → rtsp → gb28181 → srt → webrtc`

> 说明：先审 RTP/TS 等被上层复用的“被桥接”协议，再审封装型（flv/fmp4/mp4/hls），再审信令复杂型
> （rtmp/rtsp/gb28181），最后审规模最大、约束最新的 srt/webrtc。

### 4.0 每个协议的统一审查清单（模板）

对每个 `<proto>` 执行：

**A. `core`（Sans-I/O 硬约束，`AGENTS.md` §4）**
- [x] 不依赖 tokio/任何 runtime；不持有 socket/listener/stream；不 spawn 线程/任务。
- [x] 无 `async fn` 作为核心状态机接口；不调用 `Instant::now()`/系统时间。
- [x] 不访问 DB/HTTP/`EngineContext`/`StreamManager`/`RoomService`；无业务编排。
- [x] I/O 为显式 `Input / Output / Event / Timer` 模型。

**B. `driver-tokio`（`AGENTS.md` §5）**
- [x] 收包/发包/分帧/组帧/timer/spawn/channel/backpressure 均在此层。
- [x] TCP framing / UDP 收发在 driver，不在 core；不持有业务状态。
- [x] runtime 抽象通过 `RuntimeApi` 注入；公共接口不泄漏 tokio 类型。

**C. `module`（`AGENTS.md` §6）**
- [x] 通过 `EngineContext` 交互；不重写协议状态机；不直接用 `tokio::net/time/sync`、`tokio::select!`。
- [x] 资源分配/会话绑定/鉴权/API 路由/业务映射在此层；单发布者租约不被绕过。
- [x] 不复制 codec 的时间戳修复/NALU/参数集逻辑。

**D. 测试（`AGENTS.md` §11 + `SystemArchitecture.md` §6）**
- [x] core：单测 + 属性测试 + fuzz（无真实网络 I/O）。
- [x] driver：I/O 集成测试。
- [x] module：互操作 + 端到端。
- [x] 时间戳/重排/参数集补发/兼容修复均有回归测试。

**E. 一致性**
- [x] 与 `SystemArchitecture.md` Reference Mapping、README 使用说明、`config.example.yaml` 一致。

### 4.1 RTP（`crates/protocols/rtp/`）
- [x] Auto Probe：TS(`0x47`)/PS(`0x000001BA`)/Raw ES/Ehome/Hikvision XHB/JT1078 探测正确。
- [x] Jitter buffer 有界；PS 重组 `max_reassembly_bytes`(4MB) 硬上限；空闲/超时清理句柄。
- [x] UDP/TCP（RFC4571 2字节 + RTSP 4字节 interleaved 自动识别）；RTCP SR/RR、RR-timeout。
- [x] 与 RTMP/RTSP/HLS/fMP4 的双向桥接（引擎内）正确。
- 命令：`cargo test -p cheetah-rtp-core/-driver-tokio/-module/-property-tests`

### 4.2 TS（`crates/protocols/ts/`）
- [x] `TsCore` 仅处理 HTTP 头/CORS/WS 升级/会话取消；PAT/PMT 周期注入（`pat_pmt_interval_ms`）。
- [x] GOP 缓存（`bootstrap_max_frames`）；`strict_crc`、`max_reassembly_bytes` 上界。
- [x] RTP-TS ingest（`RtpTsIngest`）解复用正确；TLS(8445) 与 `.live.ts` 兼容 URL。
- [x] **缺 property-tests** → 已补齐（F-08）。
- 命令：`cargo test -p cheetah-ts-core/-driver-tokio/-module`；`cargo test -p cheetah-codec --test ts_codec_matrix`

### 4.3 HTTP-FLV（`crates/protocols/http-flv/`）
- [x] HTTP-FLV / WS-FLV 路由 `/{app}/{stream}.flv`；pull job 重试 backoff。
- [ ] fixture 回放（`tests/testdata/http-flv/{standard,probes}`）与 transport fault 视图（manifest.tsv 缺失，需补齐 fixture）。
- 命令：`cargo test -p cheetah-http-flv-core/-driver-tokio/-module`；`bash dev-scripts/check_http_flv_smoke.sh`

### 4.4 fMP4（`crates/protocols/fmp4/`）
- [x] `Fmp4Core` 纯状态机；init(ftyp+moov)/media(styp+moof+mdat) 走 codec `Fmp4Muxer`。
- [x] `max_tracks/max_box_bytes/max_fragment_duration_ms` 上界；`.mp4`/`.live.mp4` 路由；TLS(8446)。
- [x] bootstrap GOP 追赶；pull(http/https/ws/wss) backoff。
- 命令：`cargo test -p cheetah-fmp4-core/-driver-tokio/-module/-property-tests`；`cargo test -p cheetah-codec -- fmp4`

### 4.5 MP4（`crates/protocols/mp4/`）
- [x] 三段式与 codec `mp4/{writer,reader,sample_table}` 分工；点播/录制路径。
- [x] **补 SystemArchitecture Reference Mapping**（F-07）。
- 命令：`cargo test -p cheetah-mp4-core/-driver-tokio/-module/-property-tests`

### 4.6 HLS / LLHLS（`crates/protocols/hls/`）
- [x] `ts`/`fmp4` container 切换；`ll_hls_enabled`、`part_target_ms`、`segment_*` 语义。
- [x] LLHLS 标签（`SERVER-CONTROL/PART-INF/PART/PRELOAD-HINT/PROGRAM-DATE-TIME`）正确。
- [x] Session（Cookie `HLS_SESSION` + `?session=`）；CDN `cdn_secret` Bearer 与缓存语义。
- [x] **补 SystemArchitecture Reference Mapping**（F-07）。
- 命令：`cargo test -p cheetah-hls-core/-module/-property-tests`；`cargo test -p cheetah-hls-core -- ll_hls`；
  `bash dev-scripts/check_hls_smoke.sh`；`bash dev-scripts/check_llhls_demuxed.sh`

### 4.7 RTMP（`crates/protocols/rtmp/`）
- [x] core `#![no_std] + alloc`，无 std 特性开关即可 host/cross 编译；无连接门面（`RtmpServerConnection` 等）残留。
- [x] runtime wiring 仅在 driver（`start_server`/`start_client`）；module 拥有 pull/push 后台任务编排。
- [x] c-api / wasm 仅 core 绑定，不搬运 driver/module 逻辑、不改热路径。
- [x] Enhanced-RTMP、`vendor-ref/simple-media-server` 对齐点（见 `dev-docs/RTMPAlignmentWithSimpleMediaServer.md`）。
- 命令：`cargo test -p cheetah-rtmp-core/-module/-property-tests/-c-api/-wasm`；
  `bash dev-scripts/check_rtmp_core_no_std.sh`；`bash dev-scripts/check_rtmp_vendor_parity.sh`

### 4.8 RTSP（`crates/protocols/rtsp/`）
- [x] 控制面方法齐全（OPTIONS/DESCRIBE/ANNOUNCE/SETUP/PLAY/PAUSE/RECORD/TEARDOWN/GET_/SET_PARAMETER）。
- [x] 传输矩阵：UDP unicast / TCP interleaved / HTTP tunnel(GET+POST+cookie+base64) / multicast PLAY。
- [x] core 覆盖解析（Transport/Session/Range/RTP-Info/SDP/auth）+ interleaved framing + RTP/RTCP 模型。
- [x] driver 拥有 HTTP tunnel 连接配对、multicast 端点、outbound client；module 拥有租约/会话/pull/push/relay 监管。
- 命令：`cargo test -p cheetah-rtsp-core/-driver-tokio/-module/-property-tests`；
  `bash dev-scripts/check_rtsp_transport_smoke.sh`；`bash dev-scripts/check_rtsp_jobs_smoke.sh`

### 4.9 GB28181（`crates/protocols/gb28181/`）
- [x] `Gb28181Core` Sans-I/O：SIP(REGISTER/MESSAGE/INVITE/ACK/BYE)、设备注册表、keepalive 超时、SDP(`GbSdp`)。
- [x] MD5 挑战鉴权；宽松行终止符（`\r\n`/`\n`/`\r`）与 `,`/`;` Digest 解析；重复头容忍。
- [x] INVITE 分配 RTP 端口 + SSRC；BYE/离线闭环清理租约与端口映射，无句柄泄漏。
- [x] REST 端点（Catalog/INVITE/BYE/talkback）ZLM/SMS 兼容。
- 命令：`cargo test -p cheetah-gb28181-core/-driver-tokio/-module/-property-tests`

### 4.10 SRT（`crates/protocols/srt/`）
- [x] 三段式边界；依赖 `shiguredo_srt` 的封装是否局限在 driver/core 适配层。
- [x] 握手/加密/延迟窗口/带宽估计等缓冲的上界。
- [x] **补 SystemArchitecture Reference Mapping**（F-07）；对照 `dev-docs/plans-28-srt`。
- 命令：`cargo test -p cheetah-srt-core/-driver-tokio/-module/-property-tests`

### 4.11 WebRTC（`crates/protocols/webrtc/`，规模最大）
- [x] 基于 `str0m`（Sans-I/O）：确认 core 保持 Sans-I/O，`str0m` 类型不泄漏到 sdk/engine 公共接口。
- [x] 信令（WHIP/WHEP / ZLM / OME 兼容，见 `tests/fixtures/{zlm,ome}`）、SDP 协商、ICE/DTLS/SRTP 边界。
- [x] P2P 路径（`module/src/p2p`）职责；发布租约与引擎桥接。
- [x] **补 SystemArchitecture Reference Mapping**（F-07）。
- 命令：`cargo test -p cheetah-webrtc-core/-driver-tokio/-module/-property-tests`

**P3 出口：** 每协议一页“三段式合规 + 测试基线 + 一致性”结论表 + 违规/风险清单。

---

## 5. 阶段 P4：跨协议一致性与全局非功能性审查

### 5.1 跨协议桥接
- [x] 任一入口协议 → `AVFrame + TrackInfo` → 任一出口协议，转换正确（RTMP/RTSP/HLS/fMP4/TS/RTP 互转）。
      本轮已验证 RTSP↔RTSP loopback、RTMP→RTSP TCP/UDP bridge、RTSP→RTMP bridge；其余协议在 P3/P4 早期已跑通。
- [x] 使用 `dev-scripts/cross_protocol_matrix_*`（input matrix / command templates / acceptance matrix / regression）
      核对跨协议接受矩阵。

### 5.2 观测性与诊断（`SystemArchitecture.md` §4）
- [x] 运行报告暴露 `startup_latency_ms`、`first_second_avg_frame_interval_ms`、`average_playback_rate_x`、`first_keyframe_delay_ms`（F-09）。
- [x] 时间戳修复告警分层：`source_repair_events` / `canonical_repair_events` / `egress_repair_events`；
      正常 B 帧重排不升级 canonical/egress 告警；高频告警对阈值（`REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD`）判定（F-09）。
- [ ] 每条 repair 日志含 source + canonical 上下文。

### 5.3 性能与并发（`AGENTS.md` §9）
- [ ] 全仓热路径扫描：无阻塞调用、无每包必经的 contended mutex、无无界缓冲。
- [ ] 所有缓存/队列/重传窗口/jitter buffer 均有上界（逐一列举确认）。
- [ ] `dev-scripts/perf_profile.sh`、`valgrind_check.sh` 结果评估。

### 5.4 安全
- [ ] 输入侧脏数据/超长包/坏包的防溢出（各 `max_*` 上界）。
- [ ] 鉴权路径（RTSP auth、GB28181 MD5 挑战、CDN Bearer、TLS 证书加载）无绕过。
- [ ] 无凭证/密钥硬编码或日志泄漏。

### 5.5 应用层（`apps/cheetah-server`）
- [x] 启动序列：runtime → 配置 → engine → control → 按 feature 启动各 module。
- [x] feature gating 正确：默认仅 `rtmp`；`--features` 组合与 README §1/§5 一致。

---

## 6. 阶段 P5：文档同步与收尾

- [x] 按 `AGENTS.md` §13 更新 `SystemArchitecture.md`（补 hls/ts/mp4/srt/webrtc 映射）、`AGENTS.md`、README、`config.example.yaml`。
- [x] 归档审查报告：按 §0.2 格式汇总各阶段 `结论 + 证据 + 建议动作`，区分“违规（必须修）/风险（建议修）/观察（记录）”。
- [x] 提交前最低检查（`AGENTS.md` §12）：`cargo fmt` → `cargo clippy -p <changed>` → `cargo test -p <changed>`；
      影响基础/公共层时补工作区相关测试；不例行 `--all-features`。

---

## 附录 A：一键检查脚本索引（`dev-scripts/`）

| 脚本 | 用途 |
|------|------|
| `check_runtime_boundaries.sh` | runtime 中立性边界 CI guard（P0） |
| `check_rtmp_core_no_std.sh` | RTMP core no_std 编译 |
| `check_rtmp_vendor_parity.sh` / `check_rtmp_fuzz_smoke.sh` | RTMP 对齐 vendor / fuzz smoke |
| `check_http_flv_smoke.sh` | HTTP-FLV 冒烟 |
| `check_hls_smoke.sh` / `check_llhls_demuxed.sh` | HLS / LLHLS 冒烟 |
| `check_rtsp_transport_smoke.sh` / `check_rtsp_jobs_smoke.sh` | RTSP 传输 / job 冒烟 |
| `cross_protocol_matrix_*` | 跨协议接受矩阵与回归（P4） |
| `perf_profile.sh` / `valgrind_check.sh` | 性能剖析 / 内存检查（P4） |

## 附录 B：审查记录模板

```
[层/协议] <名称>  阶段 <P?>
- 审查项: <checklist 条目>
- 结论: 通过 | 风险 | 违规
- 证据: <path/to/file.rs:行号>
- 依据: AGENTS.md §? / SystemArchitecture.md §?
- 建议动作: <修实现 | 修文档 | 补测试 | 记录观察>
- 状态: 待办 | 处理中 | 已解决
```
