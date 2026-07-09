# Phase 01 — 共享 RTP / PS / TS / ES / Ehome 媒体能力

- **状态**: 已完成
- **范围**: 在 `cheetah-codec` 中补齐 RTP/PS/TS/ES/Ehome 的共享媒体内核，完善编码矩阵、多轨、时间戳、G711 打包和兼容处理
- **完成标准**: `cheetah-codec` 可独立完成 RTP payload、PS/TS/ES mux/demux 与多编码 roundtrip，并给后续 `rtp`/`gb28181` 协议层提供稳定接口

---

## 1.1 升级 PS mux/demux

**目标**:

- 把 `ps.rs` 从骨架升级为可用生产级
- 支持 pack header、system header、program stream map、PES reassembly
- 输出统一 `TrackInfo + AVFrame`

**要求**:

- 支持 H264/H265/AAC/G711/OPUS/MP3/VP8/VP9/AV1
- 兼容前导垃圾、乱序 start code、缺失 system header、重复参数集
- `max_reassembly_bytes`、`max_tracks`、`max_pes_payload_bytes` 必须 bounded

---

## 1.2 统一 RTP payload encode/decode

**目标**:

- 收敛 PS/TS/ES/Ehome payload 到共享 API
- 为后续 `cheetah-rtp-core` 与 `cheetah-gb28181-module` 提供统一媒体入口

**要求**:

- `Ps`: RTP -> PS -> `AVFrame`
- `Ts`: RTP -> TS -> `AVFrame`
- `Es`: RTP -> codec-specific depacketizer -> `AVFrame`
- `Ehome`: 兼容私有头后落到 PS/ES 路径
- raw 模式允许显式注入 codec、sample rate、channel 数、bit depth

---

## 1.3 扩展 ES 编码矩阵

**要求**:

- 抽取或复用 RTSP 中已有的 H264/H265/AAC/G711/Opus/MP3/VP8/VP9/AV1 RTP encode/decode
- 对外只暴露 `AVFrame + TrackInfo`
- unsupported codec 不 panic，输出 diagnostic
- 允许厂商脏数据在 bounded 范围内容错

---

## 1.4 Ehome 与 TCP framing 共享能力

**目标**:

- 把 Ehome probe、2-byte RTP over TCP、4-byte interleaved fallback 做成共享 helper 或 core 前置能力

**要求**:

- 识别 Ehome 私有头
- 剥离私有头后兼容 2-byte/4-byte RTP framing
- 遇到半包、粘包、异常包长时输出结构化诊断
- 支持后续按 SSRC 或 PS system header 搜索恢复

---

## 1.5 时间戳与打包策略统一

**规则**:

- 所有入口统一产出 canonical timeline
- 处理 RTP timestamp 正常增长、回绕、乱序和异常跳变
- G711 RTP 打包支持可配置 packet duration，默认 100ms
- 多轨道模式默认支持多个 audio/video track，超限 track 跳过并诊断

---

## 1.6 测试与 Fuzz

单元测试：

- `[x]` RTP header parse/encode roundtrip
- `[x]` PS mux/demux roundtrip
- `[x]` TS mux/demux 不回退
- `[x]` ES packetize/depacketize roundtrip
- `[x]` Ehome 头识别与剥离
- `[x]` timestamp wrap / disorder / abnormal jump bounded
- `[x]` G711 packet duration 打包行为

Property tests (已完成)：

- `[x]` 任意 chunk 切分下 PS/TS demux 结果一致
- `[x]` reorder 对 sequence wrap 结果稳定
- `[x]` mux 后 demux 保持 codec、track kind、frame count

Fuzz targets (已完成)：

- `[x]` `fuzz_rtp_header`
- `[x]` `fuzz_rtp_tcp_frame`
- `[x]` `fuzz_ehome_probe`
- `[x]` `fuzz_ps_demux`
- `[x]` `fuzz_rtp_es_pipeline`

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo clippy -p cheetah-ts-core
cargo test -p cheetah-ts-core
```
