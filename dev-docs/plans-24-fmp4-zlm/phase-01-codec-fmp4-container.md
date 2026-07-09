# Phase 01 — 共享 fMP4 容器补强

- **状态**: 规划中
- **范围**: 在 `cheetah-codec` 中补齐标准 fMP4 容器输出、输入鲁棒性、多编码 sample entry、多轨道和 ZLM 兼容样例。
- **完成标准**: `cheetah-codec` 能稳定 mux/demux 标准与 ZLM 风格 fMP4，HLS/fMP4 module 共用同一容器实现。

## 1.1 `sidx` 输出与兼容输入

当前 `Fmp4MuxerConfig.include_sidx` 已存在，但 media segment 仍只写 `styp + moof + mdat`。需要补齐：

- `include_sidx=true` 时在 `moof` 前写 `sidx`。
- `sidx` reference size 指向后续 `moof + mdat`。
- reference duration 使用当前 fragment duration。
- demux 输入端跳过或解析 `sidx`，不能影响后续 `moof/mdat`。
- 测试覆盖 `include_sidx=true/false`。

## 1.2 box parser 鲁棒性

demux 必须有界处理：

- 32-bit box size。
- 64-bit largesize。
- size 0 extends-to-end。
- unknown top-level box。
- nested unknown box。
- box size 小于 header。
- `mdat` 超出 `max_box_bytes`。
- `moof` 缺失、`mdat` 缺失。
- repeated init segment。

失败策略：

- 可恢复时输出 diagnostic 并继续。
- 不可恢复时丢弃当前 fragment。
- buffer 不允许无界增长。

## 1.3 sample entry 矩阵

输出默认：

| CodecId | Entry | Config |
|---------|-------|--------|
| H264 | `avc1` | `avcC` |
| H265 | `hvc1` | `hvcC` |
| AAC | `mp4a` | `esds` object `0x40` |
| G711A | `alaw` | none |
| G711U | `ulaw` | none |
| Opus | `Opus` | `dOps` |
| MJPEG | `mp4v` | `esds` object `0x6C` |
| MP2 | `mp4a` | `esds` object `0x6B` |
| MP3 | `mp4a` | `esds` object `0x69` |
| VP8 | `vp08` | `vpcC` |
| VP9 | `vp09` | `vpcC` |
| AV1 | `av01` | `av1C` |

输入额外兼容：

- H264: `avc2/avc3/avc4`
- H265: `hev1/dvh1/dvhe`
- MJPEG: `jpeg/mjpa/mjpb`
- MP3: `0x69/0x6B`

## 1.4 payload 与时间戳

- H264/H265 输入 MP4 时保持 4 字节 length-prefixed NALU。
- H264/H265 输出到 engine 时转 canonical Annex-B/H26x payload。
- AAC 使用 raw access unit，配置来自 ASC。
- G711/Opus/MJPEG/MP2/MP3/VP8/VP9/AV1 样本原样进入对应 canonical frame format。
- `tfdt + trun` 展开为 microsecond `pts/dts`。
- B-frame `composition time offset` 必须支持负值。
- timescale 为 0 时输出 diagnostic，不 panic。

## 1.5 多轨道 fragment

- 每个有样本的 track 写一个 `traf`。
- fragment 内允许多个 `traf`。
- `trun.data_offset` 必须指向该 track 在 `mdat` 中的样本区域。
- track id 冲突时稳定重映射。
- 超过 `max_tracks` 的 track 被跳过并输出 diagnostic。
- property test 覆盖多 audio、多 video、video+audio 混合。

## 1.6 测试

新增或补齐：

```bash
cargo test -p cheetah-codec -- fmp4
cargo test -p cheetah-fmp4-property-tests
```

测试场景：

- init segment 包含 `ftyp/moov/mvex/trex`。
- media segment 包含可选 `styp/sidx` 和必选 `moof/mdat`。
- `tfhd default-base-is-moof`。
- `trun.data_offset` 对多轨正确。
- H264/H265/AAC/G711/Opus/MJPEG/MP3/VP8/VP9/AV1/MP2 sample entry roundtrip。
- arbitrary chunk split 与 single push 结果一致。
- repeated init 不泄漏旧 track。
- oversized box 有 diagnostic 且 bounded。

