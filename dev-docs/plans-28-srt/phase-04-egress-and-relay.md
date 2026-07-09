# Phase 04 — SRT 输出与 Relay

- **状态**: 部分完成
- **范围**: 本地流通过 SRT 输出、SRT caller push、listener request/play、SRT relay jobs、重试和断开语义
- **完成标准**: Cheetah 中任意本地流可被 SRT peer 拉取或主动推送到远端 SRT listener；远端 SRT source 可经 Cheetah relay 到另一个 SRT target

---

## 4.1 Egress Session

每个 SRT request/play 连接对应一个 egress session：

```rust
struct SrtEgressSession {
    peer_id: SrtPeerId,
    stream_key: StreamKey,
    subscriber: Box<dyn SubscriberSource>,
    muxer: MpegTsMuxer,
    tracks: Vec<TrackInfo>,
    started_at_micros: u64,
    bytes_sent: u64,
    frames_sent: u64,
}
```

创建流程：

1. driver 发 `Connected { stream_id }`。
2. module 解析 Stream ID，得到 `mode=request|play` 和 `StreamKey`。
3. 鉴权通过。
4. 调用 `subscriber_api.subscribe`。
5. 从 stream snapshot 获取 tracks，初始化 `MpegTsMuxer`。
6. 启动 subscriber worker，将 frame mux 成 TS payload 后发给 driver。

---

## 4.2 MPEG-TS Mux

使用：

```rust
use cheetah_codec::{MpegTsMuxEvent, MpegTsMuxer, MpegTsMuxerConfig};
```

处理 frame：

```rust
for event in muxer.push_frame(&frame) {
    match event {
        MpegTsMuxEvent::Packet(packet) => {
            driver.send(SrtDriverCommand::SendPayload {
                peer_id,
                payload: packet,
            });
        }
        MpegTsMuxEvent::Diagnostic(diag) => {
            record_mux_diagnostic(peer_id, diag);
        }
    }
}
```

要求：

- SRT egress 固定使用 `cheetah-codec::MpegTsMuxer` 生成 MPEG-TS，不新增私有 TS muxer。
- 不在 SRT module 中复制 H264/H265 NALU、AAC ADTS、参数集缓存逻辑。
- muxer 初始化前 tracks 必须完整；如果 extradata 尚未就绪，按 `track_ready_timeout_ms` 等待。
- 新订阅者默认从关键帧开始；超时后返回 play wait timeout。

---

## 4.3 Egress Jobs

用于 Cheetah 主动把本地流推到远端 SRT listener。

配置：

```rust
pub struct SrtEgressJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_stream_key: String,
    pub target_url: String,
    pub disable_video: bool,
    pub disable_audio: bool,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}
```

示例：

```yaml
modules:
  srt:
    egress_jobs:
      - name: uplink-a
        enabled: true
        source_stream_key: "live/test"
        target_url: "srt://remote.example.com:9000?mode=caller&streamid=#!::r=live/test,m=publish"
```

任务行为：

1. 等待本地 `source_stream_key` 可订阅。
2. driver caller 连接远端 SRT listener。
3. 初始化 MPEG-TS muxer。
4. 持续订阅并发送 payload。
5. 源流结束或连接断开后按退避重试。

---

## 4.4 Listener Request / Play

当 Cheetah 作为 SRT listener 时，远端 caller 可以通过 `m=request` 或 `m=play` 拉本地流：

```text
srt://cheetah.example.com:9000?mode=caller&streamid=#!::r=live/test,m=request
```

语义：

- `request` 和 `play` 在 v1 等价，均表示从 Cheetah 拉流。
- 如果 stream 不存在，按 `play_wait_source_timeout_ms` 等待。
- 超时仍不存在则关闭连接并上报 not found。
- 如果 stream 存在但 tracks 未就绪，按 `track_ready_timeout_ms` 等待。

---

## 4.5 Relay Jobs

配置：

```rust
pub struct SrtRelayJobConfig {
    pub name: String,
    pub enabled: bool,
    pub source_url: String,
    pub target_url: String,
    pub stream_key: String,
    pub retry_backoff_ms: u64,
    pub max_retry_backoff_ms: u64,
}
```

示例：

```yaml
modules:
  srt:
    relay_jobs:
      - name: remote-a-to-remote-b
        enabled: true
        source_url: "srt://source.example.com:9000?mode=caller&streamid=#!::r=live/in,m=request"
        target_url: "srt://target.example.com:9000?mode=caller&streamid=#!::r=live/out,m=publish"
        stream_key: "relay/source-a"
```

Relay 分两段实现：

1. ingress job 将 `source_url` 发布到本地 `stream_key`。
2. egress job 将本地 `stream_key` 推到 `target_url`。

优点：

- 统一走引擎媒体模型。
- RTSP/RTMP/HLS/WebRTC 可以同时订阅 relay 中间流。
- 单发布者租约、背压、metrics 和 health 复用现有系统。

---

## 4.6 Backpressure

Egress 默认策略：

- subscriber queue: `256`
- backpressure: `DropUntilNextKeyframe`
- SRT send queue 满：丢弃到下一关键帧；连续超过阈值则断开。

配置：

```rust
pub struct SrtEgressConfig {
    pub subscriber_queue_capacity: usize,
    pub subscriber_backpressure: BackpressurePolicy,
    pub bootstrap_max_frames: usize,
    pub start_from_keyframe: bool,
    pub play_wait_source_timeout_ms: u64,
    pub track_ready_timeout_ms: u64,
    pub send_queue_capacity: usize,
    pub disconnect_on_send_queue_overflow: bool,
}
```

慢订阅者原则：

- 单个 SRT egress peer 慢，不影响同一 stream 的 RTSP/RTMP/HLS/WebRTC 订阅者。
- 单个 SRT peer 的 send queue 有界。
- Relay target 慢，不影响 relay source ingest 继续发布到引擎；只影响对应 egress job。

---

## 4.7 断开和重试

断开原因：

- remote closed
- idle timeout
- connect timeout
- send queue overflow
- auth rejected
- stream not found
- track ready timeout
- encryption mismatch
- driver error

重试策略：

- job 类连接采用指数退避。
- 手动 listener request/play 连接不自动重试，由客户端重连。
- relay source 和 target 分别重试；source 断开时释放 publish lease，target 断开时保留 ingress stream。

当前实现：

- `ingress_jobs`、`egress_jobs` 和 `relay_jobs` 展开的 caller 连接都会保存重连模板。
- `Disconnected` 或带 `peer_id` 的 driver `Error` 会按 `retry_backoff_ms` 指数退避，并受 `max_retry_backoff_ms` 封顶。
- `Connected` 后重置 retry attempt。
- 手动 listener request/play 连接不在 job 表中，不自动重试。
- 已有单元测试覆盖 relay job retry metadata 和退避封顶计算；外部断线长稳验证仍在 Phase 05。

当前执行记录：

- 2026-06-12: 启动 `cheetah-server --no-default-features --features "srt rtmp hls"`，使用 FFmpeg lavfi 通过 RTMP 发布 H264/AAC 到 `live/rtmp_to_srt`。
- 2026-06-12: 发布期间 control streams 显示 `live/rtmp_to_srt` publisher active，H264/AAC track 均为 `Ready`。
- 2026-06-12: 使用 `ffprobe "srt://127.0.0.1:9000?mode=caller&streamid=%23!::r=live/rtmp_to_srt,m=request"` 从 Cheetah SRT listener 拉流，成功读取 MPEG-TS 中的 H264 与 AAC 48k；SRT metrics 显示 `srt_play_connections_total=1`、`srt_bytes_out_total` 增长。
- 2026-06-12: 该 egress 验证中 ffprobe 对 H264 width/height 显示为 0，原因是 RTMP 源当前 track metadata 未携带宽高；H264/AAC 编码流本身已可通过 SRT request/play 输出。
- 2026-06-15: 使用 `srt-live-transmit` 作为远端 SRT listener，配置 Cheetah `egress_jobs` 将 RTMP 发布的 `live/egress_job` 主动推到 `127.0.0.1:9100`；listener 输出 `/tmp/cheetah_srt_egress_job.ts` 为 250KB，ffprobe 可识别 H264 与 AAC 48k。该路径验证本地流 SRT caller push 到远端 listener 可用。ffprobe 对 H264 width/height 显示为 0，原因同 RTMP 源 track metadata 未携带宽高。
- 2026-06-15: relay job 双端本机互操作初测发现 target listener 输出 0 字节；根因是 relay egress 在中间流刚创建但 tracks 尚未 Ready 时初始化 `MpegTsMuxer`，导致后续 Ready track metadata 未进入 muxer。
- 2026-06-15: 修复 `run_play_session` 等待非空且全部 Ready 的 tracks 后再初始化 muxer，并新增 `egress_wait_requires_non_empty_ready_tracks` 回归测试。
- 2026-06-15: 复测 source listener -> Cheetah relay -> target listener：中间流 `relay/local` 可见且 H264/AAC track 均为 `Ready`，target listener 输出 `/tmp/cheetah_srt_relay_out.ts` 为 42KB，ffprobe 可识别 H264 与 AAC 48k。该路径验证 relay 基础数据面可用；断线长稳和弱网下 relay 仍待 Phase 05 外部验证。
- 2026-06-16: `cheetah-codec::MpegTsMuxer` 已扩展 H266/VVC 关键帧 AUD + VPS/SPS/PPS 补发、MJPEG/ADPCM PMT 私有 descriptor、MP2/MP3/G711/Opus 等音频 passthrough；SRT module egress Ready gating 增加扩展 codec 回归测试。该能力不包含音视频转码，下游协议仍按自身 codec 能力播放或降级。

---

## 验证方法

本地流推远端 SRT listener：

```bash
ffplay "srt://127.0.0.1:9100?mode=listener"
```

Cheetah 配置：

```yaml
modules:
  srt:
    egress_jobs:
      - name: push-test
        enabled: true
        source_stream_key: "live/test"
        target_url: "srt://127.0.0.1:9100?mode=caller&streamid=#!::r=live/test,m=publish"
```

远端 caller 从 Cheetah 拉流：

```bash
ffplay "srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/test,m=request"
```

Rust 验证：

```bash
cargo fmt
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-module
```
