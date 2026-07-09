# Phase 01 — 共享 MPEG-TS 容器能力完善

- **状态**: 未开始
- **范围**: `cheetah-codec` 中 TS mux/demux、codec matrix、PAT/PMT/PES/PCR、时间戳、ADTS/H26x、兼容诊断
- **完成标准**: `cargo test -p cheetah-codec ts_` 覆盖标准和 ZLM/libmpeg 非标准样例，HLS 和 TS 可共用同一套 TS 容器 API

---

## 1.1 stream_type 与 descriptor 矩阵校准

**ZLMediaKit 参考**:

- `vendor-ref/ZLMediaKit/src/Extension/Frame.h`
- `vendor-ref/ZLMediaKit/3rdpart/media-server/libmpeg/include/mpeg-proto.h`

**目标映射**:

| CodecId | stream_type | descriptor | 说明 |
|---------|-------------|------------|------|
| H264 | `0x1B` | 无 | 标准 AVC |
| H265 | `0x24` | 无 | 标准 HEVC |
| H266 | `0x33` | 无 | 标准 VVC |
| AAC | `0x0F` | 无 | ADTS AAC |
| MP2 | `0x03` | 无 | MPEG-1 Audio |
| MP3 | `0x04` | 无 | MPEG-2 Audio / MP3 兼容 |
| G711A | `0x90` | 无 | ZLM/libmpeg 私有 |
| G711U | `0x91` | 无 | ZLM/libmpeg 私有 |
| OPUS | output `0x06`, input also `0x9C` | `"Opus"` | ETSI draft + libmpeg 兼容 |
| VP8 | `0x9D` | 可选 `"VP80"` | libmpeg 私有 |
| VP9 | `0x9E` | `"VP09"` | libmpeg 私有 |
| AV1 | `0x9F` | `"AV01"` | AV1 in MPEG-TS de facto |

**实现要求**:

- `stream_type_for_codec()` 输出侧按上表生成
- `codec_from_stream_type()` 输入侧兼容 `0x9C` OPUS
- `identify_private_stream()` 支持 registration descriptor `"Opus"`、`"VP09"`、`"AV01"`
- 未知 `0x06` private stream 输出 diagnostic，不中断 demux

**测试要求**:

- 为每个 codec 写 stream_type roundtrip 测试
- 写 private descriptor 识别测试
- 写未知 private stream diagnostic 测试

---

## 1.2 H26x / AAC 封装规范化

**ZLMediaKit 参考**:

- `Record/MPEG.cpp`: H264/H265 通过 `FrameMerger` 合并同 DTS 的 SPS/PPS/IDR
- `Record/MPEG.cpp`: AAC 要求 frame 带 ADTS header

**实现要求**:

- H264/H265/H266 输出前确保 Annex-B 格式
- H264/H265 输出 PES 前补 AUD；H266 若项目已有 AUD 常量则补，否则只保持 Annex-B
- 关键帧前优先从 `TrackInfo.extradata` 补 VPS/SPS/PPS
- AAC raw frame 输出 TS 前封装 ADTS
- demux AAC ADTS 后输出 `FrameFormat::AacRaw`，并从 ADTS 推导 `CodecExtradata::AAC { asc }`
- demux H26x 时剥离或忽略 AUD，不把 AUD 当作关键帧依据

**测试要求**:

- H264: SPS/PPS + IDR 同一 AU 输出后可 demux 为 key frame
- H265: VPS/SPS/PPS + IDR 输出后可 demux 为 key frame
- AAC: raw AAC mux 后 payload 含 ADTS，demux 后恢复 raw payload 和 ASC
- 非关键帧不重复补参数集

---

## 1.3 PAT/PMT/PES/PCR 鲁棒性

**ZLMediaKit 参考**:

- `Record/MPEG.cpp` 使用 libmpeg 自动输出 PAT/PMT/PCR/PES
- `TSMediaSourceMuxer::onWrite()` 以 key position 标记 ring cache

**实现要求**:

- PAT/PMT section length 必须精确，CRC 必须可校验
- PMT 支持多个 video/audio PID，PCR PID 选择首个视频，否则首个音频
- PES packet length：视频和超长音频允许 `0`，短音频填写实际长度
- PCR 在关键帧首包或配置周期到达时输出
- continuity counter 每 PID 独立递增并 wrap
- adaptation field stuffing 不得覆盖 payload

**测试要求**:

- PAT/PMT CRC 测试
- 多 video + 多 audio PMT 测试
- PES payload 跨多个 TS packet 测试
- PCR 存在性测试
- continuity counter wrap 测试

---

## 1.4 demux 容错与诊断

**ZLMediaKit 参考**:

- `HttpTSPlayer::onResponseBody()` 任意 chunk 输入 TS decoder
- `WebSocketSplitter` 处理粘包/半包

**实现要求**:

- demux `push()` 接受任意切片，不要求 188 对齐
- 前导垃圾触发 `SyncLoss` diagnostic 并重同步
- adaptation field 长度越界时丢包并诊断
- strict CRC=false 时 PAT/PMT CRC 错误仅诊断，strict CRC=true 时拒绝该 section
- continuity gap 诊断后继续尝试重组；PUSI 新 PES 到达时 flush 旧 PES
- 每 PID reassembly buffer 超过 `max_reassembly_bytes` 时清空该 PID 并诊断
- PTS/DTS 33-bit wrap 后输出单调展开时间

**测试要求**:

- 半包、粘包、前导垃圾重同步
- adaptation field 越界
- CRC strict/loose 双模式
- PES overflow
- PTS wrap 前后两帧单调

---

## 1.5 HLS/TS 共享容器 API

**本地问题**:

当前 HLS core 和 `cheetah-codec` 都存在 TS mux/demux 逻辑，长期会导致 stream_type、ADTS、参数集和时间戳修复分叉。

**实现要求**:

- HLS TS segment 生成改为调用 `cheetah-codec::MpegTsMuxer`
- HLS TS demux 改为调用 `cheetah-codec::MpegTsDemuxer`
- HLS 保留 segment/playlist 业务逻辑，不保留私有 TS packet writer
- 删除或弃用 HLS 私有 TS helper 前，先保证现有 HLS 测试全部通过

**测试要求**:

- `cargo test -p cheetah-codec ts_`
- `cargo test -p cheetah-hls-core`
- HLS TS segment 输出仍然 188 对齐，并以 PAT/PMT 开头

---

## 验证命令

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec ts_
cargo test -p cheetah-hls-core
```
