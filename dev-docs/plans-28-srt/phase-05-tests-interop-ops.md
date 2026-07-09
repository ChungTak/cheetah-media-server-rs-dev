# Phase 05 — 测试、互操作与运维

- **状态**: 部分完成
- **范围**: core/property/fuzz、driver 集成测试、module E2E、外部实体互操作、metrics、health、日志和验收流程
- **完成标准**: SRT 能在本地和 CI 环境稳定复现推流、拉流、relay、加密、弱网和断线重连场景

---

## 5.1 Core 单元测试

覆盖：

- Stream ID 解析。
- SRT URL 解析。
- 配置校验。
- mode 映射。
- StreamKey 安全校验。
- 错误信息稳定。

必须测试：

```text
#!::r=live/test,m=publish,u=alice
#!::r=live/test,m=request
live/test
/live/test
srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/test,m=publish
srt://:9000?mode=listener
```

命令：

```bash
cargo test -p cheetah-srt-core
```

---

## 5.2 Property Tests

新增 `cheetah-srt-property-tests`。

属性：

- Stream ID parser 不 panic。已覆盖。
- 任意 percent-encoded 输入要么解析为合法 `StreamKey`，要么返回错误。部分由 parser 单元测试和不 panic 属性覆盖。
- `parse -> normalize -> parse` 对合法 Stream ID 稳定。已覆盖 publish/user/token 路径。
- URL query 参数顺序不影响解析结果。已覆盖 known fields 和 token extras。
- 未知字段保留在 extras 中，不影响已知字段。已覆盖 Stream ID extras 和 URL extras。

命令：

```bash
cargo test -p cheetah-srt-property-tests
```

---

## 5.3 Fuzz

新增独立 cargo-fuzz workspace：

```text
crates/protocols/srt/fuzz/
├── Cargo.toml
├── README.md
└── fuzz_targets/
    ├── fuzz_stream_id.rs
    ├── fuzz_srt_url.rs
    └── fuzz_driver_packet.rs
```

目标：

- `fuzz_stream_id`: 任意 bytes 输入 Stream ID parser 不 panic。已建立。
- `fuzz_srt_url`: 任意 URL-like 输入 parser 不 panic。已建立。
- `fuzz_driver_packet`: 任意 UDP payload 输入 driver/core packet path 不 panic，不越界分配。已建立 listener-side `shiguredo_srt` packet path harness。

fuzz workspace 不加入根 workspace members。

当前 fuzz workspace 已通过：

```bash
cargo check --manifest-path crates/protocols/srt/fuzz/Cargo.toml
```

短 smoke 已通过：

```bash
cargo run --manifest-path crates/protocols/srt/fuzz/Cargo.toml --bin fuzz_stream_id -- -runs=100
cargo run --manifest-path crates/protocols/srt/fuzz/Cargo.toml --bin fuzz_srt_url -- -runs=100
cargo run --manifest-path crates/protocols/srt/fuzz/Cargo.toml --bin fuzz_driver_packet -- -runs=100
```

说明：

- 2026-06-15: `cargo fuzz run <target>` 会按 cargo-fuzz 默认逻辑查找仓库根 `fuzz/Cargo.toml`，与本计划采用的独立子 workspace `crates/protocols/srt/fuzz/` 不匹配；本地用 `cargo run --manifest-path ... --bin ... -- -runs=100` 验证 harness 可执行。
- 以上 smoke 不是带 coverage/sanitizer 的长时间 fuzz run；外部 CI 可按 `crates/protocols/srt/fuzz/README.md` 或调整 cargo-fuzz workspace 布局后运行长测。

---

## 5.4 Driver 集成测试

`crates/protocols/srt/driver-tokio/tests/driver_smoke.rs`：

- listener bind。
- caller/listener 本机握手。
- payload 双向发送。
- AES-128 passphrase 匹配。
- AES-256 passphrase 匹配。
- passphrase 不匹配失败。
- max connections。
- send queue capacity 边界。
- idle timeout。
- connect timeout。
- stats。
- encryption enabled 但 passphrase 为空时配置错误。

仍待补充或依赖外部环境：

- 慢 peer send queue overflow / event channel backpressure 压力测试。
- 弱网丢包、乱序、抖动。
- 长稳加密 key refresh 跑测。

命令：

```bash
cargo test -p cheetah-srt-driver-tokio
```

---

## 5.5 Module E2E 测试

`crates/protocols/srt/module/tests/srt_ingest.rs`：

- fake driver event worker 收到 `Connected + Payload` 后创建 publisher。
- demux 产生 TrackInfo 时调用 `update_tracks`。
- demux 产生 frame 时调用 `push_frame`。
- disconnected 释放 publish lease。

`srt_egress.rs`：

- 本地 stream snapshot 初始化 muxer。
- subscriber frame 被 mux 为 TS payload。
- source not found 超时关闭。
- track not ready 超时关闭。

`srt_relay.rs`：

- relay job 创建 ingress + egress 两段任务。
- source 断开释放 publisher。
- target 断开只重启 egress，不释放 source publish。

命令：

```bash
cargo test -p cheetah-srt-module
```

---

## 5.6 外部互操作

### FFmpeg 推流到 Cheetah

```bash
ffmpeg -re -stream_loop -1 -i sample.mp4 -c copy -f mpegts \
  "srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/ffmpeg,m=publish"
```

验证：

```bash
ffplay rtsp://127.0.0.1:554/live/ffmpeg
ffplay rtmp://127.0.0.1/live/ffmpeg
ffplay http://127.0.0.1:8088/live/ffmpeg/index.m3u8
```

当前执行记录：

- 2026-06-12: `ffmpeg -re -stream_loop -1 -i test_media_files/camera_h265.mp4 -c copy -f mpegts "srt://127.0.0.1:9000?mode=caller&streamid=%23!::r=live/srt_ffmpeg,m=publish"` 可连接 Cheetah SRT listener。
- 2026-06-12: server 日志确认 `SRT peer connected` 和 Stream ID `#!::r=live/srt_ffmpeg,m=publish`。
- 2026-06-12: 推流期间 `ffprobe rtmp://127.0.0.1/live/srt_ffmpeg` 可读取到 HEVC stream，SRT -> engine -> RTMP 路径验证通过。
- 2026-06-12: 使用 FFmpeg lavfi 生成标准 H264/AAC 推流到 `live/srt_h264`，推流期间 `ffprobe rtmp://127.0.0.1/live/srt_h264` 可读取到 H264 stream，SRT -> engine -> RTMP 路径再次验证通过。
- 2026-06-12: 修复 TS demux track metadata 更新后，使用 FFmpeg lavfi 生成标准 H264/AAC 推流到 `live/srt_hls`，`ffprobe rtmp://127.0.0.1/live/srt_hls` 可读出 H264 640x360 和 AAC 48k；control streams 显示 H264/AAC track 均为 `Ready`；HLS playlist 返回 200 并生成 TS segment，`seg_0.ts` 返回 200 `video/mp2t`。
- 2026-06-12: 之前 control/HLS HTTP `502 Bad Gateway` 记录由本机代理环境触发：`http_proxy/all_proxy` 生效且 `no_proxy` 缺少 `127.0.0.1`；本地验证命令统一使用 `curl --noproxy '*'`。
- 2026-06-12: 使用临时 `CHEETAH_CONFIG` 将 RTSP listen 改为 `0.0.0.0:8554` 后，FFmpeg lavfi H264/AAC SRT 推流到 `live/srt_rtsp`；control streams 显示 H264/AAC track 均为 `Ready`，`ffprobe -rtsp_transport tcp rtsp://127.0.0.1:8554/live/srt_rtsp` 可读出 H264 640x360 与 AAC 48k。RTSP 默认配置仍是特权端口，非 root 环境需显式配置非特权 listen。
- 2026-06-15: 使用临时 `CHEETAH_CONFIG` 将 WebRTC UDP listen 配为 `127.0.0.1:0`，启动 `cheetah-server --no-default-features --features "srt rtmp hls webrtc"`；FFmpeg lavfi H264/AAC SRT 推流到 `live/srt_webrtc` 后，control streams 显示 H264/AAC track 均为 `Ready`。
- 2026-06-15: 使用 `crates/protocols/webrtc/module/tests/fixtures/minimal_offer.sdp` 对 `/api/v1/rtc/whep?app=live&stream=srt_webrtc` 发起 WHEP POST，返回 `201 Created`、`content-type: application/sdp`、`location: /api/v1/rtc/session/webrtc-session-1`，并生成 SDP answer；该结果验证 SRT 摄入流可进入 WebRTC WHEP 信令 answer 路径。真实浏览器/ICE/DTLS/SRTP 媒体播放仍需外部 WHEP 客户端验证。
- 2026-06-15: 使用 `whep-browser-smoke.html` + Chrome headless 对 SRT 摄入流发起真实 WHEP 浏览器 play；浏览器输出 `whep_status=201`、`track=video`、`track=audio`。初测 answer 中 `remote_candidates=0`，定位到 WebRTC driver 未把 UDP listener/public IP 注入 str0m local candidates；已修复并补 `driver_accept_offer_advertises_configured_public_ip_candidate` 回归测试。
- 2026-06-15: 修复 candidate 后，Chrome 可看到 `remote_candidates=1`；无 `a=end-of-candidates` 时抓包显示 UDP 7000 为 `0 packets captured`，补 EOC 后 Chrome 会向 UDP 7000 发送 STUN。已将 `a=end-of-candidates` 注入固化到 WebRTC driver LocalDescription 输出。
- 2026-06-16: 复测真实 Chrome WHEP 时定位到 ICE/RTP 失败根因不是 SRT ingress，而是 WebRTC play bridge 在收到 AAC 音频且无 Opus 转码能力时主动 `StopSession`，随后所有 STUN 包都变成 `UnroutedPacket`。已将默认 `audio_output_strategy=auto` 改为丢弃不可输出音频并保持视频播放；显式 `transcode_to_opus` / `passthrough` 仍保留失败语义。
- 2026-06-16: 使用 `whep-browser-smoke.html` + Chrome headless 复测 SRT H264/AAC 摄入流，页面输出 `whep_status=201`、`remote_candidates=1`、`track=video`、`track=audio`、`ice=connected`、`pc=connected`、`inbound_packets=84`、`RESULT: PASS`。手动 `pc.getStats()` 也确认 video inbound RTP 收到 5095 包、解码 949 帧，transport `dtlsState=connected`、`iceState=connected`。当前 WebRTC 浏览器路径验证范围为 H264 视频；AAC 音频因无 AAC -> Opus 转码能力按降级策略丢弃，音频转码仍是后续能力。
- 2026-06-16: 扩展 codec 适配后，使用 FFmpeg lavfi 生成 H264/MP3 MPEG-TS over SRT 推到 `live/srt_h264_mp3`；control streams 显示 H264 Ready 与 MP3 Ready（44100 Hz、2 channels），`ffprobe rtmp://127.0.0.1/live/srt_h264_mp3` 可读出 H264 320x180 与 MP3 44100 stereo。该记录验证 MPEG audio 首帧头细分 MP2/MP3、采样率/声道推导和 SRT -> RTMP 数据面。
- 2026-06-16: 针对用户指出的“当前只适配 H264”问题，补充真实 FFmpeg 编码器互操作：`libx265` 推入 `live/srt_h265_codec_retry` 后 control streams 显示 H265 `Ready`；`libvpx` 推入 `live/srt_vp8_codec` 后显示 VP8 `Ready`；`libvpx-vp9 -deadline realtime -lag-in-frames 0` 推入 `live/srt_vp9_codec_retry` 后显示 VP9 `Ready`；`libaom-av1 -cpu-used 8 -lag-in-frames 0` 推入 `live/srt_av1_codec_retry` 后显示 AV1 `Ready`。该验证覆盖 FFmpeg VP8/VP9/AV1 MPEG-TS 的无 descriptor `0x06` 私有流形态。

### Cheetah 推流到 FFmpeg listener

```bash
ffmpeg -listen 1 -i "srt://0.0.0.0:9100?mode=listener" -c copy out.ts
```

Cheetah 配置 egress job 推到 `127.0.0.1:9100`。

当前执行记录：

- 2026-06-15: 使用 `srt-live-transmit` 作为远端 SRT listener，配置 Cheetah `egress_jobs` 将 RTMP 发布的 `live/egress_job` 主动推到 `127.0.0.1:9100`；listener 输出 `/tmp/cheetah_srt_egress_job.ts` 为 250KB，ffprobe 可识别 H264 与 AAC 48k。该路径验证本地流 SRT caller push 到远端 listener 可用。

### Cheetah SRT listener request/play 拉流

```bash
ffprobe -hide_banner -show_entries stream=codec_name,width,height,sample_rate,channels \
  -of json "srt://127.0.0.1:9000?mode=caller&streamid=%23!::r=live/rtmp_to_srt,m=request"
```

当前执行记录：

- 2026-06-12: 使用 FFmpeg lavfi 通过 RTMP 发布 H264/AAC 到 `live/rtmp_to_srt`。
- 2026-06-12: control streams 显示 `live/rtmp_to_srt` publisher active，H264/AAC track 均为 `Ready`。
- 2026-06-12: 使用 FFprobe 通过 SRT caller/request 从 Cheetah 拉同一流，成功读取 MPEG-TS 中的 H264 与 AAC 48k；SRT metrics 显示 `srt_play_connections_total=1`、`srt_bytes_out_total` 增长。
- 2026-06-12: 该 egress 验证中 ffprobe 对 H264 width/height 显示为 0，原因是 RTMP 源当前 track metadata 未携带宽高；编码流输出路径已验证。

### OBS 推流

OBS 自定义服务：

```text
Server: srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/obs,m=publish
Key: empty
```

验证：

- 首帧延迟。
- 音视频 tracks。
- HLS playlist 可生成。
- RTSP/RTMP 可播放。

当前执行记录：

- 2026-06-15: 当前环境可执行 `/usr/bin/obs`，版本为 `OBS Studio - 32.1.2`。
- 2026-06-15: 用户通过 OBS GUI 配置 profile 后，`service.json` 为 `rtmp_custom`，server 指向 `srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/obs,m=publish&latency=300`，scene collection 包含 `color_source_v3` 源。
- 2026-06-15: 启动 `cheetah-server --no-default-features --features "srt rtmp hls"`，执行 `timeout 45 obs --startstreaming --profile "未命名" --collection "未命名" --multi --minimize-to-tray --disable-missing-files-check --verbose`；OBS 日志显示 `==== Streaming Start`、`ffmpeg_mpegts_muxer`、`libsrt version 1.5.3 loaded`，session summary 为 `time elapsed [43.5 sec]`、`total bytes sent [10.0 MB]`、`bytes dropped [0.0 %]`、`SRT connection closed`。`timeout` 结束 OBS 后日志包含 core 提示，但发生在 session summary 和连接关闭之后。
- 2026-06-15: Cheetah control streams 显示 `live/obs` publisher active，H264/AAC track 均为 `Ready`；RTMP ffprobe 读出 H264 1280x720 与 AAC 48k stereo；HLS playlist 返回 200 并生成 `seg_0.ts` 到 `seg_2.ts`；SRT metrics 断开后显示 `connections_total=1`、`publish_connections_total=1`、`bytes_in_total=19547440`、`disconnect_total=1`、`driver_errors_total=0`。

### libsrt / srt-live-transmit

```bash
srt-live-transmit file://sample.ts "srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/libsrt,m=publish"
```

当前执行记录：

- 2026-06-15: `vendor-ref/srt/srt-live-transmit -version` 显示 SRT Library 1.5.5。
- 2026-06-15: 使用 FFmpeg lavfi 生成 MPEG-TS，经 `srt-live-transmit file://con "srt://127.0.0.1:9000?mode=caller&streamid=%23!::r=live/libsrt,m=publish"` 推到 Cheetah；control streams 显示 H264/AAC track 均为 `Ready`，RTMP ffprobe 可读 H264 640x360 和 AAC 48k，HLS playlist 返回 200 并生成 TS segment。

### SRT Relay

当前执行记录：

- 2026-06-15: 使用 `srt-live-transmit` 在 `127.0.0.1:9101` 模拟 source listener、在 `127.0.0.1:9102` 模拟 target listener，配置 Cheetah `relay_jobs` 从 source 拉流并推送到 target。
- 2026-06-15: 初测发现 target listener 输出 0 字节，修复 egress 等待 Ready tracks 后复测通过；target 输出 `/tmp/cheetah_srt_relay_out.ts` 为 42KB，ffprobe 可识别 H264 与 AAC 48k。

---

## 5.7 弱网和长稳

弱网场景：

- 1% packet loss。
- 5% packet loss。
- 50ms jitter。
- 100ms reorder。
- 带宽限制。

Linux 示例：

```bash
sudo tc qdisc add dev lo root netem loss 5% delay 50ms 10ms reorder 5%
sudo tc qdisc del dev lo root
```

观测：

- SRT RTT。
- NAK count。
- retransmit count。
- TLPKTDROP count。
- engine frame drop。
- RTSP/RTMP/HLS/WebRTC 播放连续性。

长稳：

- 单路 24h 推流。
- 100 路短连接反复 publish。
- 20 路 relay job 自动重连。
- 加密连接 key refresh 长跑。

当前环境记录：

- 2026-06-15: 当前 shell 复测 `sudo -n true` 与 `sudo -n tc qdisc show dev lo` 均成功。
- 2026-06-15: 已执行 loopback netem 短互操作：`sudo -n tc qdisc add dev lo root netem loss 1% delay 50ms 10ms reorder 1%`，使用 FFmpeg lavfi H264/AAC 通过 SRT publish 到 `live/netem`；弱网期间 HLS playlist 返回 200 并生成 `seg_0.ts` 到 `seg_3.ts`，RTMP ffprobe 读出 H264 640x360 与 AAC 48k。
- 2026-06-15: 已执行 loopback netem 5% loss 短互操作：`loss 5% delay 50ms 10ms reorder 5%`，使用 FFmpeg lavfi H264/AAC 通过 SRT publish 到 `live/netem5`；弱网期间 HLS playlist 返回 200 并生成 segment，RTMP ffprobe 读出 H264 640x360 与 AAC 48k。
- 2026-06-15: 已执行 loopback rate limit 短互操作：`rate 1200kbit delay 20ms 5ms`，输入约 650kbit H264 + 96kbit AAC，通过 SRT publish 到 `live/netem_rate`；HLS playlist 返回 200，RTMP ffprobe 读出 H264 640x360 与 AAC 48k。
- 2026-06-15: 已执行更激进丢包观测：`loss 12% delay 80ms 30ms reorder 10%`，使用 FFmpeg lavfi H264/AAC 通过 SRT publish 到 `live/tlpktdrop`；HLS playlist 返回 200 并生成 `seg_0.ts`，SRT metrics 显示 `lost_packets_total=4249`、`duplicate_packets_total=3296`、`rtt_micros=124188`、`jitter_micros=415037`。
- 2026-06-15: 已验证慢 peer / egress backpressure：将 `modules.srt.egress.send_queue_capacity` 设为 `0`，RTMP 发布 `live/slow_peer` 后用 SRT request/play 拉流；metrics 显示 `srt_send_queue_full_total=1151`、`srt_driver_errors_total=1151`、`srt_play_connections_total=1`。
- 2026-06-15: 每个 netem 场景测试结束后均执行 `sudo -n tc qdisc del dev lo root`，最终 `sudo -n tc qdisc show dev lo` 恢复为 `qdisc noqueue`。
- 2026-06-15: `shiguredo_srt` 当前 stats 未暴露独立 TLPKTDROP 计数字段，现阶段通过 lost/duplicate/RTT/jitter 与 HLS/RTMP 连续性间接观测；24h 长稳仍待继续验证。
- 2026-06-15: 已执行 100 路短连接反复 publish 压测：使用 FFmpeg lavfi 快速发起 100 次 SRT publish 到 `live/short_<n>`，结果 `ok=100 fail=0`；SRT metrics 显示 `connections_total=100`、`publish_connections_total=100`、`disconnect_total=100`、`connections_active=0`，control streams 最终为空。

---

## 5.8 Metrics

已实现 SRT module 本地 HTTP 指标端点：

- `GET /srt/metrics`: Prometheus text exposition。
- `GET /srt/metrics.json`: operator JSON。

当前已覆盖指标：

```text
srt_connections_active
srt_connections_total
srt_publish_connections_total
srt_play_connections_total
srt_bytes_in_total
srt_bytes_out_total
srt_packets_in_total
srt_packets_out_total
srt_retransmit_total
srt_lost_packets_total
srt_duplicate_packets_total
srt_send_queue_depth
srt_recv_queue_depth
srt_rtt_micros
srt_jitter_micros
srt_key_refresh_total
srt_disconnect_total
srt_driver_errors_total
srt_send_queue_full_total
```

仍待外部弱网/互操作验证或后续诊断补充：

```text
srt_nak_total
srt_tlpktdrop_total
cheetah_srt_demux_sync_loss_total{stream}
```

指标维度要限制 cardinality：

- stream label 可配置开启；默认生产环境只按 module/role/mode 汇总。
- peer addr 不作为 metric label，只进 debug log。

---

## 5.9 Health 和日志

Health：

- listener bind 状态。
- active connections。
- failed jobs。
- repeated reconnect jobs。
- encryption config validity。

日志：

- connect/disconnect：info。
- auth reject：warn。
- demux sync loss 高频：按阈值 warn。
- packet loss/retransmit 高频：按阈值 warn。
- per-packet hot path 不打 warn/info。

断开日志必须包含：

- peer id。
- role/mode。
- stream key。
- remote addr。
- bytes in/out。
- duration。
- reason。

---

## 5.10 最低提交前检查

SRT 代码改动后至少执行：

```bash
cargo fmt
cargo clippy -p cheetah-srt-core
cargo clippy -p cheetah-srt-driver-tokio
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-core
cargo test -p cheetah-srt-driver-tokio
cargo test -p cheetah-srt-module
```

涉及共享媒体层：

```bash
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo test -p cheetah-ts-core
```

涉及 server feature：

```bash
cargo check -p cheetah-server --features "srt rtsp rtmp hls webrtc"
```

外部互操作阶段：

```bash
cargo test -p cheetah-srt-module --test srt_external_interop -- --ignored
```

每个 ignored 外部测试必须在 docstring 写明：

- 需要启动的外部进程。
- 环境变量。
- 端口。
- 成功观测。
- 失败 artifact 路径。
