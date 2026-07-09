# Phase 04 — 高级特性与性能优化

- **状态**: 未开始
- **范围**: 按需 muxing、RTCP 反馈驱动 IDR 请求、跨协议转换性能基准与优化
- **完成标准**: 无订阅者时 CPU 开销趋近于零；RTCP PLI 能触发发布者发送关键帧；跨协议转换延迟 < 5ms

---

## 4.1 按需 muxing（无订阅者不执行协议转换）

**问题**: 当 RTMP 推流但无 RTSP 订阅者时，系统仍然为每帧执行 RTP packetize 等转换操作，浪费 CPU。

**ZLMediaKit 方案**: `ProtocolOption` 中的 `rtsp_demand`/`rtmp_demand` 选项。当设置为 demand 模式时，对应协议的 muxer 仅在有订阅者时才激活。`MultiMediaSourceMuxer::onTrackFrame_l()` 检查 `_rtsp->isEnabled()` 后才调用 `inputFrame()`。

**本地现状**:
- Engine 的 Dispatcher 已实现"无订阅者时 push_frame 仍写入 ring buffer 但不分发"
- 但 RTSP/RTMP module 的 egress 管线是在订阅者创建时才启动的（lazy）
- 实际上本地已经是按需模式：只有 subscribe 时才创建 play task
- **但** ring buffer 的写入和 IDR 追踪仍在执行，且 bootstrap 帧的维护有开销

**需要优化的点**:

1. **Ring buffer 写入优化**：无任何订阅者时，可以降低 ring buffer 写入频率（仅保留最近 keyframe）
2. **Track 信息懒计算**：SDP 生成、AVCC 构建等仅在首次 DESCRIBE/PLAY 时执行
3. **参数集缓存懒更新**：无订阅者时不需要维护 egress 用的参数集缓存

**实现方案**:

```rust
// cheetah-engine — 按需 ring buffer 策略
pub enum RingBufferWritePolicy {
    /// 始终写入所有帧（默认，支持秒开）
    AlwaysWrite,
    /// 无订阅者时仅写入关键帧（节省内存和 CPU）
    KeyframeOnlyWhenIdle,
    /// 无订阅者时完全不写入（最省资源，但首次订阅需等待 keyframe）
    SuspendWhenIdle,
}

// StreamState — 订阅者计数驱动策略切换
impl StreamState {
    fn on_subscriber_count_changed(&mut self, count: usize) {
        match count {
            0 => self.ring_buffer.set_write_policy(self.config.idle_policy),
            _ => self.ring_buffer.set_write_policy(RingBufferWritePolicy::AlwaysWrite),
        }
    }
}
```

**配置**:
```yaml
global:
  stream:
    idle_ring_policy: keyframe_only  # always | keyframe_only | suspend
```

**实现位置**: `cheetah-engine` stream.rs，ring_buffer.rs

**验证**:
- 基准测试：100 路 RTMP 推流无订阅者，对比 always vs keyframe_only 的 CPU 使用率
- 功能测试：idle 模式下新订阅者加入后能正常秒开

---

## 4.2 RTCP PLI/FIR 驱动关键帧请求

**问题**: RTSP 订阅者加入时如果 GOP cache 中没有可用的关键帧（如 idle 模式），需要向发布者请求立即发送关键帧。RTCP PLI（Picture Loss Indication）和 FIR（Full Intra Request）是标准机制。

**ZLMediaKit 方案**: 订阅者发送 RTCP PLI/FIR → 服务器转发给发布者 → 发布者编码器响应发送 IDR。

**本地现状**:
- RTSP core 已实现 RTCP feedback 解析（`rtcp_fb.rs`：NACK、PLI、FIR）
- 但收到 PLI/FIR 后没有向发布者传递关键帧请求的机制
- RTMP 协议本身没有标准的关键帧请求机制（需要通过 AMF 命令或带外信令）

**实现方案**:

```rust
// cheetah-sdk — 关键帧请求 API
pub trait PublisherApi {
    /// 请求发布者发送关键帧（best-effort，发布者可能不响应）
    fn request_keyframe(&self, stream_key: &StreamKey) -> Result<(), KeyframeRequestError>;
}

// cheetah-engine — 关键帧请求路由
impl StreamState {
    fn handle_keyframe_request(&self) {
        // 通过 publisher 的 command channel 发送请求
        if let Some(publisher) = &self.active_publisher {
            let _ = publisher.command_tx.try_send(PublisherCommand::RequestKeyframe);
        }
    }
}

// RTSP module — 收到订阅者 RTCP PLI/FIR
fn handle_rtcp_pli(stream_key: &StreamKey, engine: &EngineContext) {
    engine.publisher_api.request_keyframe(stream_key);
}

// RTSP module — 发布者收到关键帧请求
fn handle_keyframe_request_for_rtsp_publisher(session: &mut PublishSession) {
    // 发送 RTCP FIR 给 RTSP 推流客户端
    let fir_packet = build_rtcp_fir(session.video_ssrc, session.fir_seq_nr);
    session.fir_seq_nr += 1;
    send_rtcp(session, fir_packet);
}

// RTMP module — 发布者收到关键帧请求（best-effort）
fn handle_keyframe_request_for_rtmp_publisher(session: &mut PublishSession) {
    // RTMP 没有标准关键帧请求机制
    // 方案 1：发送 AMF 命令 "requestKeyframe"（部分编码器支持）
    // 方案 2：记录请求，等待下一个自然 keyframe 时优先分发
    // 方案 3：不做任何事（依赖 GOP interval）
    tracing::debug!("keyframe requested for RTMP publisher, waiting for next natural IDR");
}
```

**实现位置**: `cheetah-sdk` publisher.rs，`cheetah-engine` stream.rs，`cheetah-rtsp-module` play.rs + publish.rs

**配置**:
```yaml
modules:
  rtsp:
    enable_keyframe_request: true  # 默认启用
    max_keyframe_request_rate_hz: 1  # 限制请求频率（防止恶意客户端）
```

**验证**:
- 集成测试：RTSP 推流 → RTSP 拉流发送 PLI → 验证发布者收到 FIR → 验证下一帧为 keyframe
- 集成测试：RTMP 推流 → RTSP 拉流发送 PLI → 验证系统不崩溃（graceful degradation）

---

## 4.3 跨协议转换性能基准与优化

**问题**: 跨协议转换引入额外延迟和 CPU 开销，需要量化并优化。

**ZLMediaKit 方案**: 
- `PacketCache` 批量写入减少 per-packet 开销
- `FramePacedSender` 平滑突发
- 按需 muxing 避免无用转换
- 直接代理模式跳过 demux/remux

**本地现状**:
- RTMP→RTMP 已有 side_data 零拷贝优化
- 但 RTMP→RTSP 和 RTSP→RTMP 路径未做性能基准测试
- 未知瓶颈在哪里

**基准测试计划**:

```rust
// benches/cross_protocol_bench.rs
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_rtmp_to_rtsp_packetize(c: &mut Criterion) {
    // 测量：AVFrame(H264 keyframe, 100KB) → RTP packets 的耗时
    c.bench_function("h264_keyframe_to_rtp", |b| {
        let frame = create_test_h264_keyframe(100_000);
        b.iter(|| packetize_frame_to_rtp(&frame, &mut track_state));
    });
}

fn bench_rtsp_to_rtmp_mux(c: &mut Criterion) {
    // 测量：AVFrame(H264, Annex-B) → RTMP FLV payload 的耗时
    c.bench_function("h264_frame_to_flv", |b| {
        let frame = create_test_h264_frame(10_000);
        b.iter(|| map_frame_to_rtmp_flv_payload(&frame, &tracks));
    });
}

fn bench_timestamp_conversion(c: &mut Criterion) {
    // 测量：TimestampNormalizer + EgressAdapterView 的耗时
    c.bench_function("timestamp_normalize_and_convert", |b| {
        let mut normalizer = TimestampNormalizer::new(config);
        b.iter(|| normalizer.normalize(input));
    });
}
```

**优化方向**:

1. **RTP packetize 零拷贝**：避免 payload 复制，使用 `Bytes::slice()` 分片
2. **批量 RTP 发送**：累积多个 RTP 包后一次 writev（减少 syscall）
3. **参数集缓存命中**：避免每帧重新解析 SPS/PPS
4. **时间戳计算内联**：热路径的时间戳转换使用 `#[inline]` 确保无函数调用开销
5. **RTMP FLV 构建零拷贝**：使用 `BytesMut` 预分配 + 直接写入，避免中间 Vec

```rust
// 优化示例：RTP packetize 零拷贝
fn packetize_h264_fu_a_zero_copy(nalu: &Bytes, mtu: usize) -> Vec<Bytes> {
    let header_size = 2; // FU indicator + FU header
    let max_payload = mtu - RTP_HEADER_SIZE - header_size;
    let mut packets = Vec::with_capacity(nalu.len() / max_payload + 1);
    
    let nalu_header = nalu[0];
    let nalu_payload = nalu.slice(1..); // 零拷贝 slice
    
    for (i, chunk) in nalu_payload.chunks(max_payload).enumerate() {
        let mut buf = BytesMut::with_capacity(header_size + chunk.len());
        buf.put_u8(/* FU indicator */);
        buf.put_u8(/* FU header with S/E bits */);
        // 直接引用原始 Bytes，不复制
        let packet_payload = nalu_payload.slice(i * max_payload..i * max_payload + chunk.len());
        packets.push(buf.freeze().chain(packet_payload));
    }
    packets
}
```

**性能目标**:
- 单帧跨协议转换延迟 < 1ms（不含网络 I/O）
- 1080p 30fps + AAC 48kHz 跨协议转换 CPU < 5%（单核）
- 100 路并发跨协议转换 CPU < 50%（8 核）

**实现位置**: `benches/` 目录，`cheetah-rtsp-module` media/packetize.rs，`cheetah-rtmp-module` egress.rs

**验证**:
- criterion 基准测试建立 baseline
- 优化后对比 baseline 确认改进
- 压力测试：100 路 RTMP 推流 + 100 路 RTSP 拉流，监控 CPU/内存/延迟
