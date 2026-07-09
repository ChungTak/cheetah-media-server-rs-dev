# Phase 03 — SRT 摄入与跨协议转换

- **状态**: 已完成
- **范围**: SRT MPEG-TS 摄入、demux 为 `AVFrame + TrackInfo`、发布到引擎、验证 SRT 到 RTSP/RTMP/HLS/WebRTC 播放
- **完成标准**: FFmpeg/OBS 通过 SRT 推流到 Cheetah 后，可用 RTSP、RTMP、HLS、WebRTC 播放同一 `StreamKey`

---

## 3.1 Module 生命周期

`SrtModuleFactory`：

```rust
impl ModuleFactory for SrtModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new("srt"),
            display_name: "SRT Module".to_string(),
            dependencies: vec![],
            config_namespace: "srt".to_string(),
            routes_prefix: "/srt".to_string(),
            capabilities: vec![ModuleCapability::Publish, ModuleCapability::Subscribe],
        }
    }
}
```

`start()` 行为：

1. 如果 `config.enabled=false`，直接进入 `Running`，不启动 driver。
2. 启动 SRT listener driver。
3. 为每个 enabled ingress job 启动 caller job。
4. 启动 driver event worker。
5. 保存 cancellation token 和 task handles。

`stop()` 行为：

- 取消所有 job。
- 关闭 driver handle。
- 释放所有 publish lease。
- 上报 module stopped。

---

## 3.2 Ingress Session

每个 SRT publish 连接对应一个 ingress session：

```rust
struct SrtIngressSession {
    peer_id: SrtPeerId,
    stream_key: StreamKey,
    lease: PublishLease,
    publisher: Box<dyn PublisherSink>,
    demuxer: MpegTsDemuxer,
    tracks: Vec<TrackInfo>,
    started_at_micros: u64,
    last_payload_at_micros: u64,
}
```

创建流程：

1. driver 发 `Connected { stream_id }`。
2. module 解析 stream id，得到 `mode=publish` 和 `StreamKey`。
3. 鉴权通过后调用 `publisher_api.acquire_publisher`。
4. 创建 `MpegTsDemuxer`。
5. 放入 `peer_id -> SrtIngressSession` map。

拒绝流程：

- Stream ID 无效：driver close。
- 鉴权失败：driver close。
- 发布租约冲突：driver close，并返回 Conflict 事件。
- payload kind 不是 MPEG-TS：driver close。

---

## 3.3 MPEG-TS Demux

使用：

```rust
use cheetah_codec::{MpegTsDemuxEvent, MpegTsDemuxer, MpegTsDemuxerConfig};
```

处理 driver payload：

```rust
for event in session.demuxer.push(&payload) {
    match event {
        MpegTsDemuxEvent::TrackFound(track) => {
            update_tracks_if_changed(session, track)?;
        }
        MpegTsDemuxEvent::Frame(frame) => {
            publish_frame(session, frame)?;
        }
        MpegTsDemuxEvent::Diagnostic(diag) => {
            record_demux_diagnostic(peer_id, diag);
        }
    }
}
```

要求：

- 不使用 HLS module 私有 `TsDemuxer`。
- 如果 `cheetah-codec::MpegTsDemuxer` 缺少 SRT 场景所需能力，优先补 `cheetah-codec`。
- `TrackInfo` 更新必须去重，避免每包重复 `update_tracks`。
- demux 输出的 `AVFrame` 必须满足 canonical timeline 语义。

---

## 3.4 时间戳和断流

时间戳策略：

- MPEG-TS PTS/DTS 是 source timeline。
- demux 后 frame 进入 `cheetah-codec` 归一化路径。
- `AVFrame.pts/dts/pts_us/dts_us` 代表 canonical timeline。
- SRT 重连或 relay source switch 时首帧标记 `FrameFlags::DISCONTINUITY`。

异常处理：

- PTS/DTS 回绕：由 `cheetah-codec` unwrap/normalize。
- PES 缺失 DTS：由 `cheetah-codec` DTS generator 补齐。
- 长时间无 payload：idle timeout 关闭 session。
- TS sync loss：计数上报，不直接 panic。

---

## 3.5 跨协议播放

SRT 摄入发布到引擎后，其他协议按现有能力订阅：

| 输出协议 | 验证路径 | 依赖 |
|----------|----------|------|
| RTSP | `rtsp://host/live/test` | `cheetah-rtsp-module` |
| RTMP | `rtmp://host/live/test` | `cheetah-rtmp-module` |
| HLS | `http://host:8088/live/test/index.m3u8` | `cheetah-hls-module` |
| WebRTC | WHIP/WHEP 或现有 WebRTC play API | `cheetah-webrtc-module` |

SRT module 不需要直接调用这些 module，也不维护协议间 bridge 表。成功条件是 `StreamManagerApi::get_stream(live/test)` 显示 publisher active 且 tracks 正确。

当前外部验证记录：

- 2026-06-12: 启动 `cheetah-server --no-default-features --features "srt rtmp hls"`，默认 SRT listener `0.0.0.0:9000`。
- 2026-06-12: 使用 FFmpeg 将 `test_media_files/camera_h265.mp4` 以 MPEG-TS over SRT 推到 `srt://127.0.0.1:9000?mode=caller&streamid=%23!::r=live/srt_ffmpeg,m=publish`，server 日志出现 `SRT peer connected`，Stream ID 正确解析为 `#!::r=live/srt_ffmpeg,m=publish`。
- 2026-06-12: 推流期间使用 FFprobe 访问 `rtmp://127.0.0.1/live/srt_ffmpeg`，可读取到 FLV/HEVC stream，证明 SRT 摄入已发布到引擎并可被 RTMP module 订阅。
- 2026-06-12: 使用 FFmpeg lavfi 生成标准 H264/AAC 并推到 `srt://127.0.0.1:9000?mode=caller&streamid=%23!::r=live/srt_h264,m=publish`，server 日志出现 `SRT peer connected`，推流期间 FFprobe 访问 `rtmp://127.0.0.1/live/srt_h264` 可读取到 FLV/H264 stream。
- 2026-06-12: 修复 `cheetah-codec::MpegTsDemuxer` 在 PES 中发现 H26x SPS/PPS 和 AAC ADTS ASC 后不更新 `TrackInfo` 的问题；SRT module 现在会合并同一 track id 的 ready track 更新，避免跨协议输出看到旧的 NotReady 元数据。
- 2026-06-12: 使用 FFmpeg lavfi 生成标准 H264/AAC 并推到 `live/srt_hls` 后，`ffprobe rtmp://127.0.0.1/live/srt_hls` 可读出 H264 640x360 和 AAC 48k；`curl --noproxy '*' http://127.0.0.1:8891/api/v1/streams` 显示 H264/AAC track 均为 `Ready`；`curl --noproxy '*' http://127.0.0.1:8088/live/srt_hls/index.m3u8` 返回 200 并生成 `seg_0.ts`/`seg_1.ts`；`seg_0.ts` 返回 200 `video/mp2t`。
- 2026-06-12: 之前记录的 control/module HTTP `502 Bad Gateway` 是本机代理环境导致：`http_proxy/all_proxy` 生效且 `no_proxy` 缺少 `127.0.0.1`；使用 `curl --noproxy '*'` 访问 control、SRT metrics 和 HLS 后正常。
- 2026-06-12: 使用临时 `CHEETAH_CONFIG` 将 RTSP listen 改为 `0.0.0.0:8554` 后，启动 `cheetah-server --no-default-features --features "srt rtmp hls rtsp"` 成功；FFmpeg lavfi H264/AAC SRT 推流到 `live/srt_rtsp` 后，control streams 显示 H264/AAC track 均为 `Ready`，`ffprobe -rtsp_transport tcp rtsp://127.0.0.1:8554/live/srt_rtsp` 可读出 H264 640x360 与 AAC 48k。
- 2026-06-15: 启动 `cheetah-server --no-default-features --features "srt rtmp hls webrtc"`，使用临时 `CHEETAH_CONFIG` 将 WebRTC UDP listen 配为 `127.0.0.1:0`；FFmpeg lavfi H264/AAC SRT 推流到 `live/srt_webrtc` 后，control streams 显示 H264/AAC track 均为 `Ready`。
- 2026-06-15: 对 `http://127.0.0.1:8891/api/v1/rtc/whep?app=live&stream=srt_webrtc` 使用 `crates/protocols/webrtc/module/tests/fixtures/minimal_offer.sdp` 发送 WHEP POST，返回 `201 Created`、`content-type: application/sdp`、`location: /api/v1/rtc/session/webrtc-session-1`，并生成 SDP answer；WebRTC metrics endpoint 可访问。该结果验证 SRT 摄入流可走到 WHEP answer 生成，不等同于真实浏览器 ICE/DTLS/SRTP 媒体完成。
- 2026-06-15: 新增 `whep-browser-smoke.html` 作为浏览器 WHEP smoke；Chrome headless 对 SRT 摄入流发起真实 WHEP play，页面收到 `whep_status=201`、`track=video`、`track=audio`，但诊断显示 `local_candidates=16`、`remote_candidates=0`、`ice=new`，最终 `RESULT: FAIL ice timeout`。固定 `listen_udp=127.0.0.1:7000` 并配置 `public_ips=[127.0.0.1]` 后结果相同。阻塞点是 WebRTC module answer 未输出 server candidates，不是 SRT ingress 或 engine stream readiness。
- 2026-06-16: WebRTC driver 已修复 answer local candidate 注入与 `a=end-of-candidates` 输出；WebRTC module 默认 `audio_output_strategy=auto` 在 AAC 无 Opus 转码能力时改为丢弃不可输出音频并保持视频播放。Chrome headless 对 SRT H264/AAC 摄入流复测输出 `ice=connected`、`pc=connected`、`inbound_packets=84`、`RESULT: PASS`；手动 `pc.getStats()` 确认 video inbound RTP 5095 包、949 帧已解码。
- 2026-06-16: 为“更多音视频编码格式适配”扩展 `cheetah-codec` MPEG-TS 主路径：H266/VVC egress 关键帧注入 AUD + VPS/SPS/PPS，MJPEG/ADPCM 增加私有 registration descriptor 识别，MPEG audio 从 PES 首帧头细分 MP2/MP3 并推导 sample rate/channel；`ts_codec_matrix` 覆盖 H264/H265/H266/AV1/VP8/VP9/MJPEG 与 AAC/G711A/G711U/Opus/MP3/MP2/ADPCM roundtrip。
- 2026-06-16: 外部复测 FFmpeg H264/MP3 MPEG-TS over SRT publish 到 `live/srt_h264_mp3`，control streams 显示 H264 Ready 与 MP3 Ready（44100 Hz、2 channels），`ffprobe rtmp://127.0.0.1/live/srt_h264_mp3` 可读出 H264 320x180 与 MP3 44100 stereo。
- 2026-06-16: 复测 FFmpeg 真实 VP8/VP9/AV1 MPEG-TS 输出，确认 FFmpeg 使用 `stream_type=0x06` 且 PMT ES descriptor 为空，ffprobe 会将其识别为 `bin_data`；`cheetah-codec::MpegTsDemuxer` 已改为仅对“无 descriptor 的 0x06 私有视频流”进行首个 PES 延迟探测，未知 descriptor 仍保留 `UnknownStreamType` 诊断。
- 2026-06-16: SRT ingress 对 FFmpeg H265、VP8、VP9、AV1 视频流完成真实验证：启动 `cheetah-server --no-default-features --features "srt rtmp hls webrtc"` 后，分别用 `libx265`、`libvpx`、`libvpx-vp9 -deadline realtime -lag-in-frames 0`、`libaom-av1 -cpu-used 8 -lag-in-frames 0` 推 MPEG-TS over SRT 到 `live/srt_{codec}_codec_retry`；`curl --noproxy '*' http://127.0.0.1:8891/api/v1/streams` 显示 H265、VP8、VP9、AV1 track 均为 `Ready`。AV1 路径会从首个 OBU payload 提取 sequence header 写入 `CodecExtradata::AV1`，避免只识别 codec 但 track 停留在 `PendingConfig`。

当前阻塞：

- RTSP 默认配置仍是特权端口，非 root 环境需显式配置非特权 listen，例如 `0.0.0.0:8554`。
- WebRTC 浏览器视频主路径已验证；AAC/MP3 等非 Opus 音频仍需要后续转码能力才能在浏览器侧播放。

---

## 3.6 Ingress Jobs

配置：

```rust
pub struct SrtIngressJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_stream_key: String,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}
```

用于 Cheetah 主动 caller 拉远端 SRT：

```yaml
modules:
  srt:
    ingress_jobs:
      - name: camera-a
        enabled: true
        source_url: "srt://camera.example.com:9000?mode=caller&streamid=#!::r=live/camera-a,m=request"
        target_stream_key: "live/camera-a"
```

任务行为：

1. 解析 `source_url`。
2. driver `ConnectCaller`。
3. 连接成功后按 payload demux。
4. 断开后按指数退避重连。
5. module stop 时停止重试并释放租约。

---

## 3.7 鉴权

v1 支持：

- 全局 publish token。
- Stream ID 中 `u` 作为用户名。
- URL query token。
- passphrase 只用于 SRT 加密，不等同业务鉴权。

建议配置：

```rust
pub struct SrtAuthConfig {
    pub enabled: bool,
    pub publish_token: String,
    pub request_token: String,
    pub users: Vec<SrtAuthUserConfig>,
}
```

规则：

- `mode=publish` 校验 publish 权限。
- `mode=request|play` 校验 request 权限。
- auth disabled 时仍执行 StreamKey 安全校验。

当前实现：

- Stream ID 中使用 `token` 字段传递业务 token，例如 `#!::r=live/test,m=publish,token=secret`。
- Caller job URL 中的 `token` query 会合并到最终 Stream ID；如果 Stream ID 已自带 `token`，以 Stream ID 为准。
- `publish_token` 匹配 `mode=publish`，`request_token` 匹配 `mode=request|play`。
- Stream ID 中 `u` 字段作为用户名，`users[].token` 可授予该用户 publish/request 权限。
- SRT `passphrase` 仅用于链路加密，不作为业务鉴权 token。

---

## 验证方法

启动：

```bash
cargo run -p cheetah-server --features "srt rtsp rtmp hls webrtc"
```

推流：

```bash
ffmpeg -re -stream_loop -1 -i sample.mp4 -c copy -f mpegts \
  "srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/test,m=publish"
```

播放：

```bash
ffplay rtsp://127.0.0.1:554/live/test
ffplay rtmp://127.0.0.1/live/test
ffplay http://127.0.0.1:8088/live/test/index.m3u8
```

Rust 验证：

```bash
cargo fmt
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-module
```
