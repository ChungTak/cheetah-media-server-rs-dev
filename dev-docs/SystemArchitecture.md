# SystemArchitecture

> 项目：cheetah 高性能流媒体服务器
>
> 文档版本：v2
>
> 文档定位：系统总体架构设计

---

## 1. 设计目标与总原则

cheetah 的目标不是只做一个“能跑的 Rust 流媒体服务”，而是构建一套可长期演进、可扩展到多协议、多场景、多集群形态的**高性能流媒体服务器内核**。

系统设计围绕以下目标展开：

1. **高并发与低延迟**：单机承载尽可能多的流与订阅者，同时保持低延迟、低复制、低调度开销。
2. **一份媒体数据，多人共享**：通过 `Arc<AVFrame>`、无锁 RingBuffer、单次读取多路分发，避免 O(N) 拷贝与重复发送。
3. **协议与 I/O 解耦**：所有协议统一采用 `core + driver + module` 三段式；协议核心必须是 Sans-I/O。
4. **媒体语义统一**：所有协议进入引擎前都收敛为 `AVFrame + TrackInfo`；所有协议输出前都由 `cheetah-codec` 负责导出目标封装视图。
5. **兼容优先，不止标准优先**：实现目标不是“只满足 RFC/规范文档”，而是要对齐成熟 C++ 流媒体服务器simple-media-server[vendor-ref/simple-media-server]在真实客户端、真实设备、真实脏流下的互操作行为。
6. **热路径去锁化，冷路径允许上锁**：媒体热路径尽量单线程分片、无锁或无竞争；配置、注册、控制面允许使用锁，但不能污染每包必经路径。
7. **可替换运行时与密码学后端**：协议核心不依赖 Tokio；第一阶段优先支持 Tokio，后续可扩展到 compio、smol、blocking I/O、wasm；TLS/DTLS/SRTP 后端可插件化切换。
8. **模块化生态**：所有协议与功能能力按独立 crate 组织，支持按需编译、独立测试、并行演进。

本文统一使用 **module（模块）** 一词，不再使用 plugin。

---

## 2. 六层分层架构

采用严格的六层分层架构，每一层都有清晰的职责边界和依赖方向。

```text
┌─────────────────────────────────────────────────────────────┐
│                     External Clients                        │
│   Browser · Mobile · FFmpeg/OBS · IoT · GB28181 · Admin    │
├─────────────────────────────────────────────────────────────┤
│                   Infrastructure Layer                      │
│   HTTP Server · gRPC · Database · Config · Event System    │
│   FFmpeg · Network/QUIC · User System · Admin UI           │
├─────────────────────────────────────────────────────────────┤
│                 Engine Built-in Services                    │
│   StreamManager · ModuleManager · RoomService              │
│   TaskSystem · CoreAdapters                                │
├─────────────────────────────────────────────────────────────┤
│                    Feature Modules (26+)                    │
│   RTMP · RTSP · HLS · FLV · WebRTC · SRT · GB28181         │
│   WebTransport · MP4 · Snap · Transcode · Cluster · ...    │
├─────────────────────────────────────────────────────────────┤
│                   cheetah-sdk (Module SDK)                 │
│   Module trait · EngineContext · HTTP SDK · EventBus       │
│   ServiceRegistry · DatabaseApi · ProxyManager             │
├─────────────────────────────────────────────────────────────┤
│                cheetah-codec (Foundation)                  │
│   AVFrame · TrackInfo · RTP · PS · Time · Codec Helpers    │
│   Trait Interfaces · Zero internal deps                    │
└─────────────────────────────────────────────────────────────┘
```

### 2.1 依赖方向

依赖方向**单向向下**：上层依赖下层，下层不感知上层。

- `cheetah-codec` 是媒体语义基座，零内部业务依赖。
- `cheetah-sdk` 在 `cheetah-codec` 之上定义模块契约与引擎能力注入接口。
- 各个协议/功能模块通过 `cheetah-sdk` 与引擎交互，通过 `cheetah-codec` 共享统一媒体模型。
- 引擎内建服务负责流管理、任务管理、模块管理和基础调度。
- 基础设施层提供 HTTP/gRPC、数据库、配置、FFmpeg、QUIC、管理面等外部支撑能力。

### 2.2 分层收益

- **编译隔离**：修改一个模块不会触发全量重编译。
- **测试隔离**：协议 core、driver、module 可分别做单元测试、集成测试和互操作测试。
- **灵活组合**：通过 Cargo feature 按需启用协议与功能模块。
- **可维护性**：媒体语义、协议语义、运行时适配、业务编排边界清晰。

---

## 3. 统一协议内部分层设计：core + driver + module

所有协议都必须遵循统一的三段式内部分层设计：

```text
protocol-core    // 纯 Sans-I/O 协议核心
protocol-driver  // runtime / socket / timer 驱动层
protocol-module  // 引擎接入模块
```

统一依赖方向为：

```text
protocol-module -> protocol-driver -> protocol-core -> cheetah-codec
```

### 3.1 core：协议核心

`core` 负责协议状态机、协议语义、协议输入输出契约。它必须满足严格的 Sans-I/O 约束：

- 不依赖 Tokio 或任何具体 runtime
- 不持有 socket / listener / stream
- 不直接调用系统时钟
- 不启动线程或异步任务
- 不直接访问数据库、HTTP、引擎服务
- 不写业务逻辑编排
- 尽量使用同步状态机与纯函数式输入输出

也就是说：`core` 只做“**收到什么输入，协议状态如何演进，应该输出什么动作**”。

### 3.2 driver：驱动层

`driver` 负责把 `core` 落到具体运行时和 I/O 上：

- UDP/TCP/socket 包装
- timer 适配
- spawn / channel / task 驱动
- 收包、分帧、组帧、发包
- 将网络输入、时间推进、外部命令注入 `core`
- 将 `core` 输出的发送动作、定时器动作、协议事件落到 runtime

第一阶段只实现 Tokio 版本；未来可在不改变 core 设计的前提下扩展到 compio、smol、blocking I/O、tokio-uring、embassy、wasm。

### 3.3 module：接入模块

`module` 负责把协议能力接入引擎和业务系统：

- 对接 `EngineContext`
- 对接 `StreamManager / RoomService / Admin API / ServiceRegistry`
- 做资源分配、会话绑定、业务编排、权限控制
- 把协议事件映射为系统动作
- 把系统动作映射为协议命令

一句话：

- `core` 负责协议
- `driver` 负责运行
- `module` 负责接入

### 3.4 统一的协议输入输出模型

建议所有协议 core 尽量收敛到统一输入输出模型：

```rust
pub enum CoreInput<'a, C, M> {
    Packet { bytes: &'a mut [u8], meta: M },
    Timeout { id: TimerId },
    Command(C),
}

pub enum CoreOutput<E> {
    Send {
        bytes: bytes::Bytes,
        target: std::net::SocketAddr,
    },
    SetTimer {
        id: TimerId,
        at: MonoTime,
    },
    CancelTimer {
        id: TimerId,
    },
    Event(E),
}
```

`core` 对外只暴露：

- 收到网络数据包
- 某个超时到点
- 上层给出一个协议命令

然后返回：

- 需要发出的报文
- 需要设置或取消的定时器
- 抛出的协议事件

### 3.5 时间显式注入

时间必须由外部驱动注入，而不是由 core 内部主动获取：

- `core` 不得调用 `Instant::now()`
- `driver` 通过 `Timeout` 或 `now` 参数推进状态机
- 超时清理、NACK 老化、重传窗口裁剪、事务过期都应在输入驱动时被动完成

这是一条系统级规则，不只适用于 WebRTC，也适用于 RTMP、RTSP、SRT、GBSip、HTTP-FLV、WebTransport 等全部协议。

### 3.6 协议 crate 命名规则

所有协议统一采用如下命名：

```text
cheetah-<proto>-core
cheetah-<proto>-driver-tokio
cheetah-<proto>-module
```

例如：

```text
cheetah-webrtc-core
cheetah-webrtc-driver-tokio
cheetah-webrtc-module

cheetah-gbsip-core
cheetah-gbsip-driver-tokio
cheetah-gb28181-module

cheetah-srt-core
cheetah-srt-driver-tokio
cheetah-srt-module
```

目录组织不再按 crate 名称在 `crates/` 顶层扁平展开，而是按职责分组：

```text
crates/
  foundation/
  runtime/
  sdk/
  system/
  protocols/
    <proto>/
      core/
      driver-tokio/
      module/
      bindings/<target>/
      testing/<kind>/
      fuzz/
```

目录名用于阅读聚合，Cargo package name 仍按 `cheetah-` 完整命名。`fuzz/` 是独立 cargo-fuzz workspace，默认不加入根 workspace members。

---

## 4. 运行时与 I/O 适配层

协议核心不依赖具体 runtime，但 `driver` 需要 runtime 抽象。建议引入统一的 `cheetah-runtime-api`。

### 4.1 运行时抽象目标

目标是屏蔽 Tokio/compio/async-std/tokio-uring/smol/embassy/blocking I/O/wasm 差异。第一阶段只实现 Tokio，但接口按多 runtime 演进设计。

### 4.2 Runtime trait 设计

```rust
pub trait Runtime: Send + Sync + 'static {
    type UdpSocket: AsyncUdpSocket;
    type TcpStream: AsyncTcpStream;
    type TcpListener: AsyncTcpListener;
    type Timer: AsyncTimer;
    type JoinHandle: JoinHandle;

    fn now(&self) -> MonoTime;
    fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) -> Self::JoinHandle;
    fn spawn_local(&self, fut: impl Future<Output = ()> + 'static)
        -> Result<Self::JoinHandle, SpawnError>;
    fn oneshot(&self) -> (OneShotSender, OneShotReceiver);
    fn wrap_udp_socket(&self, socket: std::net::UdpSocket) -> std::io::Result<Self::UdpSocket>;
    fn wrap_tcp_listener(
        &self,
        listener: std::net::TcpListener,
    ) -> std::io::Result<Self::TcpListener>;
    fn wrap_tcp_stream(
        &self,
        stream: std::net::TcpStream,
    ) -> std::io::Result<Self::TcpStream>;
    fn sleep_until(&self, deadline: MonoTime) -> Self::Timer;
}
```

可以继续拆分：

- `AsyncUdpSocket`
- `AsyncTcpStream`
- `AsyncTcpListener`
- `AsyncTimer`
- `Spawn`
- `ChannelFactory`
- `CancellationToken`
- `OneShotSender / OneShotReceiver`

### 4.3 第一阶段落地范围

第一阶段仅实现：

- `cheetah-runtime-tokio`
- `TokioUdpSocket`
- `TokioTcpListener`
- `TokioTimer`
- `TokioSpawner`

### 4.4 通用驱动循环模型

```rust
loop {
    while let Some(out) = core.poll_output(rt.now()) {
        handle_output(out).await?;
    }

    tokio::select! {
        recv = socket.recv_from(&mut buf) => {
            let now = rt.now();
            let (n, meta) = recv?;
            core.handle_input(now, CoreInput::Packet {
                bytes: &mut buf[..n],
                meta,
            });
        }
        Some(timer_id) = timers.next_fired() => {
            core.handle_input(rt.now(), CoreInput::Timeout { id: timer_id });
        }
        Some(cmd) = cmd_rx.recv() => {
            core.handle_input(rt.now(), CoreInput::Command(cmd));
        }
    }
}
```

### 4.5 runtime 与 core 的边界

必须强调：

- `core` 不依赖 runtime trait
- runtime trait 仅存在于 `driver`
- `module` 可以选择依赖 `driver` 暴露的异步接口
- `cheetah-runtime-api` / `cheetah-sdk` / `cheetah-engine` / `*-module` 的公共接口不得直接暴露 `tokio::*` / `tokio_util::*`
- `core` 保持完全同步、可测试、可 fuzz、可在无网络环境下回放输入日志

---

## 5. cheetah-codec (Foundation)

`cheetah-codec` 不是“编解码工具箱”，而是整个系统的**媒体语义基座**。

它负责：

1. 定义系统统一的压缩媒体中间表示 `AVFrame / TrackInfo`
2. 统一时间戳、时基、回绕、跳变、DTS/PTS 生成与归一化
3. 提供音视频 elementary stream 的规范化表示
4. 提供跨协议封装转换所需的纯算法能力
5. 提供对真实世界脏数据、非标准设备、历史兼容差异的修正层

它不负责：

- socket / runtime / async 调度
- 协议状态机
- 数据库、引擎服务、业务逻辑
- 原始像素 YUV / PCM 解码处理
- FFmpeg 生命周期管理

### 5.1 设计原则

#### 5.1.1 统一媒体对象，而不是协议私有对象

进入引擎的数据必须尽快收敛为统一 `AVFrame`。RTMP、RTSP、WebRTC、GB28181、SRT、FLV、HLS 不允许各自维护一套互不兼容的私有 frame 语义。

#### 5.1.2 内部优先使用规范化中间表示

入口允许脏、允许乱、允许带历史包袱；但一旦进入 `cheetah-codec`，就要转换成统一 canonical form。

#### 5.1.3 时间戳是一级公民

时间戳修正、回绕展开、DTS 生成、跨协议时基互转必须上收到 foundation 层，不允许散落在各协议中各修各的。

#### 5.1.4 兼容优先，规范其次

规范文档定义下限；成熟 C++ 流媒体服务器simple-media-server[vendor-ref/simple-media-server]与真实设备/真实客户端的互操作行为定义实际上线标准。

### 5.2 内部分层

```text
cheetah-codec
├── frame/          // AVFrame, FrameFlags, FrameView, FrameOrigin
├── track/          // TrackInfo, CodecConfig, Extradata, ParameterSets
├── time/           // Timebase, Timestamp, StampAdjust, DtsGenerator, WrapUnwrapper
├── video/          // H264/H265/H266/AV1/VP8/VP9 access unit helpers
├── audio/          // AAC/ADTS/ASC, G711, Opus, MP3 helpers
├── rtp/            // RTP header, packetizer, depacketizer, clock helpers
├── ps/             // MPEG-PS/PES parsing与封装
├── flv/            // FLV tag body 与 sequence header helper
├── mp4/            // avcC/hvcC/esds/sample entry helper
├── compat/         // 兼容策略、quirk profile、vendor patch
└── traits/         // CodecNormalizer, Packetizer, Depacketizer, TrackExporter
```

所有模块都应满足：

- 无 I/O
- 无 runtime 依赖
- 同步、可测试
- 输入输出明确

### 5.3 统一媒体对象：AVFrame

这里的 `AVFrame` 指的是**统一压缩媒体帧**，不是 FFmpeg 中的原始图像/音频 `AVFrame`。

```rust
pub struct AVFrame {
    pub track_id: TrackId,
    pub media_kind: MediaKind,      // Video / Audio / Data / Subtitle
    pub codec: CodecId,             // H264 / H265 / AAC / Opus / G711A ...
    pub format: FrameFormat,        // CanonicalH26x / AacRaw / OpusPacket / ...
    pub pts: i64,                   // 原始时基下
    pub dts: i64,                   // 原始时基下
    pub timebase: Timebase,         // 1/90000, 1/48000, 1/1000...
    pub pts_us: i64,                // 归一化后微秒
    pub dts_us: i64,                // 归一化后微秒
    pub flags: FrameFlags,
    pub payload: bytes::Bytes,
    pub side_data: smallvec::SmallVec<[FrameSideData; 4]>,
    pub origin: FrameOrigin,
}
```

建议 `FrameFlags` 至少包含：

- `KEY`
- `CONFIG`
- `START_OF_AU`
- `END_OF_AU`
- `B_FRAME`
- `NON_PICTURE`
- `DISCONTINUITY`
- `GENERATED`
- `CORRUPTED`
- `DROPPABLE`

### 5.4 TrackInfo 与 CodecConfig

`TrackInfo` 是跨协议互转的依据，而不是可选元数据。

```rust
pub struct TrackInfo {
    pub track_id: TrackId,
    pub media_kind: MediaKind,
    pub codec: CodecId,
    pub payload_type: Option<u8>,
    pub clock_rate: u32,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<Rational32>,
    pub bitrate: Option<u32>,
    pub extradata: CodecExtradata,
    pub readiness: TrackReadiness,
}
```

`CodecExtradata` 负责承载：

- H264：SPS / PPS / avcC
- H265：VPS / SPS / PPS / hvcC
- H266：VPS / SPS / PPS
- AAC：ASC
- Opus：fmtp / channel mapping
- AV1：sequence header / codec config

`TrackInfo` 必须支持导出：

- SDP / FMTP
- FLV sequence header
- MP4 sample entry / avcC / hvcC / esds
- RTP payload 参数
- 浏览器 / FFmpeg / VLC 所需 codec config

### 5.5 Track readiness 规则

只有当轨道的关键信息准备完成后，才允许对外发布：

- H264/H265/H266：未拿到参数集前不 ready
- AAC：未拿到 ASC 前不 ready
- Opus/G711：可较早 ready
- 录制、转封装、级联、拉流发布都以 `TrackInfo::readiness` 为依据

### 5.6 时间戳系统

时间戳系统是 `cheetah-codec` 的核心子系统。

#### 5.6.1 统一时间模型

系统同时保留两套时间：

1. 原始时间：`pts/dts + timebase`
2. 归一化时间：`pts_us/dts_us`

这样可以：

- 在协议出口保留原始 clock 语义
- 在引擎内部用统一微秒时间排序、同步、做 GOP 计算、回放与集群对时

#### 5.6.2 时间处理组件

```text
WrapUnwrapper       // RTP 32bit / MPEG 时钟回绕展开
StampAdjust         // 时间戳跳变、静止、倒退、漂移修正
DtsGenerator        // 只有 PTS 或存在 B 帧时生成 DTS
TimebaseConverter   // 90000 / samplerate / ms / us 互转
DiscontinuityJudge  // 断流、seek、切源、回放跳点检测
```

#### 5.6.3 StampAdjust 模式

```rust
pub enum StampAdjustMode {
    Source,         // 优先使用源时间戳
    Arrival,        // 使用到达时间 / 系统时间
    SampleCount,    // 根据采样数推进
    FrameRateGuess, // 根据推断帧率推进
}
```

时间戳修正规则：

- 源时间戳正常：优先保留源时间
- 源时间戳回退、跳变、恒为 0：进入修正模式
- 音频优先按 `sample_count / sample_rate` 修正
- 视频优先按显式 fps，其次按统计推断 fps 修正
- RTP 回绕必须展开
- seek / flush / 切源必须打 `DISCONTINUITY`
- `pts_us/dts_us` 必须单调可比较

#### 5.6.4 DTS 生成器

Foundation 层必须内建 `DtsGenerator`：

- 解决只有 PTS 的输入流
- 解决 B 帧重排下 PTS ≠ DTS 的情况
- 提供跨协议的统一 DTS 语义

### 5.7 访问单元拼装与格式规范化

#### 5.7.1 两级对象

协议入口拿到的往往不是最终可分发的“帧”，而是：

- 单个 RTP payload
- AVCC/length-prefixed block
- Annex-B 多 NAL 拼包
- PS/PES 中的一段 ES
- FLV tag body
- AAC ADTS frame

因此 foundation 层要区分：

```text
IngressPacket / CodecUnit  ->  AccessUnitAssembler  ->  AVFrame
```

- `CodecUnit`：单个 NAL / OBU / AAC raw / RTP payload 片段
- `AVFrame`：组装完成、可进入 RingBuffer 的统一访问单元

#### 5.7.2 H26x 以 Access Unit 为核心

对于 H264/H265/H266，内部 canonical form 建议围绕 Access Unit 组织，而不是绑死 AVCC 或 Annex-B：

```text
Canonical H26x = AccessUnit { nalus: SmallVec<[NaluRef; N]> }
```

外部按需导出：

- Annex-B
- AVCC / HVCC
- FLV tag body
- RTP payload
- MP4 sample payload

#### 5.7.3 参数集缓存与自动补发

Foundation 层维护每个 video track 的参数集缓存：

- 最新 VPS/SPS/PPS
- 最近 sequence header / codec config
- 关键帧前置补发策略

这属于媒体兼容层能力，不属于某个具体协议模块。

### 5.8 懒转换与 FrameView

协议间最大的差异之一是封装格式不同。建议统一使用懒转换策略：

```rust
pub struct FrameData {
    pub source: SourceFormat,
    pub codec_type: VideoCodecType,
    pub raw: std::sync::OnceLock<CanonicalUnits>,
    pub avcc: std::sync::OnceLock<bytes::Bytes>,
    pub annexb: std::sync::OnceLock<bytes::Bytes>,
    pub rtp_payload: std::sync::OnceLock<PacketizedPayload>,
    pub flv_tag_body: std::sync::OnceLock<bytes::Bytes>,
}
```

原则：

- 首次访问时转换
- 之后只做缓存读取
- 只在有对应协议订阅者时才触发转换
- 一个关键帧的转换结果尽量被后续订阅者共享

### 5.9 协议转换边界

放在 foundation 层的：

- H264/H265/H266/AV1/VP8/VP9/AAC/Opus/G711 elementary stream
- RTP payload 打包与解包
- MPEG-PS/PES 解析
- FLV tag body helper
- MP4 sample / extradata helper
- Access Unit 拼装
- Timestamp / DTS / wrap / discontinuity 处理

不放在 foundation 层的：

- RTMP chunk/message 状态机
- RTSP DESCRIBE/SETUP/PLAY 状态机
- WebRTC ICE/DTLS/SRTP 会话状态机
- SIP/GBSip transaction/dialog
- HTTP/HLS 请求生命周期

即：foundation 知道 `RTP H264 payload`，但不知道 `RTSP SETUP`；foundation 知道 `FLV AVC tag body`，但不知道 `RTMP chunk stream`。

### 5.10 兼容优先的实现策略

#### 5.10.1 总原则

> 协议实现以“可与成熟 C++ 流媒体服务器simple-media-server[vendor-ref/simple-media-server]在真实设备和真实客户端上对齐”为目标。规范文档定义下限，兼容实践定义上线行为。

#### 5.10.2 入口宽松，内部规范，出口稳定

- ingress：尽量吃下历史包袱和不规整输入
- internal：统一转成 canonical form
- egress：始终输出可预测、可复现、可测的稳定格式

#### 5.10.3 CompatProfile

```rust
pub struct CompatProfile {
    pub protocol: ProtocolKind,
    pub vendor: Option<String>,
    pub device_model: Option<String>,
    pub client_family: Option<ClientFamily>,
    pub flags: CompatFlags,
}
```

例如：

- `ffmpeg`
- `obs`
- `vlc`
- `chrome`
- `safari`
- `hikvision`
- `dahua`
- `gb28181-gateway-x`
- `android-wechat-hls`

#### 5.10.4 已知兼容点前置到 foundation

常见问题不要散落到协议 core：

- 3/4 字节混合 start code
- 缺失 AUD
- SPS/PPS/VPS 只在流开始出现一次
- AAC 只有 ADTS 没有 ASC
- G711 没有可靠时间戳，只能按样本数推进
- RTP timestamp 跳变或回绕
- B 帧存在但没有 DTS
- H264/H265 access unit 边界不规整
- 配置帧与关键帧分离，需要自动补发
- 私有 payload type / clock rate 填错

### 5.11 Foundation 层数据流

```text
[Protocol Core]
    -> 提取 codec payload / timing metadata
    -> 调 cheetah-codec::ingress_normalize(...)
[cheetah-codec]
    -> CodecUnit
    -> AccessUnitAssembler
    -> TrackInfo update
    -> StampAdjust / DtsGenerator
    -> 输出统一 AVFrame
[Engine]
    -> RingBuffer / Dispatcher / Record / Cluster
[Egress Protocol]
    -> 请求 FrameView::AnnexB / Avcc / Adts / RtpPayload / FlvTagBody
    -> 由 cheetah-codec 导出
```

---

## 6. 核心数据引擎：无锁 RingBuffer

流媒体服务器的核心挑战是：**一份媒体数据，千人共享**。

传统方案要么为每个订阅者 clone 一份数据，带来 O(N) 内存与发送开销；要么通过 channel 逐一转发，调度开销随 N 线性增长。cheetah 选择无锁 SPMC RingBuffer 作为媒体热路径核心结构。

### 6.1 SPMC 无锁设计

```rust
pub struct RingBuffer {
    slots: Box<[RingSlot]>,
    write_pos: std::sync::atomic::AtomicUsize,
    idr_list: arc_swap::ArcSwap<Vec<IDRNode>>,
    idr_write_lock: parking_lot::Mutex<()>,
}

pub struct RingSlot {
    frame: arc_swap::ArcSwapOption<AVFrame>,
    version: std::sync::atomic::AtomicU64,
}
```

关键设计：

- **写入永不阻塞**：`fetch_add` 推进写位置
- **读取零拷贝**：返回 `Arc<AVFrame>`
- **版本号验证**：检测帧是否被覆写
- **IDR 索引**：帮助新订阅者 O(1) 定位最近关键帧

### 6.2 为什么不用 Channel 作为流存储

| 对比维度 | 传统 Channel | cheetah RingBuffer |
| --- | --- | --- |
| N 个订阅者内存 | O(N) | O(1) 共享 |
| 写入开销 | N 次 send | 1 次原子写入 |
| 锁竞争 | 高 | 低/无 |
| 时间回溯 | 不支持 | 支持从任意 IDR 开始 |

### 6.3 RingBuffer 的职责边界

RingBuffer 只负责：

- 存储最近一段统一 `AVFrame`
- 支持顺序读取和关键帧定位
- 允许多个订阅者共享同一份媒体帧

它不负责：

- 协议封装
- 订阅者发送逻辑
- 配置管理
- 跨流索引

### 6.4 IDR / GOP 追踪

RingBuffer 需要维护关键帧索引，支撑：

- 新订阅者从最近关键帧开始
- HLS/MP4 分段起点
- DVR/回看回溯
- 集群级联时的快速同步

`IDRNode` 至少记录：

- Ring position
- 对应 frame version
- dts_us / pts_us
- track 或 GOP 信息

---

## 7. Dispatcher：一次读取，万人共享

RingBuffer 解决了存储层的零拷贝，但还需要把帧高效通知给所有订阅者。Dispatcher 负责完成这一工作。

### 7.1 单流微观分发

```text
Publisher -> RingBuffer -> Dispatcher (单次读取)
                           |
         ┌──────┬──────────┼──────────┬──────┐
      Queue 1  Queue 2   Queue 3   ...   Queue N
      (bounded)(bounded)(bounded)        (bounded)
         |        |         |               |
      Writer1   Writer2   Writer3        WriterN
      (RTMP)    (WebRTC)  (HLS)          (FLV)
```

Dispatcher 从 RingBuffer **只读取一遍**，然后通过 `Arc::clone` 把帧广播到每个订阅者的独立 bounded channel。

### 7.2 背压控制

每个订阅者拥有独立的 bounded channel。某个订阅者网络拥塞导致队列满时：

- 使用 `try_send` 非阻塞投递
- 按策略丢弃可丢帧
- 慢订阅者不会拖累其他订阅者

建议的丢帧优先级：

1. 优先丢 B 帧 / P 帧
2. 尽量保留关键帧和必要配置帧
3. 控制消息、关键状态同步不能与普通媒体帧走同一丢弃策略

### 7.3 无锁订阅者管理

```rust
pub struct Dispatcher {
    subscribers: arc_swap::ArcSwap<Vec<DispatchSubscriber>>,
}
```

- 分发时通过原子 load 读取订阅者快照
- 增删订阅者通过 COW 模式更新订阅者数组
- 分发循环不因订阅者增删而阻塞

### 7.4 DispatcherPool：海量流分组

当流数量很多时，不适合为每一路流单独创建一个 dispatcher task。可以采用 DispatcherPool：

- `dispatcher_workers = 0`：每流一个 dispatcher，适合流数量较少
- `dispatcher_workers = N`：固定 N 个 worker，通过一致性哈希管理 M 路流

原则：

- 流少时优先最低延迟
- 流多时优先固定资源与减少上下文切换

---

## 8. 层级任务管理：Task Tree

为了替代缺乏父子结构与级联取消语义的“平铺任务模型”，系统采用层级任务树。

### 8.1 核心结构

```rust
pub struct Task {
    id: u64,
    task_type: TaskType,
    owner_type: String,
    token: cheetah_runtime_api::CancellationToken,
    children: parking_lot::Mutex<std::collections::HashMap<u64, std::sync::Arc<Task>>>,
    parent_id: Option<u64>,
    level: u8,
}
```

### 8.2 四种任务类型

| 类型 | 行为 | 典型场景 |
| --- | --- | --- |
| `Task` | 基础任务，独立运行 | 单次操作 |
| `Job` | 所有子任务退出时自动停止 | 批处理任务 |
| `Work` | 子任务退出后继续运行 | RTMP Server、StreamManager |
| `Channel` | 用于通信或管道 | 数据通道 |

### 8.3 级联取消

```text
Server (Work)
  └── StreamManager (Work)
        ├── RTMPServer (Work)
        │     └── TCPListener (Work)
        │           └── RTMPConnection (Task)
        │                 └── RTMPPublisher (Task)
        └── Stream "live/room1" (Job)
              ├── Dispatcher (Task)
              └── Subscriber-RTMP (Task)
```

父任务停止时，子任务应自动级联取消。通过 `#[track_caller]` 或等价手段记录创建位置，可以形成完整调用链，便于故障排查和运维观测。

---

## 9. 模块系统：26+ 独立 crate 的生态

模块系统不是简单接口抽象，而是完整的控制反转（IoC）与能力注入框架。

### 9.1 Module trait

```rust
#[async_trait::async_trait]
pub trait Module: Send + Sync {
    fn info(&self) -> ModuleInfo;
    fn state(&self) -> ModuleState;

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError>;
    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError>;
    async fn stop(&mut self) -> Result<(), SdkError>;

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError>;

    fn http_routes(&self) -> Vec<HttpRouteDescriptor> { Vec::new() }
    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> { None }
    fn http_mount_prefix(&self) -> Option<String> { None }
    fn http_max_body_bytes(&self) -> usize { 8 * 1024 * 1024 }
    fn http_request_timeout_ms(&self) -> Option<u64> { None }
}
```

对应类型统一使用：

- `ModuleInfo`
- `ModuleState`
- `ModuleManager`
- `Feature Modules`
- `Module SDK`

### 9.2 EngineContext：能力注入

```rust
pub struct EngineContext {
    pub runtime_api: std::sync::Arc<dyn RuntimeApi>,
    pub publisher_api: std::sync::Arc<dyn PublisherApi>,
    pub subscriber_api: std::sync::Arc<dyn SubscriberApi>,
    pub core_adapters_api: std::sync::Arc<dyn CoreAdaptersApi>,
    pub stream_manager_api: std::sync::Arc<dyn StreamManagerApi>,
    pub task_system_api: std::sync::Arc<dyn TaskSystemApi>,
    pub event_bus: std::sync::Arc<dyn EventBus>,
    pub config_provider: std::sync::Arc<dyn ConfigProvider>,
    pub config_apply_api: std::sync::Arc<dyn ConfigApplyApi>,
    pub module_manager_api: std::sync::Weak<dyn ModuleManagerApi>,
    pub room_service_api: std::sync::Arc<dyn RoomServiceApi>,
    pub metrics_api: std::sync::Arc<dyn MetricsApi>,
    pub health_api: std::sync::Arc<dyn HealthApi>,
    pub service_registry: std::sync::Arc<dyn ServiceRegistry>,
    pub database_api: std::sync::Arc<dyn DatabaseApi>,
    pub proxy_manager: std::sync::Arc<dyn ProxyManager>,
    pub cluster_api: std::sync::Arc<dyn ClusterApi>,
    pub ffmpeg_api: std::sync::Arc<dyn FfmpegApi>,
    pub media_services: MediaServices,
    pub media_session_directory: std::sync::Arc<dyn MediaSessionDirectoryApi>,
    pub media_data_plane: std::sync::Arc<dyn MediaDataPlaneApi>,
    pub media_file_store: std::sync::Arc<dyn MediaFileStoreApi>,
    pub media_event_bus: std::sync::Arc<dyn MediaEventBusApi>,
    pub control_auth_api: std::sync::Arc<dyn ControlAuthApi>,
    pub audit_api: std::sync::Arc<dyn AuditApi>,
}
```

这样可以做到：

- 第三方开发者只需依赖 `cheetah-sdk`
- 协议/功能模块无需直接依赖引擎内部实现
- 能力以 trait object 暴露，方便 Mock 与替换

### 9.3 模块加载方式

- 静态编译
- 动态库（`.so/.dylib/.dll`）
- WASM（预留）

第一阶段建议优先静态编译；动态库和 WASM 作为后续扩展能力。

### 9.4 Feature Modules 矩阵

#### 协议模块

- RTMP
- RTSP
- HLS
- FLV
- WebRTC
- SRT
- GB28181
- WebTransport

#### 录制/媒体模块

- MP4
- Snap
- SEI
- Mix
- Transcode
- Crypto

#### 房间/场景模块

- Live
- Meeting

#### 设备模块

- ONVIF
- HomeKit
- V4L2
- ALSA

#### 运维模块

- Cluster
- Debug
- Report
- LogRotate
- Crontab

### 9.5 Cargo Feature 组织

```bash
# 完整能力组合（media-control-full 等价展开）
cargo build --release -p cheetah-server --features media-control-full

# 显式展开验证（避免 profile 漏依赖）
cargo check -p cheetah-server --no-default-features \
  --features 'rtmp,rtsp,http-flv,hls,ts,fmp4,mp4,rtp,gb28181,record,webrtc,srt,proxy'
```

支持按模块维度启用编译，降低二进制大小与构建成本。`media-control-full` 一键启用所有已交付的媒体控制模块。`cheetah-connector` 完整能力测试需显式 `--features full`。

---

## 10. 协议模块总体策略

所有协议模块都遵循同一套设计准则：

1. 统一拆分为 `core + driver + module`
2. 进入引擎前统一收敛为 `AVFrame + TrackInfo`
3. 尽量把兼容修正上收到 `cheetah-codec`
4. 协议实现要对齐真实客户端/设备互操作，而不是只“抄标准”
5. 热路径坚持单线程分片、无锁读、多订阅共享

### 10.1 RTMP

- `cheetah-rtmp-core`：chunk/message、握手、控制消息、发布/播放协议状态
- `cheetah-rtmp-driver-tokio`：TCP socket、分块收发、超时、异步写队列
- `cheetah-rtmp-module`：对接发布、拉流、鉴权、转推、房间、管理面
- 落地依赖方向：`cheetah-rtmp-module -> cheetah-rtmp-driver-tokio -> cheetah-rtmp-core -> cheetah-codec`
- runtime 抽象落点：`cheetah-rtmp-driver-tokio` 直接依赖 `cheetah-runtime-api`，`cheetah-rtmp-module` 不直接依赖 `cheetah-rtmp-core`
- `cheetah-rtmp-module` 的生命周期控制面与并发控制原语使用 SDK/runtime 抽象，不直接暴露 Tokio 类型

当前落地配置（`modules.rtmp`）：

- `enabled`：是否启用 RTMP module（默认 `true`）
- `listen`：RTMP TCP 监听地址（默认 `0.0.0.0:1935`）
- `write_queue_capacity`：单连接异步写队列上限（默认 `256`）
- `play_wait_source_timeout_ms`：播放请求等待“流上线”的超时（默认 `15000`）；仅在源持续不存在时生效，`0` 表示不主动超时拒绝
- `subscriber_queue_capacity`：订阅端缓冲队列上限（默认 `256`）
- `subscriber_backpressure`：订阅端背压策略（默认 `DropUntilNextKeyframe`）
- `bootstrap_max_frames`：订阅端 bootstrap 帧上限（默认 `150`）
- `enable_add_mute`：当流无音频轨时，是否补静音 AAC 帧（默认 `false`，用于兼容部分播放器）

`cheetah-server` 通过 Cargo feature `rtmp` 注册 `RtmpModuleFactory`（当前默认启用）。

重点：

- FLV/RTMP 中的 AVC/AAC sequence header 统一转入 `TrackInfo`
- 视频帧尽快规范化为 H26x Access Unit
- 音频帧尽快规范化为 AAC raw / G711 / Opus 等 canonical form
- 拉流 URL query 支持 `type=enhanced` / `type=fastPts` 兼容播放模式

### 10.2 RTSP

- `cheetah-rtsp-core`：Sans-I/O 控制面与语义层（request/response、interleaved、RTP/RTCP、Transport/Session/Range/RTP-Info/SDP/auth 解析）
- `cheetah-rtsp-driver-tokio`：TCP/UDP I/O、interleaved 收发、HTTP tunnel（GET/POST + cookie + base64）、multicast endpoint、outbound client 驱动
- `cheetah-rtsp-module`：服务端 publish/play 编排、鉴权与租约、RTSP pull/push/relay job 监督、控制面集成

重点：

- 传输矩阵完整支持：RTP over UDP unicast、RTP over TCP interleaved、RTP over HTTP tunnel、RTP multicast（PLAY）
- 服务能力覆盖：服务端推流（ANNOUNCE/RECORD）、服务端播放（DESCRIBE/SETUP/PLAY）、远端拉流/推流与 relay
- RTP/RTCP payload 处理、时间戳归一化与 reorder 等媒体内核能力优先复用 `cheetah-codec`
- 对接摄像头与 NVR 时兼容非标准 SDP / payload / transport 输入，保持 bounded robustness（不 panic、不无界缓存）

### 10.3 HTTP-FLV / FLV

- `cheetah-flv-core`：FLV tag 封装/拆解、metadata 行为
- `cheetah-flv-driver-tokio`：HTTP 输出与连接驱动
- `cheetah-flv-module`：HTTP 路由、鉴权、回看/直播接入

### 10.4 HLS

- `cheetah-hls-core`：分片、播放列表、切片状态机
- `cheetah-hls-driver-tokio`：HTTP 服务与缓存调度
- `cheetah-hls-module`：录制、边缘缓存、回放、管理面集成

重点：

- Annex-B 输出由 `cheetah-codec` 导出
- AAC/ADTS、PAT/PMT、PCR/PTS 对齐要以 foundation 层时间系统为基础
- 兼容一些客户端对实时 HLS 的特殊需求

### 10.5 WebRTC

WebRTC 应以 `str0m` 的 Sans-I/O 思路为参考，实现服务端取向的纯状态机核心。

```text
cheetah-webrtc-core
cheetah-webrtc-driver-tokio
cheetah-webrtc-module
```

职责划分：

- `webrtc-core`：ICE / STUN / DTLS / SRTP / RTCP / NACK / TWCC / jitter / pacing 等协议语义
- `webrtc-driver-tokio`：UDP socket、timer、task、命令通道
- `webrtc-module`：WHIP/WHEP、浏览器接入、房间系统、与 Dispatcher / StreamManager 对接

设计原则：

- core 内部不直接依赖 Tokio
- 时间由 driver 显式注入
- 缓存与 NACK 队列全部有界
- 尽量就地处理 RTP/SRTP 包，减少复制

### 10.6 SRT

```text
cheetah-srt-core
cheetah-srt-driver-tokio
cheetah-srt-module
```

落地策略分两阶段：

- Phase 1：优先接第三方纯 Rust 实现，外层封装统一 `SrtProvider`
- Phase 2：若热路径、可控性、互操作不满足要求，再逐步内收为完整 Sans-I/O 三段式

### 10.7 GBSip（GB28181 信令）

GB28181 不是单协议，而是**信令面 + 媒体面**的组合。

- 信令面：`GBSip`
- 媒体面：RTP/PS，统一接入 `cheetah-codec` 的 RTP/PS 管线

命名上使用 `GBSip`，避免与通用 SIP 栈概念混淆。

#### 10.7.1 统一三段式

```text
cheetah-gbsip-core
cheetah-gbsip-driver-tokio
cheetah-gb28181-module
```

#### 10.7.2 cheetah-gbsip-core

`gbsip-core` 是纯同步、Sans-I/O 的协议状态机，只负责：

- `message`：基于 `rsip` 的 SIP 报文解析与构造适配
- `transaction`：`REGISTER`、`MESSAGE`、`INVITE`、`ACK`、`BYE` 及响应状态机
- `dialog`：`Call-ID`、`From/To Tag`、`CSeq`、route set 生命周期
- `auth`：Digest challenge / verify 协议态
- `manscdp`：GB28181 XML/MANSCDP 解析与构造
- `session`：设备注册、保活、目录、邀请的协议语义
- `timer`：事务重传、超时、过期管理

它不能：

- 直接持有 UDP/TCP socket
- 直接使用 Tokio
- 直接获取系统时钟
- 直接操作数据库或引擎服务

#### 10.7.3 cheetah-gbsip-driver-tokio

负责：

- UDP SIP 收发
- TCP SIP 承载与按 `Content-Length` 组帧
- timer 适配
- command/event channel
- 驱动循环：socket/timer -> core -> socket/timer

#### 10.7.4 cheetah-gb28181-module

负责：

- 对接 `EngineContext`
- 设备注册表与鉴权数据源
- 国标目录、设备信息、保活业务
- 实时流/回放流邀请
- 媒体端口、SSRC、payload type 分配
- 将 GBSip 事件映射到 `StreamManager` 与 RTP/PS 接入动作

#### 10.7.5 与媒体面的边界

`GBSip core` 不直接处理 RTP 包，也不承担媒体收发循环；`gb28181-module` 在 `INVITE/200 OK/ACK` 完成后，把会话绑定到对应的 RTP/PS 媒体接入实例。

### 10.8 WebTransport

- `cheetah-webtransport-core`：会话语义、数据通路规则
- `cheetah-webtransport-driver-tokio`：QUIC session 驱动
- `cheetah-webtransport-module`：浏览器/边缘接入、数据流与房间/控制面整合

### 10.9 录制、转码、截图

录制、转码、截图、SEI、混流等属于模块级能力，但不要侵入协议 core 和 `cheetah-codec` 热路径。

- FFmpeg 仅放在系统外缘或工作任务中使用
- `ffmpeg-next` 只作为 Rust 侧桥接接口
- 解码/转码/录制属于 `Job/Work` 型任务，不直接挤入每包必经路径

---

## 11. 引擎内建服务

### 11.1 StreamManager

职责：

- 流实例生命周期管理
- 发布者/订阅者注册
- RingBuffer / Dispatcher 创建与回收
- 流级别元数据、统计、状态维护
- 对接录制、转发、房间、集群等上层能力

### 11.2 ModuleManager

职责：

- 模块发现、注册、初始化、启动、停止
- 生命周期管理
- Feature 列表与运行状态管理
- 动态库/WASM（后续）加载入口

### 11.3 RoomService

职责：

- 直播间 / 会议室抽象
- 参与者状态
- 多路流与会话管理
- RTC/直播互动业务编排
- 对外提供稳定基础契约（如 `get_room`、`bind_stream`、`unbind_stream`）

### 11.4 TaskSystem

职责：

- Task Tree 管理
- 级联取消
- 任务统计与监控
- 调试追踪与调用链记录

### 11.5 CoreAdapters

负责在引擎层屏蔽不同协议 module 与统一媒体内核之间的接缝，包括：

- 发布/订阅适配
- 协议事件到引擎动作的映射
- `TrackInfo` / `AVFrame` 到流实例的绑定

---

## 12. 集群架构：基于 QUIC 的低延迟级联

集群方案选择 QUIC 作为节点间通信基础。

```text
         ┌─────────────────────┐
         │    Origin Cluster   │
         │  ┌──────┐  ┌──────┐ │
         │  │Node 1│◄►│Node 2│ │
         │  └──┬───┘  └──┬───┘ │
         └─────┼─────────┼─────┘
               │   QUIC   │
      ┌────────┼──────────┼────────┐
      │        ▼          ▼        │
      │   ┌─────────┐ ┌─────────┐  │
      │   │ Edge 1  │ │ Edge 2  │  │
      │   └────┬────┘ └────┬────┘  │
      │        │           │       │
      │     Clients     Clients    │
      └────────────────────────────┘
```

选择 QUIC 的原因：

- 0-RTT / 低时延重连潜力
- 多路复用，减少队头阻塞
- 内置 TLS 1.3
- 同时支持可靠 stream 与不可靠 datagram
- 适合集群控制同步与低延迟媒体级联并存的场景

建议：

- 控制同步、元数据、目录管理走 QUIC stream
- 低延迟媒体级联、探测与补充信息可考虑 QUIC datagram

---

## 13. 配置管理

采用五层优先级覆盖机制：

```text
运行时 API 修改 (最高)
    ↓
数据库配置
    ↓
环境变量 (CHMS_ 前缀)
    ↓
YAML 配置文件
    ↓
模块默认值 (最低)
```

### 13.1 配置特性

- 支持 RESTful API 动态修改
- 支持热更新与实时生效
- 支持 WebSocket 推送配置变更
- 支持 SQLite / MySQL / PostgreSQL 持久化
- 通过派生宏自动生成 schema、默认值与校验逻辑

### 13.2 配置边界

- 协议 core 不直接读取配置源
- 配置由 module 或引擎服务注入
- 热更新要区分：立即生效 / 新会话生效 / 需重启模块生效

---

## 14. TLS、密码学与 FFmpeg

### 14.1 TLS / DTLS / SRTP 后端可插拔

借鉴现代 Rust 网络库的 provider 思路：

- TLS：优先 `rustls + rustls-pemfile`
- 密码学：`ring` 或 RustCrypto
- DTLS / SRTP：按 provider 抽象，允许切换底层后端

目标：

- 控制面、集群面与媒体面加密能力可按场景替换
- 尽量避免把某个具体加密后端写死到协议 core 中

### 14.2 FFmpeg 作为边缘能力

`ffmpeg-next` 作为 Rust 接口层接入，但 FFmpeg 原生库依赖不应侵入热路径。

建议：

- 转码、截图、录制、封装转换作为 `Job/Work` 型任务
- `cheetah-codec` 只定义统一压缩媒体语义，不绑定 FFmpeg 生命周期
- 协议模块与引擎热路径不直接依赖 FFmpeg

---

## 15. 性能与工程实现原则

### 15.1 热路径单线程、分片归属固定

按 `stream_id / 5-tuple / session_id` 做一致性哈希，把会话固定到某个 worker：

- worker 内部用本地 `HashMap / slab`
- 尽量避免热路径频繁碰 `DashMap`
- `DashMap` 更适合控制面和跨线程目录索引

### 15.2 去锁化 by design，但允许冷路径上锁

原则不是“系统里绝不能有锁”，而是：

- 热路径不出现高竞争 mutex
- worker 内局部状态尽量用 `&mut self`
- 配置更新、模块注册、任务树管理可以上锁
- 读多写少的数据优先考虑 `ArcSwap / COW`

### 15.3 原地处理与对象池

- 收包缓冲优先使用 `BytesMut` / 预分配 `Vec<u8>`
- 解析尽量基于切片视图，不重复组装 `String`/`Vec`
- 解密、解封装尽量就地处理
- MTU 级 buffer pool
- RTP header extension、NACK 队列、jitter buffer 统一有界

### 15.4 Backpressure 分层

背压控制要分层：

- 会话级
- 协议级
- 流级
- 订阅者级

控制面消息、关键状态同步与普通媒体帧不应共用同一丢帧策略。

### 15.5 系统调用批处理与 Linux 优化

预留 Linux 优化口：

- `recvmmsg / sendmmsg`
- GRO / GSO
- `SO_REUSEPORT`
- CPU 亲和性
- 线程/worker 固定核

### 15.6 目标型性能指标

以下指标为设计目标，最终以基准测试和生产实测为准：

| 指标 | 目标值 | 说明 |
| --- | --- | --- |
| RingBuffer 写入延迟 | 约百纳秒级/次 | 原子写入 + `ArcSwap` |
| RingBuffer 读取延迟 | 数十纳秒级/次 | `Arc<AVFrame>` 零拷贝共享 |
| Dispatcher 分发延迟 | 亚毫秒至毫秒级 | 单次读取 + 非阻塞广播 |
| 单流并发订阅者 | 10,000+ 级别 | 依赖共享帧与背压控制 |
| 单节点并发流 | 10,000+ 级别 | 依赖 DispatcherPool 与资源配置 |

---

## 16. 兼容性策略：与成熟 C++ 实现对齐

实际流媒体系统的成功标准，不是“只实现标准协议”，而是：

- 与 FFmpeg / OBS / VLC / 浏览器互通
- 与各类 IPC / NVR / GB28181 设备互通
- 对不完全标准、带历史包袱、字段缺失或顺序异常的数据流有容忍度

### 16.1 对齐原则

1. **规范文档定义下限**：不破坏协议基本语义。
2. **成熟实现定义上线行为**：优先对齐成熟 C++ 流媒体服务器simple-media-server[vendor-ref/simple-media-server]的互操作行为。
3. **兼容逻辑优先下沉到 foundation**：减少协议 core 中散落 patch。
4. **输入宽容、输出稳定**：入口尽量兼容，内部统一，出口稳定可控。

### 16.2 兼容性测试矩阵

测试不仅覆盖“是否符合标准”，还要覆盖：

- FFmpeg 推流 / 拉流
- OBS 推流
- VLC 播放
- Chrome / Safari / 移动端浏览器 WebRTC
- 主流 IPC 厂商 RTSP / GB28181 接入
- HLS 在 Android / iOS / 微信内置环境下播放
- 不同 GOP / 时间戳异常 / 参数集缺失 / 非标准 SDP 的流

### 16.3 兼容修正落点

优先级建议：

1. 能落在 `cheetah-codec` 的，先落在 foundation
2. 协议打包/解包层特有修正，落在对应 `core`
3. 与业务相关的灰度兼容，落在 `module`

---

## 17. 技术栈建议

| 组件 | 技术选型 | 说明 |
| --- | --- | --- |
| 异步运行时 | Tokio | 第一阶段主运行时 |
| 无锁原语 | `arc-swap` | 订阅者列表、IDR 列表无锁读 |
| 互斥锁 | `parking_lot` | 轻量高性能锁 |
| 并发 Map | `DashMap` | 控制面、目录索引 |
| 零拷贝字节 | `bytes` | 引用计数字节缓冲 |
| WebRTC 参考 | `str0m` 思路 | Sans-I/O 设计参考 |
| QUIC | `quinn` | 集群节点间低延迟通信 |
| SIP 解析 | `rsip` | 仅做报文解析/构造适配 |
| SRT | `shiguredo_srt` 或等价纯 Rust 方案 | 优先纯 Rust |
| TLS | `rustls` + `rustls-pemfile` | 控制面与集群面 TLS |
| 密码学 | `ring` / RustCrypto | 可切换后端 |
| 序列化 | `serde` + `prost` | JSON/YAML + Protobuf |
| 日志 | `tracing` | 结构化日志与 span |
| FFmpeg 绑定 | `ffmpeg-next` | 边缘能力接入 |

---

## 18. 全栈交付形态

cheetah 不只是一个流媒体引擎，还包括完整的交付面：

```text
┌───────────────────────────────────────────┐
│  Rust 核心引擎                             │
│  高性能流媒体处理 · 多协议/功能模块         │
├───────────────────────────────────────────┤
│  React + TypeScript Admin 管理后台         │
│  实时监控 · 流管理 · 模块配置 · 设备管理     │
├───────────────────────────────────────────┤
│  TypeScript + Web Components SDK          │
│  浏览器端推拉流 · WebRTC · 直播间 · 互动     │
├───────────────────────────────────────────┤
│  文档网站                                   │
│  概念 · 协议 · 模块 · API · 进阶指南         │
└───────────────────────────────────────────┘
```

---

## 19. 实施阶段建议

### Phase 1：基础骨架

- `cheetah-codec` 基础对象、时间系统、H264/H265/AAC/G711/Opus 最小能力
- `cheetah-sdk`、`ModuleManager`、`StreamManager`、`TaskSystem`
- RingBuffer、Dispatcher、DispatcherPool
- Tokio runtime 抽象首版

#### Phase 1（基础架构补完）最小完成标准

- `ModuleManager` 必须支持：
  - 拓扑顺序初始化 / 启停
  - 模块 HTTP Service 挂载信息导出（SDK 契约保持框架无关，Axum 仅作为 control 适配层）
  - 配置变更后的 `apply_module_config_change / apply_module_config_changes`
  - 当模块配置返回 `ModuleRestartRequired` 时，基础层执行“重建实例”重启（`create -> init -> start`）
  - `restart_module / restart_modules` 仅允许对 `Running` 模块执行；非 `Running` 状态返回 `Conflict`
- `EngineContext` 必须是完整能力注入：
  - 包含 `runtime_api`、`publisher_api`、`subscriber_api`、`core_adapters_api`
  - 包含 `config_provider` 与 `config_apply_api`
  - 包含 `service_registry`、`database_api`、`proxy_manager`、`cluster_api`、`ffmpeg_api`
  - 包含 `media_services`、`media_session_directory`、`media_data_plane`、`media_file_store`、`media_event_bus`、`control_auth_api`、`audit_api`
- `control` 必须形成闭环：
  - `PATCH /api/v1/config` 与 `PATCH /api/v1/config/modules/:module_id` 在写入配置后，触发模块配置应用
  - 对外返回模块配置应用报告（effect 列表）
- `StreamManager` 必须同时具备：
  - `PerStream` 与 `SharedPool` 两种分发模式
  - `StreamKey` 单发布者独占语义（同一 `StreamKey` 同时只有一个活跃发布者）
  - 有界分发队列
  - 可区分的背压策略（`DropDroppableFirst / DropUntilNextKeyframe / DisconnectOnOverflow`）
  - 订阅端 bootstrap 帧数量上界
  - 统一的 `StreamSnapshot` 读接口（`list_streams/get_stream`），包含发布态与 tracks 元信息
- `TaskSystem` 必须具备：
  - 父子任务级联取消
  - 任务完成态上报（Succeeded / Failed / Cancelled）
  - 可观测的任务时间戳与完成信息
- 事件总线必须采用结构化事件体（模块、流、任务、配置、系统），避免自由字符串协议漂移。
- `control` 模块 HTTP 适配必须向 module 透传相对 `routes_prefix` 的规范化路径（挂载根路径为 `/`）。

### Phase 2：协议首批落地

- RTMP 三段式
- RTSP 三段式
- FLV / HLS 输出
- GBSip 三段式与 RTP/PS 接入

### Phase 3：RTC 与集群

- WebRTC Sans-I/O core
- QUIC 集群
- SRT 接入
- 房间/会议能力

### Phase 4：媒体能力增强

- MP4、录制、截图、SEI、转码、混流
- 多平台兼容矩阵完善
- 动态库 / WASM 模块形态探索

---

## 20. 总结

cheetah 的核心回答只有一句话：

> **把协议、运行时、媒体语义和业务接入彻底分层，让热路径尽量无锁、零拷贝、可共享、可兼容。**

落到工程上，就是四个统一：

1. **统一六层架构**：基础、SDK、模块、引擎、基础设施、客户端职责清晰
2. **统一协议三段式**：所有协议都必须遵循 `core + driver + module`
3. **统一媒体内核**：所有媒体数据统一收敛为 `AVFrame + TrackInfo`
4. **统一兼容策略**：规范定义下限，成熟实现与真实互操作定义上线行为

最终目标不是做出“又一个协议库集合”，而是做出一套能够长期承载直播、会议、安防、边缘接入、转码录制与集群分发的高性能流媒体服务器架构。

---

## 参考与设计来源

以下内容用于架构设计参考与方向对齐：

- str0m: <https://github.com/algesten/str0m>
- webrtc-rs: <https://github.com/webrtc-rs/webrtc>
- quinn: <https://github.com/quinn-rs/quinn>
- rsip: <https://crates.io/crates/rsip>
- shiguredo_srt: <https://crates.io/crates/shiguredo_srt>
- rustls: <https://github.com/rustls/rustls>
- ffmpeg-next: <https://crates.io/crates/ffmpeg-next>
- simple-media-server: <https://gitee.com/inyeme/simple-media-server>
