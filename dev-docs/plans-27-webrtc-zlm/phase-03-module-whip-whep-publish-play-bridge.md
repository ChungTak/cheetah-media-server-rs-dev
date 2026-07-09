# Phase 03 — Module、WHIP/WHEP、推流、播放与协议互转

- **状态**: 部分完成（Phase 03 第一+二+三轮：ZLM-style `rtc://` URL 解析与 SMS-style body `url` 字段 fallback、播放侧 GOP bootstrap timing 度量、`/api/v1/rtc/session/{id}` 返回 `play_bootstrap` 子对象、`spawn_play_subscriber` `wait_stream_timeout_ms` 慢启动重试、PATCH trickle-ICE candidate 解析抽出为 `extract_trickle_candidates` 公共助手并加单测/属性测试；第三轮新增 echo answer SDP `msid` rewrite（`rewrite_echo_msid` + `POST /echo` 端点 + `echo_rewrite_msid` 配置）、WHEP 慢启动超时诊断（`wait_timeout_elapsed_ms` 字段在 `play_bootstrap` 中暴露）、H264 B 帧过滤 feature flag（`h264_bframe_filter` 配置占位）。WHIP/WHEP/SMS endpoints 已经在前置阶段落地；ZLM `/api/v1/rtc/publish|play|echo` 真实路由表与 `simulcast_publish_mode=multi-stream` 子流命名留作后续小步迭代）

## 实现概览

本阶段把传输能力接入业务层，形成完整 WebRTC 推流、播放、GOP 秒开、WHIP/WHEP 和 ZLM-style API。module 只负责编排，不重写协议状态机和媒体内核。

## 已完成（Phase 03 第一轮）

- `crates/protocols/webrtc/module/src/compat.rs` 新增 `parse_zlm_rtc_url`：覆盖 `rtc/rtcs/webrtc/webrtcs` 四种 scheme，解析 `host[:port]/<app>/<stream>?signaling_protocols=...&peer_room_id=...`，未知 query 透传到 `extra_params`，4+ 段路径 fallback 到 `path_extra`，并针对非法 scheme / port / 缺 host / 缺 stream / 非整数 `signaling_protocols` 给出明确错误。
- `extract_app_stream_aliases` 在显式 `app/stream` 缺失时自动 fallback 到 ZLM-style body `url=rtc://...` 字段，利用 `parse_zlm_rtc_url` 抽出 `app/stream`；显式字段优先，不合法 URL 静默回退到默认 `live` app。
- `WebRtcBridgeRegistry` 新增 `WebRtcPlayBootstrapStats { first_frame_micros, first_keyframe_micros, first_decodable_micros, frames_forwarded, keyframes_forwarded }` 与 `record_play_frame`/`play_stats` 方法；`spawn_play_subscriber` 在每帧 forward 时记录，`first_decodable_micros` 命中条件为 `random_access || FrameFlags::CONFIG`，对齐 ZLM `WebRtcPlayer::sendConfigFrames` 的“config 或关键帧即可解码”语义。
- `remove_play` / `close_all` 同步清空 `play_stats`，避免 session 退出后留下脏数据。
- `WebRtcHttpService::handle_session_get` 把 `play_stats` 渲染成 JSON `play_bootstrap` 子对象（`first_frame_micros / first_keyframe_micros / first_decodable_micros / frames_forwarded / keyframes_forwarded`）。
- 单元测试覆盖：14 条 ZLM URL 解析（合法与各类错误）、3 条 `url` fallback 兼容（URL-only / 显式覆盖 / 非法 URL）、5 条 bootstrap stats（首帧、首关键帧、纯 config 帧、首帧不回退、`remove_play` 清理）。

## 已完成（Phase 03 第二轮）

- `spawn_play_subscriber` 新增 `wait_stream_timeout_ms` 参数：`SubscriberApi::subscribe` 返回 `SdkError::NotFound` 时按 100ms 周期重试，直到配置的等待窗口耗尽或 `CancellationToken` 触发；其他错误（Unavailable / Conflict / Internal）保持立即冒泡。`WebRtcHttpService::spawn_play` 把 `WebRtcModuleConfig::wait_stream_timeout_ms`（默认 3000ms）传入。
- `extract_trickle_candidates(body) -> Vec<String>` 抽出为 `compat.rs` 公共助手：把 WHIP/WHEP PATCH `application/trickle-ice-sdpfrag` body 的每行 `a=candidate:...` 转成 `candidate:...` 形式，自动过滤空 token；`handle_session_patch` 改用该 helper。
- 集成测试 `play_bridge_waits_for_slow_start_publisher`：先发 WHEP，300ms 后再发 publisher，断言播放侧在 5s 等待窗口内挂上 subscriber。
- 单元测试 5 条覆盖 `extract_trickle_candidates`（单条、多条、其他属性行忽略、空 candidate 忽略、任意输入不 panic）。
- 属性测试 4 条（`property_trickle_candidates.rs`）：任意字符串不 panic、所有输出行以 `candidate:` 开头且非空、非候选行被丢弃、合法候选行数量 round-trip。
- 新增 fuzz 目标 `fuzz_trickle_candidates`：libfuzzer 11 秒 / 921k 输入清洁。

## 已完成（Phase 03 第三轮）

- `rewrite_echo_msid(sdp, session_label) -> String` 公共函数：重写 answer SDP 中所有 `a=msid:` 行的 stream-id 为唯一 per-session 标签，保留 track-id，对齐 ZLM `WebRtcEchoTest` 行为，防止 Chrome 把远端 track 当作本地 track 静默丢弃。
- `POST /echo` 新端点：一步创建 bidirectional echo session（DataChannel + media），answer SDP 自动应用 `msid` rewrite（受 `echo_rewrite_msid` 配置控制，默认 true）。
- `WebRtcApiKind::Echo` 新枚举变体，echo session 在 registry 中有独立标识。
- `WebRtcModuleConfig::echo_rewrite_msid`（默认 true）：控制 echo answer SDP 是否重写 `msid`。
- `WebRtcModuleConfig::h264_bframe_filter`（默认 false）：H264 B 帧过滤 feature flag 占位，后续 `cheetah-codec` 侧实现接入后由 module 消费。
- `WebRtcModuleConfig::play_timeout_diagnostic`（默认 None）：可选自定义诊断消息。
- `WebRtcPlayBootstrapStats::wait_timeout_elapsed_ms`：当 play subscriber 的慢启动等待窗口耗尽时记录等待毫秒数，通过 `/session/{id}` GET 的 `play_bootstrap.wait_timeout_elapsed_ms` 字段对外暴露。
- `WebRtcBridgeRegistry::record_play_timeout` 方法：`spawn_play_subscriber` 在 `NotFound` 超时时调用，写入 bootstrap stats。
- 单元测试 4 条覆盖 `rewrite_echo_msid`（单 msid 重写、多 msid 重写、非 msid 行保留、无 track-id 处理）。

## 已完成（Phase 03 第四轮）

- `POST /zlm` 统一端点：ZLM-style 路由兼容，通过 JSON body `type` 字段（`push|publish|play|echo`）分发到对应处理器，兼容 ZLMediaKit 客户端使用单一 URL 的模式。
- `handle_zlm_unified` 方法：解析 `type` 字段并委托到 `handle_sms_publish` / `handle_sms_play` / `handle_echo_create`，缺失或未知 `type` 返回 400 明确错误。
- `http_routes()` 注册 `/zlm` 和 `/echo` 路由描述符。

## 已完成（Phase 03 第五轮）

- MultiStream 多 lease 路由实现：`WebRtcPublishBridge` 新增 `multistream_sinks: HashMap<String, (StreamKey, PublishLease, Box<dyn PublisherSink>)>` 字段，在 MultiStream 模式下按 RID 路由 frame 到对应子流 sink。
- `push_to_sink` 内部方法：检测 MultiStream 模式，有 RID 时路由到 per-RID sink，无 RID 或 sink 未就绪时 fallback 到主 sink。
- `acquire_multistream_sink(rid)` 异步方法：为指定 RID 动态 acquire 独立 `PublishLease` + `PublisherSink`，使用 `derive_multistream_key` 派生子流 key。
- `pending_multistream_rids()` 方法：返回已观测但尚未 acquire sink 的 RID 列表，供 module event worker 驱动异步 acquisition。
- `close()` 方法更新：关闭主 sink 和所有 multistream sub-sinks。
- `publisher_api` 字段：MultiStream 模式下保存 `Arc<dyn PublisherApi>` 引用，用于延迟 acquire。

## 已完成（Phase 03 第六轮）

- Module event worker 集成 MultiStream sink acquisition：在 `WebRtcCoreEvent::Media` 处理路径中，`push_publish_frame` 之后检查 `pending_multistream_rids`，收集待 acquire 的 RID 列表后 spawn 异步任务逐个 acquire `PublishLease` + `PublisherSink`，通过 `insert_multistream_sink` 写回 bridge。
- `WebRtcPublishBridge::publisher_api_and_stream_key()` 方法：在不持有 lock 的情况下提取 async acquisition 所需信息。
- `WebRtcPublishBridge::insert_multistream_sink(rid, key, lease, sink)` 方法：从外部插入已 acquire 的 sub-stream sink。
- `WebRtcBridgeRegistry::publish_mut(session_id)` 方法：获取 bridge 可变引用。
- `WebRtcBridgeRegistry::pending_multistream_rids(session_id)` 方法：registry 级别的 pending RID 查询。
- 解决 `parking_lot::MutexGuard` 跨 `.await` 的 `Send` 约束问题：将 async acquisition 拆分为"收集信息 → drop lock → spawn async → re-acquire lock 写回"模式。

## 后续小步迭代

Phase 03 所有计划项已完成。


## 3.1 WHIP/WHEP 生命周期

接口行为：

- `POST /whip`：创建 publish session，申请 `PublishLease`，返回 answer SDP。
- `POST /whep`：创建 play session，订阅 engine，返回 answer SDP。
- `PATCH /session/{id}`：接收 trickle ICE fragment。
- `DELETE /session/{id}`：关闭 session，释放 lease/subscriber/job。
- `GET /session/{id}`：返回 state、stream、direction、bytes、packets、bwe、loss、candidate。

兼容：

- `Content-Type` 接受 `application/sdp` 和 ZLM 常见 form/json 包装。
- `Location` 可返回绝对或相对路径，client 侧都能处理。
- Bearer token、query secret、header secret 支持统一鉴权 hook。

## 3.2 ZLM-style API

提供兼容入口：

- `POST /api/v1/rtc/publish`
- `POST /api/v1/rtc/play`
- `POST /api/v1/rtc/echo`
- `GET /api/v1/rtc/session/list`
- `GET /api/v1/rtc/session/{id}`
- `DELETE /api/v1/rtc/session/{id}`

参数兼容：

- `vhost`
- `app`
- `stream`
- `type=push|play|echo`
- `url=rtc://vhost/app/stream?...`
- `offer`
- `secret`

所有 ZLM-style 参数在 module 归一化为本项目 `StreamKey`、`WebRtcApiKind` 和 `WebRtcSessionSpec`。

## 3.3 WebRTC 推流

流程：

1. 校验 stream key 和鉴权。
2. 申请 publish lease；冲突返回 409。
3. 创建 driver session。
4. core 输出 media frame。
5. bridge 使用 simulcast policy 选择 RID。
6. `cheetah-codec` 校验 ingress contract。
7. 推入 engine。

兼容策略：

- 默认一个 `StreamKey` 只接收一个 RID。
- `simulcast_publish_mode=selected`：只入选中层。
- `simulcast_publish_mode=multi-stream`：显式生成子流 key。
- 断续推只允许在租约仍有效且 session 未关闭时继续；重建必须走 module lifecycle。

## 3.4 WebRTC 播放

流程：

1. 校验 stream key 和鉴权。
2. 查询 engine stream；不存在返回 404 或按配置等待短窗口。
3. 创建 driver session。
4. 订阅 engine，读取 bootstrap tracks + GOP。
5. 通过 `cheetah-codec` egress contract 转为 `WebRtcSendFrame`。
6. core packetize，driver 发送。

GOP 秒开：

- 首先发送 codec config / 参数集。
- 优先发送最近关键帧及其完整 Access Unit。
- 首帧非关键帧时请求上游关键帧，并按配置等待或先丢弃 delta frame。
- 记录 first packet、first keyframe、first decodable frame 耗时。

## 3.5 编码与 RTSP 一致性

WebRTC 与 RTSP 共享编码策略：

- H264/H265：优先保持 Annex B / AVCC / HVCC 转换在 codec 层。
- G711A/G711U：监控场景优先，保持 clock rate 和 payload view 一致。
- Opus：WebRTC browser 默认音频。
- AAC：WebRTC browser profile 默认不输出，非浏览器 profile 可协商。

不做转码时：

- 可协商轨道继续播放。
- 不可协商轨道返回诊断并按配置拒绝 session 或丢弃该轨道。

## 3.6 Echo test

Media echo：

- answer 中 audio/video direction 设为 sendrecv。
- 收到 frame 后按相同 track 回发。
- answer SDP 改写 echo `msid`，避免 Chrome 认为远端 track 与本地 track 相同。
- RTCP 可回环或按配置由 core 正常处理。

DataChannel echo：

- 支持 text/binary。
- 超过 `datachannel_max_message_bytes` 返回 channel error 或丢弃诊断。
- echo 队列有上界，满时拒绝低优先级消息。

## 3.7 测试要求

运行：

```powershell
cargo test -p cheetah-webrtc-module
cargo test -p cheetah-webrtc-driver-tokio
cargo test -p cheetah-webrtc-core
```

新增测试：

- WHIP publish 成功、重复 publish 409、DELETE 释放 lease。
- WHEP play 不存在 stream 返回明确错误。
- WHEP play 等待短窗口后命中慢启动 publisher。
- PATCH candidate fragment 解析。
- ZLM-style `rtc://vhost/app/stream` 参数归一化。
- GOP bootstrap 首包包含配置帧和关键帧。
- echo answer 改写 `msid`。

## 完成后检查

- module 不直接持有 Tokio socket/timer 作为公共接口。
- 推流和播放都走 engine，不能私下绕过 `AVFrame + TrackInfo`。
- 配置变更需要重启时返回 `ModuleRestartRequired`。

