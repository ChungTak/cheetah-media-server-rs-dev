# Phase 01 — Codec TS 兼容增强

- **状态**: 计划中
- **范围**: `crates/foundation/cheetah-codec` 中 MPEG-TS mux/demux、非标准 stream_type、时间戳、帧率估计、G711/AAC 容错和诊断。
- **完成标准**: `cheetah-codec` 能独立覆盖 ABL/libmpeg 常见 TS 输入输出，所有协议 module 不再复制 TS 时间戳和 codec 修补逻辑。

---

## 1.1 stream_type 与 descriptor 复核

**现状**: `ts_common.rs` 已覆盖 H264/H265/H266/AAC/MP2/MP3/G711A/G711U/OPUS/VP8/VP9/AV1。

**补充要求**:

1. OPUS 输入同时兼容 `0x06 + "Opus"` descriptor 和 ABL/libmpeg 风格 `0x9C`。
2. VP8/VP9/AV1 输入同时兼容私有 stream_type 和 registration descriptor。
3. 未知 `0x06` private stream 不 panic，输出 `UnknownStreamType` diagnostic。
4. 为每个目标编码建立 mux -> demux roundtrip case。

**测试**:

```bash
cargo test -p cheetah-codec ts_codec_matrix
```

---

## 1.2 AAC ADTS 与连续音频帧

**ABL 参考**: `RtpTSStreamInput.cpp::on_ts_packet()` 使用 `mpeg4_aac_adts_frame_length()` 在同一 PES payload 中循环切多个 ADTS frame。

**补充要求**:

1. demux AAC 时支持一个 PES 中连续多个 ADTS frame。
2. ADTS length 小于 header、超过 payload 或 sample rate index 非法时输出 diagnostic。
3. `TrackInfo.extradata` 从首个合法 ADTS 推导 ASC。
4. mux raw AAC 时继续封装 ADTS，mux 已带 ADTS 的 payload 时避免双封装。

---

## 1.3 G711 时间戳与 duration

**ABL 参考**: `NetRecvBase.cpp::CalcG711TimeStampByLength()` 按 G711 payload length 计算音频时间。

**补充要求**:

1. 为 G711A/G711U 增加 duration 推导 helper，默认 sample rate 8000Hz，单字节一个 sample。
2. demux G711 时若 PES 无明确 duration，按 payload length 推导 `duration_us` 或 frame metadata。
3. mux G711 时使用 `AVFrame` 的 pts/dts，不在 module 中按帧数临时推进。
4. audio-only G711 可作为 PCR PID。

---

## 1.4 真实帧率估计

**ABL 参考**:

- `CalcMaxVideoFrameSpeed = 250`
- `Calc1078VideoFrameSpeedCount = 500`
- 启动前 15 帧不稳定间隔不参与最终值
- 帧率上限裁剪到 120

**补充要求**:

1. 在 codec 或 foundation 层新增 `FrameRateEstimator`，输入为 timestamp 和 timebase。
2. 支持 warmup frame、窗口平均、最小/最大 fps clamp。
3. RTP timestamp 和 TS PTS 都可复用同一 estimator。
4. module 只消费 estimator 结果更新 stream metadata，不复制算法。

---

## 1.5 demux 容错增强

**补充要求**:

1. `push()` 保持任意切片输入能力。
2. 增加坏 sync byte、非 188 对齐、前导垃圾、半包、粘包样例。
3. continuity gap 后保留诊断，下一 PUSI 强制重新同步 PES。
4. PAT/PMT CRC 在 loose 模式继续解析，在 strict 模式拒绝该 section。
5. 所有 per-PID reassembly buffer 受 `max_reassembly_bytes` 限制。

---

## 验证命令

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec ts_
```
