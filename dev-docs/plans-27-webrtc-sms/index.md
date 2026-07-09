# WebRTC 实现设计与渐进式开发计划（对标 SimpleMediaServer）

- **状态**: **全部五个 Phase 已完成**。Phase 01-03 已完成（含 engine 媒体桥），Phase 04 已完成（simulcast 全部三种静态策略 + `Adaptive`（`bwe_low_threshold_kbps` / `bwe_high_threshold_kbps` 阈值 + `min(bwe, remb)` 双轨动态降层 + NACK storm 触发降层闭环）落地、stats/BWE/RTCP 事件链路完整、§4.8 metrics 表面（`WebRtcModuleMetrics` 聚合器 + `metrics_snapshot()` + `GET /api/v1/rtc/metrics`(Prometheus) + `GET /api/v1/rtc/metrics.json`(JSON) 端点）已上线、RTP 头扩展（audio-level / voice-activity / video-orientation / 序列号 / contiguous）已通过 `WebRtcFrameMeta` 透传到 `cheetah-codec` `FrameSideData`、netem-style 路由表丢包/乱序/migration race 单元测试已落地），Phase 05 已完成（DataChannel 双向收发与 echo loopback、`POST /session/{id}/datachannel/send`（文本 + base64 binary 双模式）、WHIP/WHEP PATCH（trickle ICE + 客户端发起的 ICE restart）、core `CreateOffer` 与 `IceRestart`、客户端 WHIP/WHEP HTTP 信令栈与 `pull/start|stop|list` `push/start|stop|list` 端到端（含真实 `CreateOffer→POST→ApplyAnswer` 编排）、SSRF 拒绝、P2P add/remove/list（含 `playStreamName` 触发的 sendrecv 引擎桥接）、`POST /session/{id}/ice-restart` 端点、cargo-fuzz harness（含 `fuzz_url_parse` 与 `fuzz_http_response`）、Pion / GStreamer / 浏览器 `--ignored` 互操作测试 scaffold、SMS SDP fixture 全套冒烟（h265 / janus / simulcast）、`config.example.yaml` 完整 webrtc 配置示例 全部已上线）。仅真实外部 peer 的全链路互操作（需要外部进程或 docker 互联）与 netem 集成测试留作后续 CI 配置时启用
- **目标**: 使用 `str0m 0.19.0` 作为 WebRTC Sans-I/O 协议栈，新增符合本项目 `core + driver + module` 架构的 WebRTC 推流、播放、协议互转、WHIP/WHEP、双向 echo、simulcast、RTX/NACK/TWCC、DataChannel、WebRTC over TCP、client pull/push 与 P2P 能力
- **方法**: 参考 `vendor-ref/simple-media-server/Src/Webrtc/` 与 `vendor-ref/simple-media-server/Src/Api/WebrtcApi.cpp` 的功能拆分和兼容行为，但不移植其 STUN/DTLS/SRTP/SCTP 内部实现；这些协议状态由 `str0m` 承担
- **完成标准**: WebRTC 三段式 crate、codec contract、单端口多会话 driver、SMS-compatible API、WHIP/WHEP、engine 桥接、丢包重传、simulcast、DataChannel、client/P2P、互操作测试与 fuzz/fixture 体系全部落地

---

## V1 范围

首版固定支持：

1. WebRTC 推流：浏览器或 WebRTC 客户端发布音视频，统一收敛为 `AVFrame + TrackInfo` 后进入 engine。
2. WebRTC 播放：RTMP、RTSP、RTP、GB28181、HTTP-FLV、HLS/fMP4 等已有源通过 engine 转 WebRTC 输出。
3. 协议互转：WebRTC 入站流可被其他协议播放，其他协议入站流可被 WebRTC 播放。
4. 双向 echo test：支持 media loopback 和 DataChannel echo 两种诊断模式。
5. Simulcast 推流：识别 RID / SSRC 关系，按配置选择入引擎层或保留多层观测。
6. 上下行 RTX/NACK：使用 `str0m` 的 NACK/RTX 能力与 bounded resend cache，结合 codec/driver 观测做回归测试。
7. TWCC/BWE：启用 Transport Wide CC 与发送侧带宽估计，模块层根据估计调整发送策略。
8. RTCP：支持 SR、RR、PLI、FIR、REMB、NACK、TWCC 等常用反馈的收发和指标暴露。
9. RTP extension：解析 audio-level、abs-send-time、transport-wide-cc、mid、rid、repaired-rid、orientation 等扩展。
10. GOP 秒开：复用 engine ring buffer 与 `BootstrapPolicy`，并由 `cheetah-codec` 完成关键帧参数集补发。
11. DataChannel：支持浏览器 DataChannel 建立、收发、echo test 与控制消息通道。
12. WebRTC over TCP：支持 TCP host candidate 和 TCP listener，首版聚焦 passive TCP server 模式。
13. WHIP/WHEP：提供标准 HTTP 信令入口，并保留 SMS-style `/api/v1/rtc/play`、`/api/v1/rtc/publish` 兼容入口。
14. ICE full：默认 `ice_lite=false`，支持作为服务端 answerer、客户端 offerer/answerer，以及 P2P 模式。
15. WebRTC client pull/push：支持作为 WebRTC 客户端从远端拉流或向远端推流。

首版不做：

1. 不做内置转码；不兼容的 codec 组合返回明确错误或降级到可协商轨道。
2. 不实现 TURN server；TURN/STUN server 地址由配置注入，`str0m` 不负责 TURN relay 服务。
3. 不直接复制 SMS 的自研 DTLS/SRTP/SCTP/STUN 代码。
4. 不在 module 里复制媒体时间戳修正、参数集缓存、NALU 处理或 jitter buffer 逻辑。
5. 不承诺“开源界唯一”等营销表述；只以可测能力描述单端口、多线程、连接迁移。

---

## 与 SimpleMediaServer 对比后的主要缺口

| 能力 | SMS 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| WebRTC HTTP API | `Api/WebrtcApi.cpp` | 无 WebRTC 模块 | Phase 03 |
| WHIP/WHEP 兼容入口 | `/api/v1/rtc/whip`、`/api/v1/rtc/whep` | 无 | Phase 03 |
| WebRTC session/context | `WebrtcContext.*` | 无 | Phase 01 / 02 使用 `str0m::Rtc` 包装 |
| 单端口 UDP/TCP server | `WebrtcServer.*`、`WebrtcContextManager.*` | 无 | Phase 02 |
| STUN/ICE/DTLS/SRTP/SCTP | `WebrtcStun.*`、`WebrtcIce.*`、`WebrtcDtlsSession.*`、`WebrtcSrtpSession.*`、`SctpAssociation.*` | 无 | 由 `str0m 0.19.0` 承担，driver 只负责 I/O |
| RTP/RTCP 包解析 | `WebrtcRtpPacket.*`、`WebrtcRtcpPacket.*` | `cheetah-codec` 有 RTP 基础，暂无 WebRTC 扩展 | Phase 01 / 04 |
| RTP extension map | `RtpExtTypeMap` | 无 WebRTC 映射 | Phase 01 / 04，优先使用 `str0m` extension values |
| WebRTC media source ring | `WebrtcMediaSource.*` | engine ring buffer 已存在 | Phase 03 复用 engine bootstrap |
| NACK/RTX 缓存 | `WebrtcContext::_rtpCache` | 无 WebRTC resend path | Phase 04 使用 `StreamTx::set_rtx_cache` / send buffer |
| WebRTC client pull/push | `WebrtcClient.*` | 无 | Phase 05 |
| WebRTC P2P | `WebrtcP2PClient.*`、`WebrtcP2PManager.*` | 无 | Phase 05 |
| DataChannel | `SctpAssociation.*` | 无 WebRTC DataChannel | Phase 05 由 `str0m` SCTP/DataChannel API 实现 |

---

## `str0m 0.19.0` 能力边界

`str0m` 适合作为 `cheetah-webrtc-core` 的主依赖，原因是它本身是 Sans-I/O WebRTC 实现：`Rtc` 不做网络 I/O、不启动内部线程或 async task，时间通过外部 `Instant` 输入推进。这与本项目 core 层约束一致。

`str0m` 覆盖：

- SDP / ICE / DTLS / SRTP / SCTP / RTP / RTCP / DataChannel
- Send/Recv Reports、Transport Wide CC、Bandwidth Estimation
- Simulcast、NACK、packetize、fixed depacketize/reorder buffer
- H264、H265、VP8、VP9、AV1、Opus、PCMA、PCMU 等 codec 协商开关
- RTP mode：可绕过内部 packetizer/depacketizer，直接收发 RTP packet
- RTX cache 与 NACK 响应配置

`str0m` 不覆盖，必须由本项目实现：

- socket bind、UDP/TCP accept/connect、单端口多会话调度
- TURN server、network interface 枚举和 candidate policy
- WHIP/WHEP HTTP 信令、SMS-compatible HTTP API
- engine publish/subscribe、协议互转、GOP bootstrap
- 自适应 jitter buffer；`str0m` 有 fixed depacketize/reorder buffer，但不提供完整 adaptive jitter buffer
- 音视频采集、编码、解码、转码
- 生产级码率策略闭环；`str0m` 给出 BWE/TWCC 信息，具体降层、丢帧、限速由 module 决策

---

## 架构约束

1. 严格新增协议三段式 crate：
   - `crates/protocols/webrtc/core` (`cheetah-webrtc-core`)
   - `crates/protocols/webrtc/driver-tokio` (`cheetah-webrtc-driver-tokio`)
   - `crates/protocols/webrtc/module` (`cheetah-webrtc-module`)
2. `cheetah-webrtc-core` 只包装 `str0m::Rtc` 与纯状态机输入输出，不依赖 Tokio、socket、HTTP、engine、数据库或系统时间 API。
3. `cheetah-webrtc-driver-tokio` 负责 socket、timer、spawn、channel、TCP/UDP framing、backpressure 和单端口调度。
4. `cheetah-webrtc-module` 负责 HTTP API、鉴权、配置、engine 接入、资源分配、业务编排、client/P2P job 生命周期。
5. `cheetah-codec` 负责 WebRTC ingress/egress 的媒体归一化、RTP timestamp、access unit、参数集补发、payload view 和 codec compatibility。
6. 所有队列、RTX cache、GOP bootstrap、candidate/session map、DataChannel buffer 都必须有上界。
7. 不在 SDK、engine、module 公共接口暴露 `tokio::*` 或 `tokio_util::*` 类型。

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [webrtc-str0m-architecture.md](webrtc-str0m-architecture.md) | 已完成 | 总体架构、crate 边界、数据流、配置、API、codec contract |
| [webrtc-sms-gap-analysis.md](webrtc-sms-gap-analysis.md) | 已完成 | SMS 行为拆解、`str0m` 替代范围、本地缺口 |
| [phase-01-core-codec-contracts.md](phase-01-core-codec-contracts.md) | 已完成 | crate skeleton、core Sans-I/O wrapper、codec contract |
| [phase-02-driver-single-port-ice-tcp.md](phase-02-driver-single-port-ice-tcp.md) | 已完成（基础设施落地，`WebRtcDriverHandle` 暴露 `stats_snapshot()` 与 `WebRtcDriverStats`，driver smoke 测试矩阵覆盖 UDP bind / accept_offer / 垃圾 SDP / 未路由 UDP 包 / session 关闭 count 复位 / 命令队列满 / stats 计数器；`WebRtcSendError` 公开；TCP/多 shard 标记为后续迭代） | 单端口 UDP/TCP、多线程 shard、ICE、连接迁移 |
| [phase-03-module-whip-whep-publish-play.md](phase-03-module-whip-whep-publish-play.md) | 已完成（含 engine PublisherSink 桥接、play 端 SubscriberApi 桥接、WHIP/WHEP PATCH trickle ICE） | WHIP/WHEP、SMS API、推流、播放、GOP 秒开 |
| [phase-04-rtcp-simulcast-loss-control.md](phase-04-rtcp-simulcast-loss-control.md) | 已完成（simulcast `highest`/`lowest`/`rid:<n>`/`adaptive`（BWE 估计与 remote REMB 取 `min` 双轨动态降层 + NACK storm 触发强制最低层）层选择策略已落地，stats/BWE/RTCP 事件链路完整；RTP 头扩展（audio-level / voice-activity / video-orientation / 首包 RTP 序号 / contiguous）通过 `WebRtcFrameMeta` 写入 `cheetah-codec::FrameSideData`；REMB 与 TWCC 双轨 fallback 仿真 + selection-loop + NACK storm 单元测试已落地；netem-style 路由表丢包/乱序/migration race 单元测试已落地；§4.8 metrics 表面（`WebRtcModuleMetrics` 聚合器 + `metrics_snapshot()` 操作员快照 + `GET /api/v1/rtc/metrics` Prometheus 端点 + `GET /api/v1/rtc/metrics.json`，counters 由事件 worker 在线增量）已落地；真实媒体路径下的丢包/乱序网络场景需要外部 netem 工具，标记为后续 CI 迭代；发送 pacing cap 留作未来 BWE 闭环增强） | simulcast、RTX/NACK、TWCC/BWE、RTCP、RTP extension |
| [phase-05-client-p2p-datachannel-interop.md](phase-05-client-p2p-datachannel-interop.md) | 已完成（DataChannel 双向收发 + echo loopback + `POST /session/{id}/datachannel/send`、WHIP/WHEP PATCH（含客户端 ICE restart 触发 + trickle ICE）、core `CreateOffer` + `IceRestart`、客户端 WHIP/WHEP HTTP 信令栈 + pull/push `start/stop/list` + 真实 `CreateOffer→POST→ApplyAnswer` 编排 + SSRF 拒绝、P2P add/remove/list（含可选 `playStreamName` 双向 sendrecv 引擎桥接）、`POST /session/{id}/ice-restart` 端点、cargo-fuzz harness（`fuzz_sdp_compat` / `fuzz_zlm_rtc_url` / `fuzz_tcp_framing` / `fuzz_trickle_candidates` / `fuzz_url_parse` / `fuzz_http_response`）、Pion / GStreamer / 浏览器 `--ignored` 互操作 scaffold、SMS SDP fixture 全套（h265 / janus / simulcast / publish）冒烟集合 全部已上线；真实外部 peer 的全链路互操作 body 留作 CI 配置完成后启用） | WebRTC client、P2P、DataChannel、互操作、fuzz |

---

## 渐进式执行顺序

1. **Phase 01** — 建立 `cheetah-webrtc-core` 和 `cheetah-codec` WebRTC contract，确保 Sans-I/O 边界和 codec 策略先固定。
2. **Phase 02** — 建立 `cheetah-webrtc-driver-tokio`，先跑通单端口 UDP/TCP、timer、session route、连接迁移。
3. **Phase 03** — 建立 `cheetah-webrtc-module`，打通 WHIP/WHEP、SMS API、WebRTC publish/play 和 engine 桥接。
4. **Phase 04** — 增强实时质量能力：simulcast、RTX/NACK、TWCC/BWE、RTCP 反馈、RTP extension、丢包乱序测试。
5. **Phase 05** — 补齐 WebRTC client pull/push、P2P、DataChannel、浏览器/非浏览器互操作和 fuzz/fixture 体系。

---

## 已知改进项（review 阶段补丁）

- `cheetah-webrtc-core::WebRtcSession::request_keyframe` 已从占位实现改为通过 `str0m::Writer::request_keyframe` 真实下发 PLI/FIR，并对未连接 / 未知 mid / writer 失败给出明确诊断。
- `cheetah-webrtc-driver-tokio` runner 移除遗留的 `_phantom`、未使用的 `WebRtcMediaEvent` 引用以及夸大的 TCP listener 文档；`listen_tcp` 当前仅作为配置项被解析，未真正绑定。
- `cheetah-webrtc-module::bridge::push_frame` 在协商后 codec / clock-rate 变化时会重新登记 track 并刷新本地 cache，避免 engine 看到陈旧 metadata。
- `cheetah-webrtc-module::http_client` URL 解析仅在 authority 部分检查 `@` 用户信息，避免误拒查询串里出现 `@` 的合法 URL；新增 IPv6 字面量 / `?email=user@host` 等回归测试。
- `cheetah-webrtc-module::jobs` 注册表允许覆盖处于 `Failed` / `Stopped` 终态的旧条目，以支持同 `StreamKey` 重新启动；活跃态仍按 409 处理。
- `pull/start` 入口 `protocol` 字段对未知值返回 400，不再被静默 fallback 到 `Whep`。
- `module_lifecycle` 集成测试中的 fake WHIP/WHEP 服务器改为按 `content-length` 完整读完请求体，消除并行下的偶发 flake。

### 第二轮 review 修复

- **driver session_count 双扣**：`StopSession` 命令处理与 `WebRtcCoreOutput::CloseSession` drain 都在调用 `fetch_sub`，会把 `usize` 计数器扣到下溢；现在统一交给 drain 路径处理，确保和远端发起的关闭走同一条路径。
- **driver tick O(n²)**：`WebRtcCoreInput::Tick` 在 core 内部已经对所有 session 迭代，runner 不应该再按 session 数量重复触发；改成每次唤醒只触发一次全局 tick。
- **driver runner 死代码**：清理 `let _ = (routes, session_remote);` 占位、`if prev.is_none() || prev == Some(...)` 空分支，以及不再准确的“filter periodic media frame”注释。
- **module 事件路由**：`WebRtcDriverEvent::Diagnostic` 的失败传播不再仅匹配 `AcceptOffer failed`，而是对所有带 session id 的 lifecycle 失败都向 dispatcher 推送，避免 `CreateOffer` / `ApplyRemoteAnswer` 错误时 HTTP 调用一直挂到 timeout。
- **module DataChannel echo**：移除把 `WebRtcCoreCommand::SendDataChannel` 立刻 match 出来的反向构造；直接构造 `WebRtcDataChannelOut`。
- **module bridge.rs**：删除 `let _ = (CodecId::Unknown,);` 死代码与对应未使用的 `CodecId` 局部 `use`；修正 `Highest` 测试的注释，使其不再倒置 ASCII 字典序。
- **HLS muxer ADTS 5.1**：`AdtsHeader::build` 之前忽略 channel_configuration 的最高位，导致 `6`（5.1）写入后被强制变成 `2`（立体声）；修复并新增 `adts_wrap_preserves_high_channel_configurations` 回归测试。
- **HLS muxer AAC PCE**：当 ASC 给出 `channel_configuration=0` 并通过 PCE 描述布局时（FLV 5.1 来源常见），`set_tracks`/`push_frame` 现在通过 `aac_channel_count_from_asc` 解析真实通道数，并把 ADTS 头里的 channel_configuration 从 0 修正到匹配的值，避免 ffmpeg/Safari/hls.js 把流当做“channels=0, sample_rate=0”拒播；新增 PCE 5.1 解析测试。
- **HLS subscriber 启动**：playlist 触发先于发布者注册 stream 时，`subscribe()` 之前会立刻 `NotFound` 失败；现在以 200ms 节奏退避重试 30 次（约 6s 窗口），允许 HEVC enhanced RTMP 等慢启动 publisher 完成注册。
- **HLS muxer 终态保留**：发布结束后 muxer 立刻被移出 map，会把刚结束的短片段直接“吃掉”；新增 `concluded_retention_secs`（默认 30s）让带 ENDLIST 的 playlist 与已完成 segment 在结束后仍可被 late-join 客户端拉取。
- **HLS AAC 配置补抓**：第一帧音频到达时如果 muxer 还缺 ASC，重新拉一次 stream snapshot 并喂给 muxer，处理“publisher 在第一个 audio frame 之后才发布 AAC config”的真实顺序。
- **WebRtcModuleConfig::validate**：拒绝形如 `rid:` 或 `rid:   ` 的空白 rid 策略，避免运行时静默 fallback 到 `Highest`；新增对应回归测试。

### 第三轮 review 修复

- **sdp_compat 重复 OR / 空 if 分支**：`preprocess_remote_sdp` 中 `only_crlf` 用了 `input == replace || input == replace`（左右两边相同的死表达式）；之后又有一个空 `if !report.normalized_line_endings && final_text != input { /* comment-only */ }` 块。整体重写：normalize / trim / append-terminator 三步线性，标志在恰当的时刻被设置，删除自我比对的 hack。新增 `preprocess_does_not_flag_canonical_input` 与 `preprocess_handles_lone_cr_terminators` 回归测试。
- **driver 迁移检测语义错乱**：原实现用 `routes.bind` 的返回值（前一个绑定到该地址的 *session*）做迁移检测，并把 `previous_addr` 设成 `prev.map(|_| datagram.source)` —— 等于把 new_addr 当 previous_addr 抛出去。改为：在 `bind` 之前从 `session_remote` 读出该 session 的上一个地址，作为 `previous_addr`；同时为旧地址在 `RouteTable` 上调用新增的 `unbind_address`，把旧绑定移到 stale set，等 stale_ttl 过期，避免 stale 路由继续把新到达的不相关包错误派给已迁移的 session。新增 `unbind_address_moves_route_to_stale` 路由表单元测试。
- **driver UnroutedPacket 字节数恒为 0**：`route_unbound_packet` 已经把 `datagram.data` move 走，下游 None 分支用 `format!("dropped {} byte ...", 0, ...)` 写死了 0；改为在 move 前 capture `packet_len`。
- **core send_frame `random_access` 死忽略**：原来一句 `let _ = frame.random_access;` 没有任何注释，看起来像未实现路径；补上明确注释解释为什么 str0m 不需要它（packetizer 自己从 codec payload 推导关键帧），并保留字段以便未来 RTP-mode 透传。
- **core output WebRtcDirection 死定义**：`output::WebRtcDirection`（Ingress/Egress）从未被任何地方使用，但通过 `lib.rs` `pub use ... as WebRtcOutputDirection` 暴露在公共 API 上；删除未用枚举与对应的 re-export。
- **module compat.rs 死辅助函数**：`stream_key_from_pair` 是 1 行的 `StreamKey::new(app, stream)` wrapper，从未被调用；按 AGENTS.md §10 删除，并去掉随之失效的 `StreamKey` import。同时把 `url_decode_lossy` 的两段缓冲（`String` + `Vec<u8>`）合并成一个 `Vec<u8>`，最后用 `into_owned`。
- **module codec_policy 冗余分支**：`WebRtcAudioCodecPreference::is_allowed` 写了 `(Browser, Aac) => false`、`(Browser, G711a | G711u) => true`、`_ => true` —— 第二个分支是显式的 fallthrough，没有意义；改写成 `!matches!((profile, self), (Browser, Aac))`；视频版本对应做了相同清理。

### 第四轮 review 修复

- **`AdtsHeader::build` 静默截断 ch_cfg > 7**：之前用 `self.channel_configuration & 0x07`，把 ASC 中 11（7-channel 6.1）等大于 7 的值默默掩成 3，导致解码器看到错误的声道布局。改成 `min(7)`：超过 ADTS 三位字段范围的值饱和到 7（八声道）这一最接近的合法布局，并新增 `adts_wrap_saturates_oversized_channel_configurations` 回归测试。
- **HLS muxer `channels_to_aac_channel_configuration`**：补上文档解释 ADTS 只有 3 位 ch_cfg 字段，超过 8 声道的 MPEG-4 ASC 值（11/12/14）无法穿透 ADTS，因此 7 声道也回落到 None（调用方走 stereo），避免写出 ADTS 不支持的值。
- **HLS muxer `concluded_retention_secs` 例子**：`config.example.yaml` 增加 HLS 段的中文注释，明确该字段语义。
- **WebRtcModuleConfig 缺失校验**：`read_buffer_size == 0` 之前没被 `validate()` 拒绝，运行起来会让 UDP recv loop 拿到 0 长度缓冲；新增校验和单元测试。
- **`dispatch_timeout` 把 last_activity_at 当成 wall-clock 更新**：每次 Tick / Timeout 都设置 `session.last_activity_at = now`，使该字段无法被未来的 idle-timeout 逻辑使用；移除该写入并补上注释解释 Tick / Timeout 是时间推进而不是会话活动。

### 第五轮 review 修复

- **`WebRtcPublishBridge::push_frame` 全量替换 track 列表**：每次首次见到一个 mid 或 codec 变更时，bridge 都会调 `update_tracks(vec![info])`，但 engine 的 `PublisherSink::update_tracks` 是“整列表替换”而不是“按 track 合并”。这意味着先发布音频再发布视频时，视频帧到达时的 update_tracks 调用会把音频 track 从 engine 视图里抹掉。新增私有 helper `build_tracks_snapshot` 把所有已知 mid 的 TrackInfo 重新构造成完整快照再传给 `update_tracks`，并新增 `build_tracks_snapshot_includes_sibling_tracks` 单元测试覆盖该回归。
- **driver 闲时 sleep 退化为 busy-loop**：`run_driver_core` 在没有定时器时，`sleep_until` 用的是 `start_instant + 1h`。一旦驱动跑超过一小时，这个时间已经过了，select 会立即 wake 触发 Tick，进入 100% CPU 循环。改成 `Instant::now() + 1h`，确保 idle 等待始终是真正的 1 小时。
- **HTTP client 死 HashMap**：`parse_complete_response` 维护了一个 `header_map: HashMap<String, String>`，用 `let _ = header_map;` 抑制 unused 警告；其实根本没人读它。删除这段代码与对应 `use std::collections::HashMap;`。
- **bridge 死 `denom == 0` 分支**：`spawn_play_subscriber` 中 `let denom = frame.timebase.den.max(1);` 已经把 `denom` 至少抬到 1，紧跟着的 `if denom == 0 { 0 } else { ... }` 就成了死代码；移除并直接做 RTP tick 计算。
- **codec adapter 公共类型用 crate-private prelude**：`WebRtcIngressContractView` 的 `rid` / `repaired_rid` 字段写成 `Option<crate::prelude::String>`，prelude 是 `pub(crate)`，对外用户拿到的是同一个 std `String`，但路径名造成误导。改用裸 `String`（adapter.rs 已经 `use crate::prelude::*`），让对外签名清爽。

### 第六轮迭代（Phase 05 收尾）

- **ICE restart 端到端落地**：core 新增 `WebRtcCoreCommand::IceRestart { session_id, keep_local_candidates, now_micros }`，调用 `SdpApi::ice_restart` + `apply()`，把新的 `SdpPendingOffer` 落到 session 并通过 `LocalDescription { kind: Offer }` 输出；driver 新增对应 `WebRtcDriverCommand::IceRestart`，命令进入 core 后由 `OfferReady` 事件回送给请求方。模块新增 `POST /api/v1/rtc/session/{id}/ice-restart` 端点：optional JSON body `{"keepLocalCandidates": true}`（默认保留本地候选），命令通过 driver `IceRestart` 触发后 `wait_answer` 收到新 SDP offer，HTTP 响应返回 `200 + application/sdp`。`Closed`/`Closing` 状态返回 `409`，未知 session 返回 `404`，core 内部对 `pending_offer` 已存在的情况返回结构化 `InvalidState`。新增 `ice_restart_emits_fresh_offer_for_existing_session`、`ice_restart_unknown_session_is_not_found` core 单测以及 `ice_restart_endpoint_returns_fresh_sdp_offer` 模块集成测试。
- **新增两个 fuzz 目标**：`crates/protocols/webrtc/fuzz/fuzz_targets/fuzz_url_parse.rs` 驱动 WHIP/WHEP HTTP 客户端的 URL 解析，断言三个不变量：never panic、host 非空、`request_target` 以 `/` 开头或为空。`fuzz_http_response.rs` 驱动 HTTP/1.1 响应解析（`parse_complete_response`），断言 body 不超过传入的 `max_body` 上界。`http_client.rs` 通过 `#[doc(hidden)] pub fn fuzz_parse_url_for_testing` / `fuzz_parse_http_response_for_testing` 暴露内部解析器，仅供 fuzz harness 使用，不进入稳定 API。
- **AGENTS §1.1 fuzz 目录**：fuzz crate 仍是独立 cargo workspace，不进入根 workspace members；目标列表全部加入 `crates/protocols/webrtc/fuzz/Cargo.toml`，可通过 `cd crates/protocols/webrtc/fuzz && cargo +nightly fuzz run <target>` 运行。

### 第七轮迭代（Phase 04 BWE 闭环 + Phase 05 PATCH ICE restart）

- **BWE → simulcast 主动降层闭环**：`SimulcastSelection` 新增 `bwe_estimate_bps` 字段与 `set_bwe_estimate` 方法，`elect_rid` 在 `Adaptive` 策略下根据 `(low_threshold, high_threshold)` 把估计 bin 到 low/mid/high 三档；`elect_adaptive` 负责具体的层选择（无估计 → 最高，低于 low → 最低，高于 high → 最高，中段且层数 ≥ 3 → 中间，否则最低）。模块配置新增 `bwe_low_threshold_kbps`（默认 600）和 `bwe_high_threshold_kbps`（默认 1800）字段，`validate()` 拒绝两值反向。bridge 注册表新增 `set_publish_bwe_estimate(session_id, bps)` API，driver 事件 worker 在收到 `WebRtcCoreEvent::Bwe` 后调用它把估计直接喂给 publish bridge，下一帧 `(mid, rid)` 触发重新选层。新增四个 `SimulcastSelection` 单测覆盖 no-estimate / low / mid / high 四种 binning 行为，以及一个 `module_rejects_inverted_bwe_thresholds_at_init` 集成测试覆盖反向阈值校验。
- **PATCH-driven ICE restart**：`compat::extract_trickle_ice_restart_creds` 解析 PATCH body 中的 `a=ice-ufrag:` + `a=ice-pwd:` 对，仅在两者都非空时返回 `Some`。`handle_session_patch` 在收到 trickle PATCH 后：先尝试解析候选行，再尝试解析 ICE-restart 凭据；若两者都为空则返回 `400 no_candidates`，否则把候选先送入 driver `AddRemoteCandidate`，凭据存在时再触发 `WebRtcDriverCommand::IceRestart { keep_local_candidates: true }`。PATCH 响应保持 `204 No Content` 与既有 WHIP/WHEP 客户端兼容。新增 `patch_with_ice_restart_creds_triggers_credential_rotation` 集成测试覆盖 happy path + body 为空时的 400 拒绝路径，以及五个 `extract_trickle_ice_restart_creds` 单测覆盖只 ufrag、只 pwd、空值、混合候选行等输入。

### 第八轮迭代（Phase 04 RTP 扩展 + Phase 05 P2P sendrecv 桥接）

- **RTP 头扩展透传到 codec layer**：`WebRtcMediaEvent::Frame` 新增 `meta: WebRtcFrameMeta` 字段，承载 `audio_level_dbov` / `voice_activity` / `video_orientation`（CVO 字节）/ `sequence_number`（首包 RTP 序号）/ `contiguous`（str0m reorder buffer 报告的连续性）；`session.rs` 在翻译 `Str0mEvent::MediaData` 时把 `data.ext_vals` 的 audio-level / voice-activity / video-orientation 与 `data.seq_range.start().as_u16()`、`data.contiguous` 一并写入 meta。bridge `push_frame` 把 `meta.contiguous=false` 翻译为 `FrameFlags::DISCONTINUITY`，`meta.sequence_number` 写入 `FrameSideData::SequenceNumber`，audio-level / voice-activity / video-orientation 通过 `FrameSideData::Metadata { key: "webrtc.<name>", value: ... }` 透传。这样 `cheetah-codec` ingress 适配器在构造 `WebRtcIngressContractView` 时不再需要回查 str0m，直接读取 AVFrame 上的 side data 即可，符合 AGENTS §4 关于 "core 输入输出必须是显式 Input/Output/Event/Timer 模型" 的约束。
- **P2P 双向 sendrecv 引擎桥接**：`handle_p2p_add` 现在默认 acquire 一个 publish bridge（与 WHIP 行为一致，处理 peer 推给 cheetah 的入向流）；可选的 body 字段 `playStreamName`（兼容 `playStream`）触发 `spawn_play(session_id, play_stream_key, driver)`，让同一个 driver session 既上行 publish 又下行 play，完成真正的 sendrecv。`stream_key`（publish 方向）与 `play_stream_key`（play 方向）显式分离，避免 publisher/subscriber 自循环；只有调用方明确写相同名字的极端调试场景才会形成回环。新增集成测试 `p2p_sendrecv_with_play_stream_acquires_publish_and_subscriber` 验证 `appName=live, streamName=p2p-tx, playStreamName=p2p-rx` 时引擎同时观察到 `live/p2p-tx` 的 publisher_active 与 `live/p2p-rx` 的 subscriber_count > 0。

### 第九轮迭代（Phase 04 弱网仿真 + Phase 05 互操作 scaffold）

- **REMB / TWCC 双轨 fallback 仿真**：新增 `telemetry_dual_track_bwe_and_remb_remain_independent` 单元测试。模拟 TWCC-driven BWE 估计（`merge_bwe`）与 REMB（`record_remb`）按时序到达：先 TWCC=2.5 Mbps，再 REMB=1.5 Mbps，断言两个字段独立可见；接下来 TWCC 升到 3 Mbps 不动 REMB；最后 REMB 降到 1.2 Mbps 不动 TWCC。这覆盖了 SMS / ZLM 中"本地 pacing 估计与远端接收方 hint 不一致"场景下 telemetry 的双轨可观测性。
- **netem-style 路由表丢包/乱序仿真**：在 `crates/protocols/webrtc/driver-tokio/src/route.rs` 新增两组单元测试：
  1. `reordered_old_path_packet_does_not_resurrect_active_binding` — 模拟 connection migration 后，旧路径的延迟包（reordered）到达时仍能通过 stale 集合解析到原 session，但活动路由不会被回滚到旧地址。
  2. `stale_route_drops_after_loss_burst_then_new_session_binds_cleanly` — 模拟长时间丢包（> stale_ttl）后 NAT rebinding：旧 session 的 stale ghost 已过期，新 session 落到同一远端地址不会 cross-route 到旧 session。
  这两组测试用确定性时间推进取代真实 netem，覆盖路由表在弱网/迁移条件下的 happy / sad path，避免 `usize` 下溢、stale ghost 复活、新会话路由污染等真实事故路径。真实媒体流下的 netem 集成测试需要外部工具（`tc qdisc`）配合，留待 CI 环境就绪后启用。
- **互操作 `--ignored` scaffold**：新增 `crates/protocols/webrtc/module/tests/interop.rs`，包含三个 `#[ignore]` 集成测试：`pion_pull_smoke`（要求 `WEBRTC_INTEROP_PION=1` + `WEBRTC_INTEROP_WHEP_URL`）、`gstreamer_push_smoke`（`WEBRTC_INTEROP_GST=1` + `WEBRTC_INTEROP_WHIP_URL`）、`browser_whip_whep_smoke`（`WEBRTC_INTEROP_BROWSER=1`）。当前 body 仅校验 env-var 契约，CI 配置 Pion / GStreamer / Selenium 容器后再扩展为完整 SDP 交换 + 媒体面状态机断言。`cargo test --test interop -- --ignored` 在所有 env var 未设置时早返回并视为 ok，与本地开发流程兼容。

### 第十轮迭代（Phase 05 DataChannel 主动发送 + 配置示例）

- **DataChannel 主动发送 HTTP 端点**：新增 `POST /api/v1/rtc/session/{id}/datachannel/send`，body schema：`{ "channel": <u32>, "payload": <string>, "binary": <bool?> }`。当 `binary=false`（默认）时 payload 作为 UTF-8 文本透传；`binary=true` 时 payload 必须是标准 base64 编码的字节数组（拒绝非法 base64 → 400）。对未知 session 返回 404，对 closed/closing session 返回 409，缺失字段返回 400，成功返回 202 Accepted（写入是异步的，SCTP 层可能仍因 buffer 满而 drop，drive 层会 surface diagnostic）。`compat::base64_decode` 复用 workspace `base64` 0.22 引擎；`module/Cargo.toml` 引入 `base64.workspace = true`。集成测试 `datachannel_send_endpoint_validates_inputs` 覆盖未知 session、缺 channel、缺 payload、bad base64、happy path 五条路径。
- **`config.example.yaml` 增加 webrtc 配置示例**：把所有新引入的字段（`bwe_low_threshold_kbps` / `bwe_high_threshold_kbps`、`simulcast_default_policy`、`datachannel_max_message_bytes`、`tcp_idle_timeout_ms`、`migration_route_ttl_ms` 等）以注释形式落到示例配置中，并对每个字段给出中文说明，方便部署同学开箱使用。所有字段保留默认值，整段以 `#` 注释包起；启用时取消注释即可。

### 第十一轮迭代（Phase 05 SMS SDP fixture 全套冒烟）

- **SMS SDP fixture 全套挂入冒烟**：新增 `crates/protocols/webrtc/core/tests/sms_sdp_fixtures.rs`，把 `vendor-ref/simple-media-server/Src/Webrtc/SdpExample/` 下的 `publish-offer-sms.sdp` / `publish-offer.sdp` / `offer.sdp` / `offer-simulcast.sdp` / `h265-offer.sdp` / `janus_offer.sdp` 全部接入 `WebRtcCore::AcceptOffer` 冒烟集合。每条 fixture 都断言：SDP 预处理保持 `v=0\r\n` / CRLF 终结、`Created` 与 `LocalDescriptionReady` lifecycle 事件齐全、`LocalDescription{Answer}` 输出非空且以 `v=0` 起始。新增 `sms_offer_simulcast_advertises_rid_layers` / `sms_h265_offer_advertises_h265` 两个 fixture 自检防止上游样例漂移。这关闭了 Phase 05 落地清单中"其它 SMS fixture（h265-offer/janus_*/offer-simulcast）已就位但暂未挂入冒烟集合"这一长期遗留项。`cargo test -p cheetah-webrtc-core --test sms_sdp_fixtures` 一次跑完全部 8 个测试，与既有的 `zlm_sdp_fixtures` 形成 SMS / ZLM 双家供应商对照覆盖。

### 第十二轮迭代（Phase 04 REMB 闭环升级到 simulcast 选层）

- **REMB → simulcast 选层闭环**：之前 REMB 只到 telemetry 层（`session.telemetry.remb_bitrate_bps`），不影响层选择。本轮把 `WebRtcRtcpFeedback::Remb { bitrate_bps, .. }` 同步喂给 publish bridge：
  - `SimulcastSelection` 新增 `remb_cap_bps` 字段与 `effective_cap_bps()` 助手；后者对 BWE 估计与 REMB cap 取 `min`，使得"远端要求降速但本地 TWCC 还很乐观"的常见错位真正会触发降层，而不是被本地高估值悄悄盖过。
  - bridge registry 暴露 `set_publish_remb_cap(session_id, bps)`；driver 事件 worker 在收到 `WebRtcCoreEvent::RtcpFeedback::Remb` 时同时调 `record_remb`（telemetry）与 `set_publish_remb_cap`（选层）。
  - `elect_adaptive` 参数从 `bwe_estimate_bps` 改名为 `effective_cap_bps`，语义上从"BWE 估计 bin 到三档"扩展为"effective cap bin 到三档"。
  - 三个新单测：`simulcast_adaptive_remb_cap_overrides_higher_bwe_estimate`（REMB 收紧时拉低层）、`simulcast_adaptive_remb_cap_does_not_relax_low_bwe`（REMB 抬高不放松 BWE 紧约束）、`simulcast_adaptive_remb_only_drives_election`（无 BWE 时 REMB 单独驱动）。这关闭了 phase-04 中"进一步的发送策略联动（REMB 比 TWCC 低时是否额外做 pacing cap）"中关于 simulcast 选层那一半。pacing-cap 与 NACK storm 触发降层仍是后续迭代。

### 第十三轮迭代（Phase 04 NACK storm 触发降层）

- **NACK storm 检测器**：`SimulcastSelection` 新增 `nack_storm_*` 字段族（`last_nack_in` / `nack_storm_recovery_left` / `nack_storm_threshold_per_sample` / `nack_storm_recovery_samples`），实现样本间增量比较的轻量风暴检测器：
  - `observe_nack_in(nack_in)` 在收到一个 stats 样本时计算与上次 `nack_in` 的差值。差值 ≥ 50（默认 `DEFAULT_NACK_STORM_THRESHOLD`）触发风暴，置 `nack_storm_recovery_left = 5`（`DEFAULT_NACK_STORM_RECOVERY_SAMPLES`）。
  - 每个后续小样本递减 `nack_storm_recovery_left`，归零后退出风暴态。
  - `in_nack_storm()` 助手暴露当前状态，`admit()` 读取后强制选最低 RID（绕过 BWE / REMB 决策），匹配 SMS / ZLM"风暴期间锁低层"语义。
- **driver → bridge 接线**：`WebRtcCoreEvent::Stats` 携带非零 `nack_in` 时，模块事件 worker 调用 `bridges.record_publish_nack_in(session_id, nack_in)` 把样本喂入风暴检测器；触发时打印 `warn!` 日志。
- **三组单测**：
  - `nack_storm_pins_to_lowest_layer_until_recovery`：BWE/REMB 都很乐观但 NACK 增量 60 ⇒ 强制选最低层。
  - `nack_storm_recovery_window_lifts_force_lowest_after_decay`：5 个亚阈值样本后退出风暴态，重新依 BWE 选层。
  - `nack_storm_sub_threshold_delta_does_not_trip`：每样本增量 10（远低于 50 阈值）持续 10 个样本不会触发风暴。
- 这关闭了 phase-04 中"NACK storm 触发降层"项；剩余仅 pacing-cap 与真实 netem 集成测试需要外部工具。

### 第十四轮迭代（Phase 02 driver smoke 测试矩阵补全）

- **driver_smoke 集成测试 +3 项**，对齐 phase-02 §2.9 测试矩阵：
  - `driver_rejects_unknown_udp_packet_without_panic`：在驱动监听端口外发送 16 字节非 STUN/DTLS/RTP 噪声，断言 driver 报告 `WebRtcDriverDiagnosticKind::UnroutedPacket` 且 UDP recv loop 仍存活。这是 §2.9 中"`driver_rejects_unknown_packet_without_panic`"对 UDP 路径的精确实现。
  - `driver_session_close_decrements_count_and_cleans_state`：`AcceptOffer` 后 `session_count()=1`，`StopSession` 后等到 `SessionClosed` 事件，`session_count()=0`。这是 §2.9 中"`driver_session_close_cleans_routes_and_timers`"中关于会话计数器复位的关键不变量。
  - `driver_command_queue_full_returns_explicit_error`：把 `command_queue_capacity=4` 缩到极小，循环 `try_send_command(StopSession)`，断言 saturated 队列会返回 `WebRtcSendError::QueueFull` 而非 panic / 静默阻塞。这是 §2.9 中"`driver_queue_full_does_not_block_recv_loop`"的契约层。
- **`WebRtcSendError` 公开**：`crates/protocols/webrtc/driver-tokio/src/lib.rs` 把 `WebRtcSendError` 加入 `pub use runner::*` 列表，使外部测试 / 调用者可以 match 队列满 / 通道关闭两种结构化错误，符合 AGENTS §2 关于"通过 trait / API 显式表达跨层能力"的约束。

### 第十五轮迭代（Phase 02 stats_snapshot 落地）

- **`WebRtcDriverHandle::stats_snapshot() -> WebRtcDriverStats`** 落地，对齐 phase-02 §2.2 公共 API 草案中长期占位的 `stats_snapshot` 接口：
  - 新增公开类型 `WebRtcDriverStats { local_udp_addr, local_tcp_addr, session_count, commands_accepted_total, events_emitted_total, unrouted_packets_total }`。
  - 新增内部 monotonic 计数器 `commands_accepted` / `events_emitted` / `unrouted_packets`（皆 `Arc<AtomicU64>`），由 `run_driver_core` 的命令分支、`WebRtcDriverHandle::recv_event` 与 `handle_datagram` 的 None 分支分别 `fetch_add(1, Ordering::Relaxed)`。
  - `run_driver_core` 与 `handle_datagram` 签名扩展接收 `commands_accepted` / `unrouted_packets` 的 Arc 句柄；`Arc::clone` 在 `spawn_driver` 一次完成，运行期不再分配。
  - `WebRtcDriverStats` 通过 `pub use runner::*` 公开；运维 dashboard / Prometheus exporter 可两次快照差值得到 cmd / event / unrouted 速率，无需触摸 driver 内部锁。
  - 集成测试 `driver_stats_snapshot_reports_counters_and_addresses` 覆盖：baseline 全 0、`AcceptOffer` 后 `commands_accepted_total ≥ 1`、`recv_event` 后 `events_emitted_total ≥ 1`、外部 UDP 噪声后 `unrouted_packets_total ≥ 1`、`local_udp_addr` 与 `handle.local_udp_addr()` 一致。

### 第十六轮迭代（Phase 04 §4.8 metrics 表面落地）

- **`WebRtcModuleMetrics` 聚合器**：新增 `crates/protocols/webrtc/module/src/metrics.rs`，把 phase-04 §4.8 documented metrics 列表落到聚合器：
  - 计数器：`packets_in` / `packets_out` / `nack_in` / `nack_out` / `rtx_sent` / `rtx_miss` / `pli` / `fir` / `twcc_feedback` / `simulcast_layer_switches` / `route_migrations` / `queue_drops`，皆 `AtomicU64` + `Ordering::Relaxed`，`fetch_add` 在线增量，无锁。
  - Gauge：`remb_bitrate_bps` / `bwe_estimate_bps`（last-writer-wins）由 `record_remb` / `record_bwe` 设置。
  - `add_stats_delta(WebRtcSessionStatsDelta)` 接受事件 worker 计算的"本次 stats 与上次 stats 的差值"，避免会话 ID 复用导致计数倒退。
- **`WebRtcModule::metrics_snapshot() -> WebRtcModuleMetricsSnapshot`** 操作员快照：组合聚合器原子计数与 registry 中 `WebRtcSessionRole::Publisher` / `Player` / `Bidirectional` 计数（gauge）。`Bidirectional` 同时算 publish + play（与 P2P sendrecv 双 lease 语义一致）。snapshot 字段直接对应 §4.8 文档名（去掉 `webrtc_` 前缀）。
- **事件 worker 接线**：`run_driver_event_worker` 新增 `metrics: Arc<WebRtcModuleMetrics>` 与 `last_session_stats` 缓存参数。
  - `RtcpFeedback::Pli/Fir` → `metrics.pli/fir.fetch_add(1)`。
  - `RtcpFeedback::Remb` → `metrics.record_remb(bps)`。
  - `Stats` → 从 `last_session_stats` 取上次快照，计算 `WebRtcSessionStatsDelta` 后 `metrics.add_stats_delta(&delta)`，`last_session_stats` 缓存更新；PLI/FIR 只走 RtcpFeedback arm 防双计。
  - `Bwe` → `metrics.record_bwe(bps)` + `metrics.inc_twcc_feedback()`（每个 BWE 事件视为一次 TWCC feedback 处理样本）。
  - `RouteUpdated` → `metrics.inc_route_migration()`。
  - `SessionClosed` → `last_session_stats.remove(session_id)` 防 ID 复用泄漏。
- 5 个新单元测试：`metrics_default_is_all_zero` / `add_stats_delta_accumulates` / `record_remb_and_bwe_are_last_writer_wins` / `assemble_combines_counters_and_gauges` / `module_metrics_snapshot_starts_at_zero`。模块单元测试从 92 → **97**。`metrics_snapshot()` 通过 `pub use metrics::*` 公开，运维侧可在不修改 module 源代码的情况下导出 Prometheus / OpenMetrics。

### 第十七轮迭代（Phase 04 metrics HTTP 暴露）

- **`GET /api/v1/rtc/metrics`** Prometheus 文本格式端点：模块 HTTP 服务新增 `handle_metrics()`，按 Prometheus 0.0.4 exposition 规范输出 17 行 metric block（每个 metric 三行：`# HELP <name> <text>` / `# TYPE <name> <kind>` / `<name> <value>`）。Counter 用 `_total` 后缀，gauge 用裸名；name 对应 phase-04 §4.8 documented list（带 `webrtc_` 前缀）。`Content-Type: text/plain; version=0.0.4; charset=utf-8`。
- **`GET /api/v1/rtc/metrics.json`** JSON 调试端点：相同字段以扁平 JSON 对象返回，方便 curl / Grafana JSON datasource / 自研 dashboard 直接读取，无需 Prometheus 中间件。
- **`compose_metrics_snapshot()` 助手**：`WebRtcHttpService` 持有 `metrics: Arc<WebRtcModuleMetrics>` 句柄（与模块共享同一个 `Arc`），调用时一次取 atomic 计数 + 一次 registry session 角色聚合，无重复锁。这样 metrics scrape path 与控制面 / 媒体面完全解耦，运维侧高频抓取不会影响热路径。
- **路由表注册**：`WebRtcModule::http_routes` 增加 `GET /metrics` 与 `GET /metrics.json` 两个 `HttpRouteDescriptor`，由引擎的 module manager 自动挂载。
- **集成测试 `metrics_endpoint_returns_prometheus_and_json`**：分别 GET `/metrics` 与 `/metrics.json`，断言：状态码 200、Prometheus 端的 Content-Type 是 `text/plain`、JSON 端能 `serde_json::from_slice` 解析、两端都包含 `webrtc_sessions_active` / `webrtc_pli_total` / `webrtc_route_migration_total` / `webrtc_bwe_estimate_bps` 等关键字段、空模块时 `sessions_active=0`。模块集成测试从 20 → **21**。



---

## 参考来源

| 来源 | 路径 / 链接 |
|------|-------------|
| SMS WebRTC API | `vendor-ref/simple-media-server/Src/Api/WebrtcApi.cpp` |
| SMS WebRTC core | `vendor-ref/simple-media-server/Src/Webrtc/` |
| SMS WebRTC SDP fixtures | `vendor-ref/simple-media-server/Src/Webrtc/SdpExample/` |
| SMS RTP codec packetizer/depacketizer | `vendor-ref/simple-media-server/Src/Rtp/Encoder/`、`vendor-ref/simple-media-server/Src/Rtp/Decoder/` |
| 本项目架构 | `SystemArchitecture.md`、`AGENTS.md` |
| 本项目 codec WebRTC contract | `crates/foundation/cheetah-codec/tests/future_protocol_adapter_contract.rs` |
| `str0m 0.19.0` crate docs | <https://docs.rs/str0m/0.19.0/str0m/> |
| `str0m` crate page / feature matrix | <https://docs.rs/crate/str0m/0.19.0> |
| `str0m` repository | <https://github.com/algesten/str0m> |

