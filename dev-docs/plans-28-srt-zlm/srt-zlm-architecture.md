# SRT 总体架构设计（对标 ZLMediaKit）

## 架构目标

SRT 仍按本项目协议三段式组织：

- `cheetah-srt-core`：纯 Sans-I/O 模型与业务无关协议类型；封装 `shiguredo_srt` 所需的配置、Stream ID（含 ZLM 语义）、版本策略、事件/命令、FEC 配置形状。
- `cheetah-srt-driver-tokio`：Tokio UDP socket、timer、listener/caller connection map、背压、统计与（可选）FEC 驱动集成。
- `cheetah-srt-module`：streamid → publish/play 决策、发布租约、订阅、TS demux/mux、鉴权参数、jobs、HTTP metrics。

ZLMediaKit 把 `SrtTransport` 与 `SrtTransportImp` 同时承担协议状态、NACK、队列、业务 streamid 和 TS 桥接。本项目 **不能** 复制这种混合边界：

| 职责 | ZLM | Cheetah |
|------|-----|---------|
| 协议状态机 | 自研 C++ | `shiguredo_srt` |
| UDP / timer | Poller / Session | `cheetah-srt-driver-tokio` |
| streamid / 鉴权 / 媒体桥 | `SrtTransportImp` | `cheetah-srt-module` |
| TS demux/mux | `DecoderImp` / `TSMediaSource` / MPEG | `cheetah-codec` |
| 统一媒体模型 | MediaSource 多 schema | `AVFrame + TrackInfo` + engine |

本计划是 [plans-28-srt](../plans-28-srt/srt-design.md) 的增强层，不重写 crate 骨架。

## Crate 与依赖方向

```text
apps / control
  -> cheetah-srt-module
  -> cheetah-srt-driver-tokio
  -> cheetah-srt-core
  -> shiguredo_srt（Sans-I/O）
  -> cheetah-codec（仅 module 媒体路径）
```

约束：

- `core` 不依赖 Tokio、socket、HTTP、engine、数据库或系统时间 API。
- `driver` 可依赖 Tokio，不持有业务状态，不直接操作 publish lease。
- `module` 公共接口不暴露 `tokio::net` / `tokio::time` / `tokio::sync`；多路等待使用 `CancellationToken` + futures 组合子。
- 所有缓存、发送队列、重传窗口、FEC 矩阵必须有上界。

## 数据流

### SRT 推流（publish）

```text
OBS / FFmpeg / libsrt (caller, m=publish)
  -> UDP
  -> cheetah-srt-driver-tokio (listener)
  -> shiguredo_srt::SrtConnection
  -> SrtDriverEvent::Connected { stream_id }
  -> module: parse streamid → StreamKey + auth
  -> acquire PublishLease
  -> Payload → cheetah-codec::MpegTsDemuxer
  -> TrackInfo + AVFrame
  -> PublisherSink → engine
  -> RTSP / RTMP / HLS / WebRTC / ... subscribers
```

对齐 ZLM `SrtTransportImp`：`m=publish` 时创建 TS decoder 并向 MediaSource 喂数据。Cheetah 差异：不直写 TS ring，而是 demux 为 canonical frame 以支持跨协议。

### SRT 拉流（request / play / 默认无 m）

```text
ffplay / VLC / libsrt (caller, m 缺省或 request/play)
  -> Connected { stream_id }
  -> module: parse → StreamKey + auth
  -> SubscriberSource (bootstrap + frames)
  -> cheetah-codec::MpegTsMuxer
  -> SrtDriverCommand::SendPayload
  -> shiguredo_srt → UDP → player
```

对齐 ZLM：查找 `TS` schema 源并挂 ring reader。Cheetah 差异：从 engine 订阅任意协议入站的统一帧，再 mux 为 TS（跨协议优先）。

### Caller jobs / Relay

保持 [plans-28-srt](../plans-28-srt/srt-design.md) 路径：

```text
ingress job:  remote SRT source → local StreamKey → engine
egress job:   engine StreamKey → remote SRT listener
relay job:    source → local key → target（经 engine，非包直通）
```

### 与 ZLM TS 直通路径的差异

| 项目 | ZLM | Cheetah（本计划保持） |
|------|-----|------------------------|
| 推流媒体 | TS → Decoder → Tracks → MultiMediaSourceMuxer | TS → MpegTsDemuxer → AVFrame → engine |
| 拉流媒体 | TSMediaSource ring 直接吐 TS | engine frames → MpegTsMuxer → SRT |
| 跨协议 | 依赖各 schema MediaSource | 统一 engine，天然互转 |
| SRT→SRT 直通 | 可同 schema 高效转发 | 默认经 engine；若未来做 TS 直通须单独标注 |

## Stream ID 权威语义（ZLM 对齐）

格式：

```text
#!::key1=value1,key2=value2,...
```

### 特殊 key

| Key | 含义 | 规则 |
|-----|------|------|
| `h` | vhost | 缺省使用配置 `default_vhost`（建议 `__defaultVhost__` 或 `default`，与项目约定一致即可） |
| `r` | 资源路径 | 期望 `app/stream` 两段；映射到本地流定位 |
| `m` | 模式 | `publish` = 推流；其它值或缺失 = 拉流（`request`/`play` 均拉流） |
| 其它 + `m` | 鉴权参数 | 进入 `auth_params`，供 webhook / 本地 auth 使用 |

示例：

```text
#!::h=zlmediakit.com,r=live/test,m=publish

vhost  = zlmediakit.com
app    = live
stream = test
mode   = publish
```

```text
#!::r=live/test
# 无 m → 默认拉流
```

### 与当前解析器差异

当前 `parse_srt_stream_id`（`crates/protocols/srt/core/src/stream_id.rs`）：

- 已解析 `r/m/u/h/s/extras`
- 未拆 `app`/`stream` 字段
- 未强制 `r` 两段
- module 默认 mode 为 `publish`

目标模型（建议类型形状）：

```text
ParsedSrtStreamId {
  vhost: String,              // h 或 default_vhost
  app: String,                // r 第一段
  stream: String,             // r 第二段
  mode: Option<SrtStreamMode>,// None 表示未声明
  user: Option<String>,       // u
  session: Option<String>,    // s
  auth_params: BTreeMap,      // 除 h/r 外全部 key（含 m）
  raw: String,                // 原始 streamid
}
```

### StreamKey 映射

Cheetah `StreamKey { namespace, path }` 无独立 vhost 维。

**推荐默认策略（`stream_key_vhost_mode = "app_only"`）**：

- `namespace = app`
- `path = stream`
- `vhost` 进入 session meta、metrics label、`auth_params` 旁路字段，不进入 key

**可选策略（`stream_key_vhost_mode = "vhost_prefix"`）**：

- `namespace = "{vhost}/{app}"` 或规范化后的合成段
- 仅在多 vhost 隔离部署开启

**兼容**：

- `stream_id.allow_bare_key = true` 时允许 `r=live/test` 整串或 bare key 走旧 `stream_key_from_string`
- 严格 ZLM 模式：`r` 不足两段则拒绝连接

### 客户端用法（写入运维与 Phase 05）

| 客户端 | 用法 |
|--------|------|
| OBS | `srt://host:9000?streamid=#!::r=live/test,m=publish` |
| FFmpeg 推 | `-f mpegts srt://host:9000?streamid=#!::r=live/test,m=publish` |
| ffplay 拉 | `srt://host:9000?streamid=#!::r=live/test` |
| VLC 拉 | 偏好设置 SRT streamid = `#!::r=live/test`；URL = `srt://host:9000` |

## 版本策略

- 握手 HS version：5（UDT/SRT 握手包 version 字段）。
- 本端 SRT 库版本宣告：建议 `0x010500`（1.5.0），与 ZLM / `shiguredo_srt` 默认一致。
- **策略要求 peer `>= 1.3.0`**（`0x010300`）：
  - 配置：`min_peer_srt_version = "1.3.0"`
  - 若 peer HS 扩展版本可解析且低于最小值 → 拒绝并诊断 `peer_version_too_old`
  - 若 peer 未携带 HS 扩展版本 → 按配置 `require_peer_version_extension` 决定拒绝或兼容放行
- 本端不得宣称低于 1.3.0。

## NACK / ARQ 架构位置

```text
丢失检测 / NAK 生成 / 重传调度
  └── 在 shiguredo_srt 内（对标 ZLM NackContext + PacketSendQueue 的能力，而非复制代码）
统计与配置
  └── driver 透传 stats；module 聚合 metrics
latency / TSBPD / buffer 上界
  └── ConnectionOptions + driver config
弱网验收
  └── driver/module 集成测试 + netem 互操作
```

不在 module 实现 NACK 状态机。若实测 `shiguredo_srt` 在高丢包下行为不足，优先：升级库 → 配置调优 → 再评估补丁。

## FEC 架构位置（Phase 04）

SRT Packet Filter / FEC 是握手扩展能力（ZLM 未实现）。

推荐分层：

```text
cheetah-srt-core
  - FecConfig / FecLayout / FecNegotiateResult 类型
  - 纯函数：布局校验、恢复算法（若放 core）
cheetah-srt-driver-tokio
  - 在连接选项中启用协商
  - 收发包路径挂 filter（若库支持）或旁路集成
cheetah-srt-module
  - 配置透传、metrics：recovered / unrecovered / negotiated
```

协商失败或对端不支持 → **降级 ARQ-only**，连接仍应成功（除非配置 `fec.required = true`）。

实现路径（Phase 04 决策）：

1. 优先：`shiguredo_srt` 上游能力或可合并补丁。
2. 次选：本仓库对 `shiguredo_srt` 的 vendor/patch（记录版本钉扎）。
3. 再次：在 driver 边界做有限 filter 包装（需严格评估与库缓冲一致性）。

## 配置模型

在现有 `SrtModuleConfig` 上扩展（建议字段，名称可微调）：

```text
enabled: true
listen: "0.0.0.0:9000"
max_connections: 1024
idle_timeout_ms: 30000
connect_timeout_ms: 5000
latency_ms: 120                 # TSBPD delay
latency_mul: 4                  # 对齐 ZLM latencyMul 文档语义（可选，用于按 RTT 建议）
pkt_buf_size: 8192              # 对齐 ZLM pktBufSize / 收发缓冲包数上界
stats_interval_ms: 5000
payload.kind: "mpegts"          # 仅允许 mpegts
min_peer_srt_version: "1.3.0"
require_peer_version_extension: false
default_vhost: "__defaultVhost__"
stream_id:
  strict_resource: true         # r 必须 app/stream
  allow_bare_key: false
  stream_key_vhost_mode: "app_only"  # app_only | vhost_prefix
ingress:
  default_mode: "request"       # ZLM 对齐；可设 publish 兼容旧部署
  default_publish_stream_key: ""
  publish_keepalive_ms: 0
encryption:
  enabled: false
  passphrase: ""
  key_length: 16
auth:
  enabled: false
  publish_token: ""
  request_token: ""
  users: []
  # 预留 webhook：将 auth_params 转发到 control
  webhook_enabled: false
fec:
  enabled: false
  required: false
  cols: 10
  rows: 5
  # layout / algorithm 细节见 Phase 04
egress: { ... 保持 plans-28-srt }
ingress_jobs / egress_jobs / relay_jobs: { ... }
```

配置应用语义：

- `listen` / encryption / payload / fec 主开关 / max_connections 变更 → `ModuleRestartRequired`
- 仅 jobs 列表变更：首版可 `ModuleRestartRequired`，后续再做热更新
- 重启由基础层 `create -> init -> start`，module 不维护私有重启流程

## HTTP 与控制接口

保持并扩展现有 SRT module 路由前缀 `/srt`：

| 方法 | 路径 | 用途 |
|------|------|------|
| GET | `/srt/metrics` | Prometheus 文本 |
| GET | `/srt/metrics.json` | JSON 快照 |
| （可选后续） | `/srt/sessions` | 会话列表：peer、mode、stream、vhost、stats |

错误语义必须明确：

- streamid 非法 / 缺 `r` / 严格模式下 `r` 非两段
- 鉴权失败
- 发布租约冲突
- 流不存在（拉流）
- peer 版本过低
- 连接数上限
- 非 TS payload 配置

## 观测与诊断

Driver / module 应能暴露：

| 指标 / 字段 | 说明 |
|-------------|------|
| connections / publish / play | 连接计数 |
| bytes/packets in/out | 吞吐 |
| sender_total_retransmits | 重传 |
| receiver_total_lost / duplicates | 丢包 / 重复 |
| rtt / jitter | 链路质量 |
| loss_list depth | 当前丢失列表深度 |
| tlpktdrop（若库暴露） | 过期丢包 |
| handshake_reject_total{reason} | 含 version / auth / streamid |
| fec_negotiated | 是否协商成功 |
| fec_packets_recovered / unrecovered | FEC 效果 |
| auth_reject_total | 鉴权失败 |

日志关键字段：`peer_id`、`remote`、`stream_id`、`vhost`、`app`、`stream`、`mode`、`reject_reason`。

## 模块拆分目标

`module.rs` 当前过大（~1400 行）。目标结构：

```text
module/
  src/
    lib.rs
    config.rs
    metrics.rs
    http.rs
    module.rs              # Module trait 与启动拼装
    stream_classify.rs     # streamid → mode + StreamKey
    auth.rs                # auth_params + token/webhook
    ingress_session.rs     # publish + TS demux
    egress_session.rs      # play + TS mux
    jobs.rs                # ingress/egress/relay plan
```

单文件尽量 <500 行，明显超过 800 行必须拆。

## 测试分层

| 层 | 内容 |
|----|------|
| core 单元 | streamid 全矩阵、版本解析、配置校验、FEC layout 纯函数 |
| property | streamid 乱序字段、percent-encoding、边界字符 |
| driver 集成 | listener/caller、加密、stats、弱网 netem、版本拒绝 |
| module E2E | publish→engine→play；跨协议；鉴权失败；默认 mode |
| fuzz | streamid、URL、driver 脏包、FEC 配置 |
| 外部互操作 | OBS / ffmpeg / ffplay / VLC / libsrt / 可选 ZLM 对端 |

## 一句话总纲

- 协议状态：`shiguredo_srt`
- 业务语义：ZLM streamid（`h/r/m` + 默认拉流）
- 媒体：MPEG-TS only → `AVFrame + TrackInfo`
- 可靠传输：ARQ 必选可观测；FEC 可选可降级
- 边界：core / driver / module 不混写
