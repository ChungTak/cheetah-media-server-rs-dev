# SRT 运维与互操作验收清单

- **状态**: 已建立清单，FFmpeg/libsrt/srt-live-transmit/OBS 主路径已完成本机互操作；SRT TS codec 矩阵已扩展到 H265/H266/VP8/VP9/AV1/MJPEG/ADPCM/MP3 等 passthrough 格式，并已验证 FFmpeg H265/VP8/VP9/AV1 MPEG-TS over SRT 摄入到 Cheetah control streams 均为 `Ready`；WebRTC answer candidate/EOC 已修复，Chrome WHEP 已验证 SRT H264 视频 RTP，AAC/MP3 音频在无 Opus 转码能力时按降级策略丢弃；长稳仍待外部执行
- **范围**: FFmpeg/OBS/libsrt 互操作、弱网、长稳、metrics、health、日志和失败 artifact 采集

---

## 前置条件

工具：

```bash
ffmpeg -version
ffplay -version
srt-live-transmit -version
```

服务：

```bash
cargo run -p cheetah-server --features "srt rtsp rtmp hls webrtc"
```

样本：

- `sample.mp4`: H264/AAC、H265/AAC 或 H264/MP3。
- `sample.ts`: MPEG-TS over H264/AAC、H265/AAC、H264/MP3；自生成样本可覆盖 H266/VP8/VP9/AV1/MJPEG/ADPCM passthrough 矩阵。

端口：

- SRT listener: `127.0.0.1:9000`
- RTSP: `127.0.0.1:554`
- RTMP: `127.0.0.1:1935`
- HLS HTTP: `127.0.0.1:8088`
- FFmpeg SRT listener: `127.0.0.1:9100`

---

## FFmpeg 推流到 Cheetah

推流：

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

成功标准：

- Cheetah 日志出现 SRT connected 和 publish lease acquired。
- RTSP/RTMP/HLS 至少一种播放连续 5 分钟无中断。
- 音视频 track 数与输入一致。
- 断开 FFmpeg 后 publish lease 释放。

执行记录：

- 2026-06-12: FFmpeg SRT publish 已连接 Cheetah，server 日志出现 `SRT peer connected`，Stream ID 为 `#!::r=live/srt_ffmpeg,m=publish`。
- 2026-06-12: 推流期间 FFprobe 访问 `rtmp://127.0.0.1/live/srt_ffmpeg` 成功读取 HEVC stream，SRT -> RTMP 路径通过最小互操作验证。
- 2026-06-12: 使用 FFmpeg lavfi 生成标准 H264/AAC 推流到 `live/srt_h264` 后，FFprobe 访问 `rtmp://127.0.0.1/live/srt_h264` 成功读取 H264 stream。
- 2026-06-12: 修复 TS demux track metadata 更新后，使用 FFmpeg lavfi 生成标准 H264/AAC 推流到 `live/srt_hls`，FFprobe 访问 `rtmp://127.0.0.1/live/srt_hls` 成功读取 H264 640x360 与 AAC 48k；control streams 显示 H264/AAC track 均为 `Ready`；HLS playlist 返回 200 并生成 TS segment，`seg_0.ts` 返回 200 `video/mp2t`。
- 2026-06-12: 之前 HLS/control HTTP `502 Bad Gateway` 记录由本机代理环境触发：`http_proxy/all_proxy` 生效且 `no_proxy` 缺少 `127.0.0.1`；本地 curl 验证需使用 `--noproxy '*'` 或补齐 `NO_PROXY=127.0.0.1,localhost`。
- 2026-06-12: 使用临时 `CHEETAH_CONFIG` 将 RTSP listen 改为 `0.0.0.0:8554` 后，FFmpeg lavfi H264/AAC SRT 推流到 `live/srt_rtsp`，FFprobe 访问 `rtsp://127.0.0.1:8554/live/srt_rtsp` 成功读取 H264 640x360 与 AAC 48k。RTSP 默认配置仍是特权端口，非 root 环境需显式配置非特权 listen。
- 2026-06-15: 启动带 `webrtc` feature 的 server，并用 FFmpeg lavfi H264/AAC SRT 推流到 `live/srt_webrtc`；control streams 显示 H264/AAC track 均为 `Ready`。
- 2026-06-15: 使用 WebRTC module 测试 fixture `minimal_offer.sdp` 对 `/api/v1/rtc/whep?app=live&stream=srt_webrtc` 发起 WHEP POST，返回 `201 Created`、`content-type: application/sdp`、`location: /api/v1/rtc/session/webrtc-session-1`，并生成 SDP answer。该结果只验证 WHEP 信令 answer 路径，真实媒体播放仍需浏览器或 WHEP 客户端完成 ICE/DTLS/SRTP。
- 2026-06-15: 使用 Chrome headless 和 `whep-browser-smoke.html` 对 SRT 摄入流发起真实 WHEP 浏览器 play；浏览器收到 WHEP 201 和 video/audio track 事件。初测 answer 无 server candidate，已修复 WebRTC driver local candidate 注入；随后确认无 `a=end-of-candidates` 时 Chrome 不向 UDP 7000 发 STUN，已将 EOC 注入固化到 driver 输出。
- 2026-06-16: 真实 Chrome WHEP 复测定位到旧失败根因为 WebRTC bridge 收到 AAC 后因无 Opus 转码能力关闭会话。默认 `audio_output_strategy=auto` 已改为丢弃不可输出音频并保持视频播放；显式 `transcode_to_opus` / `passthrough` 仍保持失败语义。
- 2026-06-16: SRT H264/AAC 摄入到 Chrome WHEP 页面复测通过：页面输出 `ice=connected`、`pc=connected`、`inbound_packets=84`、`RESULT: PASS`；手动 `pc.getStats()` 确认 video inbound RTP 5095 包、949 帧已解码。当前浏览器互操作验收为 H264 视频通过，AAC 音频需后续 AAC -> Opus 转码能力。
- 2026-06-16: 为更多音视频格式适配扩展 `cheetah-codec` MPEG-TS 主路径：H266/VVC egress 关键帧注入 AUD + VPS/SPS/PPS，MJPEG/ADPCM 通过 PMT 私有 registration descriptor 识别，MPEG audio 从 PES 首帧头细分 MP2/MP3 并推导 sample rate/channel。`cargo test -p cheetah-codec --test ts_codec_matrix` 覆盖 H264/H265/H266/AV1/VP8/VP9/MJPEG 与 AAC/G711A/G711U/Opus/MP3/MP2/ADPCM roundtrip。
- 2026-06-16: FFmpeg H264/MP3 MPEG-TS over SRT 推到 `live/srt_h264_mp3`，control streams 显示 H264 Ready 与 MP3 Ready（44100 Hz、2 channels），RTMP ffprobe 可读 H264 320x180 与 MP3 44100 stereo。
- 2026-06-16: FFmpeg VP8/VP9/AV1 MPEG-TS 输出使用 `stream_type=0x06` 且无 PMT ES descriptor，ffprobe 会显示为 `bin_data`；`cheetah-codec` 已通过首个 PES payload 延迟识别 VP8/VP9/AV1，AV1 同步提取 sequence header。使用 `libx265`、`libvpx`、`libvpx-vp9 -deadline realtime -lag-in-frames 0`、`libaom-av1 -cpu-used 8 -lag-in-frames 0` 推 SRT 后，control streams 分别显示 H265、VP8、VP9、AV1 track `Ready`。

---

## Cheetah 推流到 FFmpeg Listener

启动 FFmpeg listener：

```bash
ffmpeg -listen 1 -i "srt://0.0.0.0:9100?mode=listener" -c copy out.ts
```

Cheetah 配置：

```yaml
modules:
  srt:
    egress_jobs:
      - name: ffmpeg-listener
        enabled: true
        source_stream_key: "live/ffmpeg"
        target_url: "srt://127.0.0.1:9100?mode=caller&streamid=#!::r=live/ffmpeg,m=publish"
        retry_backoff_ms: 1000
        max_retry_backoff_ms: 30000
```

成功标准：

- FFmpeg listener 收到数据并持续写入 `out.ts`。
- 断开 FFmpeg listener 后，SRT job 按指数退避重连。
- 重新启动 FFmpeg listener 后，job 自动恢复。

执行记录：

- 2026-06-15: FFmpeg listener 模式初测返回 listener I/O error，改用 `srt-live-transmit` 作为远端 SRT listener 后，Cheetah `egress_jobs` 将 RTMP 发布的 `live/egress_job` 主动推到 `127.0.0.1:9100`；输出 `/tmp/cheetah_srt_egress_job.ts` 为 250KB，ffprobe 可识别 H264 与 AAC 48k。

---

## Cheetah SRT Listener Request/Play 拉流

执行记录：

- 2026-06-12: 使用 FFmpeg lavfi 通过 RTMP 发布 H264/AAC 到 `live/rtmp_to_srt`。
- 2026-06-12: control streams 显示 `live/rtmp_to_srt` publisher active，H264/AAC track 均为 `Ready`。
- 2026-06-12: 使用 `ffprobe "srt://127.0.0.1:9000?mode=caller&streamid=%23!::r=live/rtmp_to_srt,m=request"` 从 Cheetah SRT listener 拉流，成功读取 MPEG-TS 中的 H264 与 AAC 48k；SRT metrics 显示 `srt_play_connections_total=1`、`srt_bytes_out_total` 增长。
- 2026-06-12: 该 egress 验证中 ffprobe 对 H264 width/height 显示为 0，原因是 RTMP 源当前 track metadata 未携带宽高；编码流输出路径已验证。

---

## OBS 推流

OBS 自定义服务：

```text
Server: srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/obs,m=publish
Key: empty
```

成功标准：

- Cheetah 日志中 stream key 为 `live/obs`。
- 首帧进入引擎。
- HLS playlist 生成，RTSP/RTMP 可播放。

当前状态：

- 2026-06-15: 当前环境可执行 `/usr/bin/obs`，版本为 `OBS Studio - 32.1.2`。
- 2026-06-15: 用户通过 OBS GUI 配置自定义 SRT server 后，profile `未命名` 的 `service.json` 指向 `srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/obs,m=publish&latency=300`，scene collection 有 `color_source_v3`。
- 2026-06-15: 执行 `obs --startstreaming --profile "未命名" --collection "未命名"`，OBS 使用 `ffmpeg_mpegts_muxer` + libsrt 推流 43.5 秒，发送 10.0 MB；Cheetah 侧 `live/obs` H264/AAC Ready，RTMP ffprobe 读出 H264 1280x720 与 AAC 48k stereo，HLS 生成 `seg_0.ts` 到 `seg_2.ts`，SRT metrics 无 driver error。

---

## libsrt / srt-live-transmit

```bash
srt-live-transmit file://sample.ts \
  "srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/libsrt,m=publish"
```

成功标准：

- Cheetah 能识别 Stream ID。
- TS demux 产生 track 和 frame。
- 客户端播放连续 5 分钟。

当前状态：

- 2026-06-15: 原生工具位于 `vendor-ref/srt/srt-live-transmit`，需通过 `LD_LIBRARY_PATH=vendor-ref/srt` 加载本地 libsrt；`-version` 显示 SRT Library 1.5.5。
- 2026-06-15: 使用 FFmpeg lavfi 生成 MPEG-TS，经 `srt-live-transmit file://con "srt://127.0.0.1:9000?mode=caller&streamid=%23!::r=live/libsrt,m=publish"` 推到 Cheetah；control streams 显示 H264/AAC track 均为 `Ready`，RTMP ffprobe 可读 H264 640x360 和 AAC 48k，HLS playlist 返回 200 并生成 TS segment。

## SRT Relay

场景：

- `srt-live-transmit` source listener。
- Cheetah `relay_jobs` caller 拉 source，并将中间流发布为本地 stream。
- Cheetah egress caller 推到 `srt-live-transmit` target listener。

执行记录：

- 2026-06-15: source listener -> Cheetah relay -> target listener 本机链路已验证；修复 egress tracks Ready 竞态后，target listener 输出 `/tmp/cheetah_srt_relay_out.ts` 为 42KB，ffprobe 可识别 H264 与 AAC 48k。

---

## 弱网

启用弱网：

```bash
sudo tc qdisc add dev lo root netem loss 5% delay 50ms 10ms reorder 5%
```

恢复：

```bash
sudo tc qdisc del dev lo root
```

场景：

- `loss 1%`
- `loss 5%`
- `delay 50ms 10ms`
- `reorder 5%`
- 带宽限制到输入码率的 1.2x

观测：

- SRT reconnect 次数。
- bytes/packets in/out。
- RTSP/RTMP/HLS/WebRTC 播放连续性。
- TS demux diagnostic。

当前状态：

- 2026-06-15: 当前 shell 复测 `sudo -n true` 与 `sudo -n tc qdisc show dev lo` 均成功。
- 2026-06-15: 已执行 loopback netem 短互操作：`loss 1% delay 50ms 10ms reorder 1%`；FFmpeg lavfi H264/AAC 通过 SRT publish 到 Cheetah `live/netem`，弱网期间 HLS playlist 返回 200 并生成多个 segment，RTMP ffprobe 读出 H264 640x360 与 AAC 48k。
- 2026-06-15: 已执行 loopback netem 5% loss 短互操作：`loss 5% delay 50ms 10ms reorder 5%`；FFmpeg lavfi H264/AAC 通过 SRT publish 到 `live/netem5`，HLS playlist 返回 200，RTMP ffprobe 读出 H264 640x360 与 AAC 48k。
- 2026-06-15: 已执行 loopback rate limit 短互操作：`rate 1200kbit delay 20ms 5ms`；输入约 650kbit H264 + 96kbit AAC，通过 SRT publish 到 `live/netem_rate`，HLS playlist 返回 200，RTMP ffprobe 读出 H264 640x360 与 AAC 48k。
- 2026-06-15: 已执行更激进丢包观测：`loss 12% delay 80ms 30ms reorder 10%`；FFmpeg lavfi H264/AAC 通过 SRT publish 到 `live/tlpktdrop`，HLS playlist 返回 200，SRT metrics 显示 lost/duplicate/RTT/jitter 明显增长。
- 2026-06-15: 已验证慢 peer / egress backpressure：`modules.srt.egress.send_queue_capacity=0` 场景下 SRT request/play 被快速关闭，metrics 显示 `srt_send_queue_full_total=1151`、`srt_driver_errors_total=1151`。
- 2026-06-15: 每个 netem 场景测试结束后已删除 qdisc，最终 `sudo -n tc qdisc show dev lo` 恢复为 `qdisc noqueue`。
- 2026-06-15: `shiguredo_srt` 当前 stats 未暴露独立 TLPKTDROP 计数字段，现阶段通过 lost/duplicate/RTT/jitter 与 HLS/RTMP 连续性间接观测；24h 长稳仍待执行。
- 2026-06-15: 已执行 100 路短连接反复 publish 压测：FFmpeg lavfi 快速发起 100 次 SRT publish，`ok=100 fail=0`；metrics 显示 `connections_total=100`、`publish_connections_total=100`、`disconnect_total=100`、`connections_active=0`。

---

## 长稳

场景：

- 单路 FFmpeg publish 24 小时。
- 100 路短连接循环 publish。
- 20 路 relay job 自动重连。
- AES-128/AES-256 加密连接 24 小时。

成功标准：

- 无 panic。
- 无无界内存增长。
- 慢订阅者不拖累其他订阅者。
- job 断开后能按退避恢复。

---

## Metrics 与 Health

已实现 SRT module 本地指标端点：

- `GET /srt/metrics`
- `GET /srt/metrics.json`

当前指标：

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

health 关注：

- listener bind 状态。
- active connections。
- failed jobs。
- repeated reconnect jobs。
- encryption config validity。

label 约束：

- 默认不把 peer addr 放入 metrics label。
- stream label 需要可配置开启。

---

## 失败 Artifact

每次外部互操作失败时保存：

- Cheetah stdout/stderr。
- FFmpeg/ffplay/OBS/libsrt 命令和输出。
- 使用的配置文件。
- 失败前后 60 秒日志。
- `out.ts` 或失败输入片段。
- netem/tc 配置。

推荐目录：

```text
artifacts/srt-interop/<date>/<case-name>/
```
