# Phase 01 — 共享 RTP / PS / TS / ES 媒体能力

- **状态**: 已完成
- **范围**: 在 `cheetah-codec` 中补齐 RTP/PS/TS/ES/Ehome 的共享媒体内核，完善编码矩阵、多轨、时间戳和兼容处理
- **完成标准**: `cheetah-codec` 可独立完成 RTP payload、PS/TS/ES mux/demux 与多编码 roundtrip；`ts` 协议现有 RTP-TS 路径可复用共享能力

---

## 1.1 升级 PS mux/demux

**问题**: 当前 `ps.rs` 只有基础 PES/PS parse/encode 骨架，不足以承载 GB28181/RTP 主路径。

**实现方案**:

- 增加 `PsMuxer`、`PsDemuxer`、`PsMuxConfig`、`PsDemuxConfig`
- 支持 pack header、system header、program stream map、PES reassembly
- 支持 H264/H265/AAC/G711/MP3/Opus/VP8/VP9/AV1 的 PS 封装与解封装
- 输出统一 `TrackInfo` 与 `AVFrame`
- 兼容缺失 system header、乱序 start code、前导垃圾和重复参数集

**设计要求**:

- 时间戳统一来自 canonical `pts_us/dts_us`
- 参数集缓存、关键帧判定、音频 duration 推导回到 `cheetah-codec`
- 多轨道 PS 输入要能稳定映射到 audio/video track
- `max_reassembly_bytes`、`max_tracks`、`max_pes_payload_bytes` 必须 bounded

---

## 1.2 统一 RTP payload encode/decode

**目标**: 把独立 RTP 协议和 GB28181 模块都会用到的 payload encode/decode 收敛到共享层。

**目标 API**:

```rust
pub enum RtpPayloadMode {
    Ps,
    Ts,
    Es,
    Ehome,
    RawAudio,
    RawVideo,
}

pub enum RtpMediaEvent {
    TrackInfo(Vec<TrackInfo>),
    Frame(AVFrame),
    Packet(Bytes),
    Diagnostic(RtpMediaDiagnostic),
}
```

**实现要求**:

- `Ps`：RTP -> PS -> `AVFrame`
- `Ts`：RTP -> TS -> `AVFrame`
- `Es`：RTP -> codec-specific depacketizer -> `AVFrame`
- `Ehome`：先做 payload probe 和 framing 兼容，最终落到 PS/ES 路径
- raw 模式允许 API 显式提供 codec、sample rate、channel、bit depth
- RTP marker、timestamp、SSRC、sequence 信息保留为 side data 或 diagnostic

---

## 1.3 扩展 ES 编码矩阵

**现状**: RTSP module 已有较完整 packetize/depacketize，但能力还没抽为共享 API。

**落地要求**:

- 抽取或复用 H264/H265/AAC/G711/Opus/MP3/VP8/VP9/AV1 的 RTP encode/decode
- 保持 `AVFrame + TrackInfo` 为唯一对外媒体模型
- ES 路径不要求所有编码都有标准 SDP 生成，但必须可稳定接收/发送与诊断
- 对厂商脏数据做 bounded 容错，不允许 panic 或无界 buffer 增长

---

## 1.4 改造现有 `rtp_ts` Sans-I/O 能力

**目标**: 让 `crates/protocols/ts/core/src/rtp_ts.rs` 不再只支持 TS，转而复用通用共享媒体层。

**改动点**:

- `PayloadProbe` 从 `Ts/Ps/Unknown` 扩为 `Ts/Ps/Es/Ehome/Unknown`
- PS 不再只发 `UnsupportedPsPayload` 诊断，而是进入 `PsDemuxer`
- ES 模式支持从 REST/API 或 SDP 注入 codec hint
- 现有 TS path 行为保持不退化
- source address 绑定、padding/extension、unaligned vendor prefix 兼容继续保留

---

## 1.5 多轨与时间戳统一

**目标**: 避免 RTP、PS、TS、GB28181 各自维护私有时间戳和 track 模型。

**规则**:

- 所有入口统一产出 `TrackInfo` 和 `AVFrame`
- track kind、codec、timescale、channel、sample rate 在首次识别后稳定保存
- DTS/PTS 展开、回绕处理、断流标记、参数集补发统一回到 `cheetah-codec`
- G711/MP3/Opus duration 推导不放在协议 module
- 多轨道模式默认支持多个 video/audio track，超过 `max_tracks` 的 track 跳过并输出 diagnostic

---

## 1.6 测试与 Fuzz

单元测试：

- RTP header parse/encode roundtrip
- TCP 2-byte frame parse/reassembly
- PS mux/demux roundtrip
- TS mux/demux roundtrip 不回退
- H264/H265/AAC/G711/Opus/MP3/VP8/VP9/AV1 ES packetize/depacketize roundtrip
- 多轨音视频 PS/TS 到 `TrackInfo + AVFrame` 正确
- 时间戳回绕、乱序、重复包、source address 切换 bounded

Property tests：

- 任意 chunk 切分下 PS/TS demux 结果一致
- RTP reorder 对 sequence wrap 结果稳定
- mux 后 demux 保持 codec、track kind、keyframe flag、frame count

Fuzz targets：

- `fuzz_rtp_header`
- `fuzz_rtp_tcp_frame`
- `fuzz_ps_demux`
- `fuzz_rtp_ps_pipeline`
- `fuzz_rtp_es_pipeline`

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo clippy -p cheetah-ts-core
cargo test -p cheetah-ts-core
```
