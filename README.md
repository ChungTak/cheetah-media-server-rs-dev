# Cheetah Media Server

这是 `cheetah` 流媒体服务器的仓库根入口文档。下面按“启动、改配置、运行、测试、发布”给出最短可执行流程。

## 1. 启动程序

默认启动应用入口：

```bash
cargo run -p cheetah-server
```

当前 `apps/cheetah-server` 默认启用 `rtmp` feature。`rtsp`、`http-flv` 作为可选 feature，需要显式打开：

```bash
cargo run -p cheetah-server --features rtsp
```

启用 HTTP-FLV module：

```bash
cargo run -p cheetah-server --features http-flv
```

启用 HLS module：

```bash
cargo run -p cheetah-server --features hls
```

启用 fMP4 module：

```bash
cargo run -p cheetah-server --features fmp4
```

同时启用 RTSP + HTTP-FLV + HLS + fMP4：

```bash
cargo run -p cheetah-server --features "rtsp,http-flv,hls,fmp4"
```

启用可选媒体处理模块（依赖 avcodec-rs，需要系统已安装 FFmpeg 和 OpenCV 开发库）：

```bash
cargo run -p cheetah-server --features media-processing
```

启用 signaling control plane（当前为 MIG-01 占位骨架，包含 gRPC adapter 与 SQLite control-plane 依赖）：

```bash
cargo run -p cheetah-server --features signaling-control-plane
```

如果你想只启用 RTSP、禁用默认 RTMP：

```bash
cargo run -p cheetah-server --no-default-features --features rtsp
```

程序启动后会：

1. 初始化 Tokio runtime
2. 加载配置
3. 构建 engine
4. 启动 control 服务
5. 按已启用的 module 启动 RTMP / RTSP / 媒体处理服务

## 2. 修改配置

程序的配置加载顺序是：

1. 代码内默认值
2. `CHEETAH_CONFIG` 指向的 YAML 文件
3. `M7S_` 前缀环境变量

### 2.1 配置文件

最小可用配置示例见 [config.example.yaml](/dataset/datavol/workspace/media_server/cheetah-media-server-rs/config.example.yaml)：

```yaml
global:
  control:
    listen: 127.0.0.1:8891
modules:
  rtmp:
    enabled: true
    listen: 0.0.0.0:1935
```

启用 `signaling-control-plane` 时的最小配置示例：

```yaml
global:
  control:
    listen: 127.0.0.1:8891
modules:
  rtmp:
    enabled: true
    listen: 0.0.0.0:1935
  signaling_control_plane:
    enabled: true
    grpc:
      listen: 127.0.0.1:9090
    store:
      path: /var/lib/cheetah/signaling.db
    registry:
      node_identity: node-1
```

同时启用 RTSP 的示例：

```yaml
global:
  control:
    listen: 127.0.0.1:8891
modules:
  rtmp:
    enabled: true
    listen: 0.0.0.0:1935
    write_queue_capacity: 256
    subscriber_queue_capacity: 256
    bootstrap_max_frames: 150
    enable_add_mute: false
    emit_play_metadata: true
  rtsp:
    enabled: true
    listen: 0.0.0.0:554
    session_timeout_secs: 60
    multicast:
      enabled: false
    pull_jobs: []
    push_jobs: []
    relay_jobs: []
  http_flv:
    enabled: true
    listen: 0.0.0.0:8080
    write_queue_capacity: 256
    read_buffer_size: 65536
    pull_jobs:
      - name: remote_http_flv
        enabled: false
        source_url: http://127.0.0.1:8081/live/in.flv
        target_stream_key: live/remote_in
        retry_backoff_ms: 500
        max_retry_backoff_ms: 5000
  hls:
    enabled: true
    listen: 0.0.0.0:8088
    segment_duration_ms: 4000
    segment_count: 5
    session_timeout_secs: 10
```

RTSP 默认行为说明：

1. `multicast.enabled` 默认 `false`，不主动开启 RTP multicast PLAY。
2. `pull_jobs` / `push_jobs` / `relay_jobs` 默认空数组，后台任务默认不启动。
3. HTTP tunnel 属于 RTSP 传输能力，默认配置不主动发起；仅在启用对应 pull/push/relay 任务并选择 `transport_preference` 后才会使用。

配置文件加载方式：

```bash
export CHEETAH_CONFIG=./config.yaml
cargo run -p cheetah-server --features "rtsp,http-flv,hls,fmp4,ts,rtp,gb28181"
```

### 2.2 环境变量覆盖

环境变量前缀固定为 `M7S_`，规则是：

1. 全局配置使用 `M7S_GLOBAL__...`
2. 模块配置使用 `M7S_MODULE__<module>__...`
3. 层级字段之间用双下划线 `__` 分隔

示例：

```bash
export M7S_GLOBAL__control__listen=0.0.0.0:8891
export M7S_MODULE__rtmp__listen=0.0.0.0:1935
export M7S_MODULE__rtmp__bootstrap_max_frames=1024
```

值类型会做基础解析：

1. `true` / `false` 会转成布尔值
2. 整数会转成整数
3. 浮点数会转成浮点数
4. 其他值按字符串处理

### 2.3 热修改语义

模块配置修改后，如果配置值发生变化，RTMP / RTSP module 会返回 `ModuleRestartRequired`。这表示配置已接受，但模块需要由基础层按生命周期重建，不要在 module 内自行绕过这个语义。

## 3. 运行

### 3.1 看日志

程序使用 `tracing_subscriber`，可以直接通过 `RUST_LOG` 控制日志级别：

```bash
RUST_LOG=info cargo run -p cheetah-server
```

### 3.2 RTMP 拉流与推流

推流示例：

```bash
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c copy -f flv rtmp://127.0.0.1:1935/live/test
```

拉流示例：

```bash
export SDL_VIDEODRIVER=dummy
export SDL_AUDIODRIVER=dummy
ffplay rtmp://127.0.0.1:1935/live/test
```

### 3.3 常见检查点

1. `rtmp.listen` 是否真的绑定成功
2. `control.listen` 是否冲突
3. 配置文件是否被 `CHEETAH_CONFIG` 正确加载
4. `M7S_` 环境变量是否覆盖了 YAML 文件

### 3.4 HTTP-FLV 播放 URL

当启用 `http-flv` feature 且 `modules.http_flv.enabled=true` 后：

```text
HTTP-FLV: http://127.0.0.1:8080/{app}/{stream}.flv
WS-FLV:   ws://127.0.0.1:8080/{app}/{stream}.flv
```

例如 RTMP 推流到 `rtmp://127.0.0.1:1935/live/test` 后，可直接拉：

```bash
ffplay http://127.0.0.1:8080/live/test.flv
```

### 3.5 HLS 播放 URL

当启用 `hls` feature 且 `modules.hls.enabled=true` 后：

```text
Master:  http://127.0.0.1:8088/{app}/{stream}.m3u8
Media:   http://127.0.0.1:8088/{app}/{stream}/index.m3u8
Segment: http://127.0.0.1:8088/{app}/{stream}/{seg_name}.ts
```

例如 RTMP 推流到 `rtmp://127.0.0.1:1935/live/test` 后，可直接拉：

```bash
ffplay http://127.0.0.1:8088/live/test.m3u8
```

也可用 hls.js 在浏览器中播放：

```html
<script src="https://cdn.jsdelivr.net/npm/hls.js@latest"></script>
<video id="video"></video>
<script>
  var hls = new Hls();
  hls.loadSource('http://127.0.0.1:8088/live/test.m3u8');
  hls.attachMedia(document.getElementById('video'));
</script>
```

HLS 支持的编码格式：H264、H265、VP8、VP9、AV1（视频）；AAC、G711A、G711U、MP3、OPUS（音频）。

### 3.6 Low-Latency HLS (LLHLS)

当 `container: fmp4` 且 `ll_hls_enabled: true`（默认）时，自动启用 LLHLS 模式。

**LLHLS 特有的 URL：**

```text
Part:    http://127.0.0.1:8088/{app}/{stream}/part_{seq}.m4s
Init:    http://127.0.0.1:8088/{app}/{stream}/init.mp4
Player:  http://127.0.0.1:8088/{app}/{stream}/
```

**内嵌播放页：** 浏览器访问 `http://127.0.0.1:8088/live/test/` 即可直接播放（内嵌 hls.js，自动开启 LLHLS 低延迟模式）。

**使用 hls.js LLHLS 模式：**

```html
<script src="https://cdn.jsdelivr.net/npm/hls.js@latest"></script>
<video id="video"></video>
<script>
  var hls = new Hls({
    lowLatencyMode: true,
    liveSyncDurationCount: 3,
    liveMaxLatencyDurationCount: 6,
  });
  hls.loadSource('http://127.0.0.1:8088/live/test.m3u8');
  hls.attachMedia(document.getElementById('video'));
</script>
```

**LLHLS 配置说明：**

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `container` | `"ts"` | 设置为 `"fmp4"` 以启用 LLHLS |
| `ll_hls_enabled` | `true` | LL-HLS 开关（fMP4 模式下生效） |
| `part_target_ms` | `200` | Part 目标时长（毫秒），越小延迟越低 |
| `cdn_secret` | `""` | CDN Bearer Token（为空禁用 CDN 认证） |
| `segment_duration_ms` | `4000` | 完整 segment 时长 |

**CDN 模式：** 配置 `cdn_secret` 后，CDN 边缘节点通过 `Authorization: Bearer <secret>` 认证，playlist 响应不设置 `no-cache`（允许 CDN 缓存）。

**Session 标识：** 支持 Cookie（`HLS_SESSION`）和 URL 查询参数（`?session=`）两种模式，兼容不支持 cookie 的播放器。

**生成的 Playlist 包含的 LLHLS 标签：**
- `EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=...`
- `EXT-X-PART-INF:PART-TARGET=...`
- `EXT-X-PART:DURATION=...,URI="...",INDEPENDENT=YES`
- `EXT-X-PRELOAD-HINT:TYPE=PART,URI="..."`
- `EXT-X-PROGRAM-DATE-TIME:...`

### 3.7 HTTP-fMP4 / WS-fMP4 模块使用指南

当启用 `fmp4` feature 且 `modules.fmp4.enabled=true` 后，服务器支持通过 HTTP/WebSocket 进行 Fragmented MP4 (fMP4) 格式的实时媒体流式发布与播发。

**工作原理与核心特性：**
- **Sans-I/O 架构解耦**：`Fmp4Core` 纯状态机设计，不持有 socket、timer 或异步任务，只关注 HTTP 请求解析、CORS 头部处理、WebSocket 协议升级与帧传输，确保 100% 协议正确性与极速状态流转。
- **fMP4 实时流式封装**：通过 `cheetah-codec` 将底层流媒体引擎注入的 `AVFrame` 统一归一化，自适应重构为包含 `moof` (Movie Fragment) 箱体和 `mdat` (Media Data) 箱体的 Fragmented MP4 格式字节流。
- **WebSocket 极速传输**：原生支持将生成的 fMP4 初始化片段（`ftyp` + `moov`）和连续媒体数据包以 WebSocket 数据帧形式流式推送至前端，避免了本地庞大临时文件的磁盘 I/O 损耗，完美兼容前端 `MSE (Media Source Extensions)`、`dash.js` 或自定义播放器。
- **起播 GOP 追赶缓存 (Bootstrap)**：支持在播放客户端连接时自动检索最近的关键帧并补发必要的配置与初始化元数据，保证画面首屏秒开体验。

**播放 URL：**
```text
HTTP-fMP4:  http://127.0.0.1:8083/{app}/{stream}.mp4
WS-fMP4:   ws://127.0.0.1:8083/{app}/{stream}.mp4
```

ZLMediaKit/SMS 兼容 URL（支持 `.live.mp4` 后缀）：
```text
HTTP-fMP4:  http://127.0.0.1:8083/{app}/{stream}.live.mp4
WS-fMP4:   ws://127.0.0.1:8083/{app}/{stream}.live.mp4
```

例如，向 RTMP 模块推流到 `rtmp://127.0.0.1:1935/live/test` 后，可以直接通过 ffplay 播放 fMP4 流：
```bash
ffplay http://127.0.0.1:8083/live/test.mp4
```

**HTTPS/WSS-fMP4 (TLS 模式)：**
```text
HTTPS-fMP4: https://127.0.0.1:8446/{app}/{stream}.mp4
WSS-fMP4:   wss://127.0.0.1:8446/{app}/{stream}.mp4
```

**fMP4 模块配置示例：**
```yaml
modules:
  fmp4:
    enabled: true
    listen: 0.0.0.0:8083
    write_queue_capacity: 256
    read_buffer_size: 65536
    subscriber_queue_capacity: 256
    bootstrap_max_frames: 150
    play_wait_source_timeout_ms: 15000
    max_tracks: 32
    max_box_bytes: 4194304
    max_fragment_duration_ms: 1000
    force_fragment_on_keyframe: true
    include_styp: true
    include_sidx: true
    demand_mode: false
    tls:
      enabled: false
      listen: 0.0.0.0:8446
      cert_path: /path/to/cert.pem
      key_path: /path/to/key.pem
      handshake_timeout_ms: 5000
    pull_jobs:
      - name: remote_fmp4
        enabled: false
        source_url: http://example.com/live/stream.mp4
        target_stream_key: live/remote_fmp4
        retry_backoff_ms: 500
        max_retry_backoff_ms: 5000
        insecure_tls: false
```

**支持的编码格式：** H264、H265、H266、VP8、VP9、AV1、MJPEG（视频）；AAC、G711A、G711U、Opus、MP3、MP2（音频）。

### 3.8 HTTP-TS / WS-TS 模块使用指南

当启用 `ts` feature 且 `modules.ts.enabled=true` 后，服务器支持将实时音视频封装为标准的 MPEG-TS 格式，并通过 HTTP/WebSocket 协议提供极低延迟的播发。

**工作原理与核心特性：**
- **Sans-I/O 架构解耦**：`TsCore` 状态机专心处理 HTTP 头解析、CORS 控制、WebSocket 握手升级以及会话取消，完美隔离了底层的 I/O 引擎。
- **MPEG-TS 动态复用封装**：通过 `cheetah-codec` 自适应将传入的 `AVFrame` 序列化复用生成标准的 188 字节 TS 包，支持 PAT (Program Association Table) 和 PMT (Program Map Table) 节目的周期性注入（`pat_pmt_interval_ms`），确保任何时间点接入的播放器均能快速获取轨道拓扑。
- **首屏秒开优化 (GOP 缓存)**：完美支持在拉流端连接时，根据 `bootstrap_max_frames` 快速推送历史 GOP 帧，配合 PAT/PMT 瞬间点亮首帧。
- **双协议桥接 (RTP-TS Ingest)**：核心层还支持将 RTP 封装的 MPEG-TS 包流式解复用，并通过 `RtpTsIngest` 模块自适应转换为底层流媒体引擎的音视频轨道，极大丰富了多媒体接收面。

**播放 URL：**
```text
HTTP-TS:  http://127.0.0.1:8082/{app}/{stream}.ts
WS-TS:   ws://127.0.0.1:8082/{app}/{stream}.ts
```

ZLMediaKit 兼容 URL（支持 `.live.ts` 后缀）：
```text
HTTP-TS:  http://127.0.0.1:8082/{app}/{stream}.live.ts
WS-TS:   ws://127.0.0.1:8082/{app}/{stream}.live.ts
```

例如，当 RTMP 推流到 `rtmp://127.0.0.1:1935/live/test` 后，可以直接播放：
```bash
ffplay http://127.0.0.1:8082/live/test.ts
```

**HTTPS/WSS-TS (TLS 模式)：**
```text
HTTPS-TS: https://127.0.0.1:8445/{app}/{stream}.ts
WSS-TS:   wss://127.0.0.1:8445/{app}/{stream}.ts
```

**TS 模块配置示例：**
```yaml
modules:
  ts:
    enabled: true
    listen: 0.0.0.0:8082
    write_queue_capacity: 256
    read_buffer_size: 65536
    subscriber_queue_capacity: 256
    bootstrap_max_frames: 150
    play_wait_source_timeout_ms: 15000
    max_tracks: 32
    strict_crc: false
    max_reassembly_bytes: 4194304
    pat_pmt_interval_ms: 500
    demand_mode: false
    tls:
      enabled: false
      listen: 0.0.0.0:8445
      cert_path: /path/to/cert.pem
      key_path: /path/to/key.pem
      handshake_timeout_ms: 5000
    pull_jobs:
      - name: remote_ts
        enabled: false
        source_url: http://remote:8082/live/source.ts
        target_stream_key: live/remote_source
        retry_backoff_ms: 500
        max_retry_backoff_ms: 5000
        insecure_tls: false
```

**支持的编码格式：** H264、H265、VP8、VP9、AV1（视频）；AAC、G711A、G711U、Opus、MP3、MP2（音频）。

**Pull 拉流支持的 URL scheme：** `http://`、`https://`、`ws://`、`wss://`

### 3.9 RTP 模块使用指南

当启用 `rtp` feature 且 `modules.rtp.enabled=true` 后，服务器支持通过 UDP/TCP 直接接收纯 RTP 载荷流。

**工作原理与核心特性：**
- **Sans-I/O 架构推进**：`RtpCore` 纯状态机解耦，不持有 socket、timer 或异步任务，输入输出纯粹显式通过 `Input / Output` 模型进行流转。
- **高兼容性载荷探测 (Auto Probe)**：能够自适应识别 RTP 传入的数据包格式。完美支持探测并解复用：
  - **MPEG-TS**（同步头 `0x47`）
  - **PS (Program Stream)**（前缀 `0x000001BA`，如国标 GB28181 摄像头视频流）
  - **Raw ES 裸流**
  - **Ehome 特征包**（探测 Ehome2 特征握手魔法字 `[0x01, 0x00, 0x01/0x02]`）
- **高容错与失序重组 (Jitter Buffer)**：内置大容量的重构排序窗口，有效抵御丢包、乱序、重复包等复杂的专网/公网丢包乱序环境。
- **防御性内存安全防护**：在进行 PS (Program Stream) 重组解包时，设计了高达 4MB 的硬上限防溢出接收缓冲区（`max_reassembly_bytes`），彻底杜绝因坏包注入或长包溢出导致的内存耗尽与越界风险，并在超时或者空闲时及时清理连接句柄。

**配置示例：**
```yaml
modules:
  rtp:
    enabled: true
    listen_udp: 0.0.0.0:20000
    listen_tcp: 0.0.0.0:20000
    rtcp_listen_udp: 0.0.0.0:20001
    write_queue_capacity: 256
    read_buffer_size: 65536
    max_reassembly_bytes: 4194304
    max_tracks: 32
    idle_timeout_ms: 15000
    default_payload: ps
    allow_unaligned_payload: true
```

### 3.10 GB28181 SIP 视频国标流接入

当启用 `gb28181` feature 且 `modules.gb28181.enabled=true` 后，`cheetah` 服务器将作为 **国标 SIP 信令服务器 (GB/T 28181-2016)** 运行，提供安全摄像头的国标注册级联、心跳保活及视频实时点播。

**工作原理与核心能力：**
- **信令三段式解耦**：基于 Sans-I/O `Gb28181Core` 协议解析，完全分离了底层 UDP/TCP socket 网络传输，只关注 SIP 信令消息（REGISTER, MESSAGE, INVITE, BYE）的状态维护。
- **防刷级联注册与 MD5 挑战鉴权**：符合国标标准的双向挑战鉴权体系，收到 REGISTER 时发送 MD5 密码挑战，有效防止非授权摄像头注册接入。
- **实时会话定时器与心跳保活**：心跳定时检测机制（`tick_interval_ms` 轮询）会持续追踪在线摄像头的 Keepalive 心跳包，当检测到超时时自动将设备置为离线状态，防范僵尸设备资源占用。
- **点播 Invite 信令协商 (Invite/Bye)**：自动生成符合国标 RFC2327 规范的 SDP 描述，分配 RTP 输入端口及 unique SSRC，通过 Invite 命令拉取摄像头的音视频流，并能在不需要时发送 Bye 优雅切断流。
- **流生命周期闭环管理**：与底层 RTP/engine 强强联合，当设备注册离线、Invite 异常、或者 Bye 挂断时，自动闭环清理关联的媒体发布者租约与网络端口映射，确保系统无任何句柄泄露。
- **非阻塞控制面接口**：提供强大的 RESTful 信令操作接口，允许上层控制系统实时获取在线设备目录列表（Catalog），一键启动 Invite 实时监视与 Bye 挂断。

**配置示例：**
```yaml
modules:
  gb28181:
    enabled: true
    control_owner: local
    listen_udp: 0.0.0.0:5060
    listen_tcp: 0.0.0.0:5060
    read_buffer_size: 65536
    tick_interval_ms: 1000
```

`control_owner` 默认 `local`，表示由媒体进程内的 GB SIP 监听器处理信令；设为 `signaling` 时，由集群信号控制面接管（需要 `signaling_control_plane.enabled=true`）。`canary`/`production` 灰度阶段不允许 `local` 与信号控制面同时作为 GB 控制入口。

## 4. 测试

### 4.1 单 crate 测试

RTMP 相关改动优先跑：

```bash
cargo test -p cheetah-rtmp-module
cargo test -p cheetah-rtmp-core
cargo test -p cheetah-rtmp-property-tests
```

RTSP 相关改动优先跑：

```bash
cargo test -p cheetah-rtsp-module
cargo test -p cheetah-rtsp-driver-tokio
cargo test -p cheetah-rtsp-property-tests
```

HTTP-FLV 相关改动优先跑：

```bash
cargo test -p cheetah-http-flv-core
cargo test -p cheetah-http-flv-driver-tokio
cargo test -p cheetah-http-flv-module
bash dev-scripts/check_http_flv_smoke.sh
```

HLS 相关改动优先跑：

```bash
cargo test -p cheetah-hls-core
cargo test -p cheetah-hls-module
cargo test -p cheetah-hls-property-tests
bash dev-scripts/check_hls_smoke.sh
```

LLHLS 相关测试（含 part 切片、playlist 生成、请求路由）：

```bash
# 核心层：LLHLS playlist 标签、part 切片逻辑、fMP4 part 封装、URL 解析
cargo test -p cheetah-hls-core -- ll_hls
cargo test -p cheetah-hls-core -- format_iso8601
cargo test -p cheetah-hls-core -- parse_blocking
cargo test -p cheetah-hls-core -- parse_part
cargo test -p cheetah-hls-core -- parse_mp_suffix
cargo test -p cheetah-hls-core -- part_has_moof
cargo test -p cheetah-hls-core -- ll_hls_playlist_format

# Module 层：StreamMuxer 端到端 LLHLS 集成测试
cargo test -p cheetah-hls-module -- llhls
```

TS 相关改动优先跑：

```bash
cargo test -p cheetah-codec --test ts_codec_matrix
cargo test -p cheetah-ts-core
cargo test -p cheetah-ts-driver-tokio
cargo test -p cheetah-ts-module
```

fMP4 相关改动优先跑：

```bash
cargo test -p cheetah-codec -- fmp4
cargo test -p cheetah-fmp4-core
cargo test -p cheetah-fmp4-driver-tokio
cargo test -p cheetah-fmp4-module
```

RTP 相关改动优先跑：

```bash
cargo test -p cheetah-rtp-core
cargo test -p cheetah-rtp-driver-tokio
cargo test -p cheetah-rtp-module
cargo test -p cheetah-rtp-property-tests
```

GB28181 相关改动优先跑：

```bash
cargo test -p cheetah-gb28181-core
cargo test -p cheetah-gb28181-driver-tokio
cargo test -p cheetah-gb28181-module
cargo test -p cheetah-gb28181-property-tests
```

### 4.2 提交前最低检查

修改 Rust 代码后，至少执行：

```bash
cargo fmt
cargo clippy -p <changed-crate>
cargo test -p <changed-crate>
```

如果改动影响共享基础层、协议公共层或 `cheetah-codec` / `cheetah-sdk`，再补充工作区相关测试。

## 5. 发布

### 5.1 开发环境发布与编译说明

在打包编译发布版程序时，默认只启用基础的 `rtmp` feature。如果需要编译支持其他协议模块（如 `fmp4`, `ts`, `rtp`, `gb28181`），**必须**在编译指令中通过 `--features` 参数显式指定，以将其静态链入最终生成的 `cheetah-server` 二进制产物中。

默认发布只含 RTMP 的版本：

```bash
cargo build -p cheetah-server --release
```

如果需要同时打包 RTSP：

```bash
cargo build -p cheetah-server --release --features rtsp
```

如果需要同时打包 HTTP-FLV：

```bash
cargo build -p cheetah-server --release --features http-flv
```

如果需要同时打包 HLS：

```bash
cargo build -p cheetah-server --release --features hls
```

如果需要同时打包 fMP4 协议模块：

```bash
cargo build -p cheetah-server --release --features fmp4
```

如果需要同时打包 MPEG-TS 协议模块：

```bash
cargo build -p cheetah-server --release --features ts
```

如果需要同时打包 RTP 接收分发模块：

```bash
cargo build -p cheetah-server --release --features rtp
```

如果需要同时打包 GB28181 SIP 国标信令模块：

```bash
cargo build -p cheetah-server --release --features gb28181
```

**一键打包启用所有协议模块（RTSP + HTTP-FLV + HLS + fMP4 + TS + RTP + GB28181）的全功能版本：**

```bash
cargo build -p cheetah-server --release --features "rtsp,http-flv,hls,fmp4,ts,rtp,gb28181"
```

### 5.2 产物检查

发布前建议确认：

1. `cargo fmt` 已通过
2. `cargo clippy -p <changed-crate>` 已通过
3. `cargo test -p <changed-crate>` 已通过
4. 实际运行时配置文件路径和环境变量覆盖规则已确认
5. 目标机器上监听端口没有冲突

### 5.3 运行发布版

```bash
RUST_LOG=info ./target/release/cheetah-server
```

## 6. 相关文档

- [SystemArchitecture.md](/dataset/datavol/workspace/media_server/cheetah-media-server-rs/SystemArchitecture.md)
- [AGENTS.md](/dataset/datavol/workspace/media_server/cheetah-media-server-rs/AGENTS.md)
