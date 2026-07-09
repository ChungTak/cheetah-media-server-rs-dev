# fMP4 与 SimpleMediaServer 差距分析

- **状态**: 规划中
- **范围**: 记录 SMS fMP4 实现中的标准行为、实际落地兼容点、本项目现状和实现缺口。
- **完成标准**: 实现阶段可以按本文逐项补齐 fMP4 容器、HTTP/WS 直播、拉流与互操作测试。

## SMS 关键路径

| 领域 | SMS 文件 | 观察到的行为 |
|------|----------|--------------|
| HTTP/WS 路由 | `Http/HttpConnection.cpp` | `.mp4` 进入 `handleFmp4()`，WebSocket 与 HTTP 共用播放入口 |
| fMP4 媒体源 | `Mp4/Fmp4MediaSource.*` | 保存 init segment，ring 中持续写 media segment |
| fMP4 mux | `Mp4/Fmp4Muxer.*` | 写 `ftyp/moov`、`styp/sidx/moof/mdat`、`mfra`，支持多 track |
| fMP4 demux | `Mp4/Fmp4Demuxer.*` | H26x length-prefixed 转 Annex-B，AAC 补 ADTS，其他编码直接透传 |
| MP4 box / codec tag | `Mp4/Mp4Box.*` | 维护 sample entry、ObjectType、box flag 常量和 tag 映射 |
| codec object mapping | `Mp4/Mp4Muxer.cpp` | 映射 AAC/G711/MP3/Opus/H264/H265/VP8/VP9/H266/AV1 |

## SMS 标准行为

- init segment：`ftyp + moov`
- fragmented segment：`styp + sidx + moof + mdat`
- `moov` 内包含 `mvex/trex`
- `moof` 内包含 `mfhd` 和每个 track 的 `traf`
- `traf` 内包含 `tfhd/tfdt/trun`
- `tfhd` 使用 `default-base-is-moof`
- `trun` 使用 `data-offset-present`
- `tfdt` 使用 base media decode time
- `mdat` 大于 32-bit size 时写 largesize
- H264/H265/H266 写入 MP4 时转换为 4 字节 length-prefixed NALU

## SMS 落地兼容行为

1. **直播 URL 使用 `.mp4`**  
   SMS API 输出 `http-mp4/https-mp4/ws-mp4/wss-mp4`，路径是原 stream path 加 `.mp4`。本项目首版必须兼容该形态。

2. **HTTP 播放为长连接 chunked**  
   `handleFmp4()` 在非 WebSocket 下设置 `Transfer-Encoding: chunked`，先发送 fMP4 header，再从 ring 推送 fragment。

3. **WebSocket 播放仍用 `.mp4` 路径**  
   WebSocket 与 HTTP 共享 `handleFmp4()`，成功后发送 binary fMP4 数据。

4. **低延迟 fragment 策略偏激进**  
   `Fmp4Muxer::inputFrame_l()` 当前代码中 `if (true || flags || elapsed > 50)` 导致几乎每帧都可触发 fragment flush。Cheetah 首版采用可配置策略：默认关键帧与最大 fragment 时长触发，低延迟模式可开启更小 fragment。

5. **H26x 配置帧不单独输出为普通样本**  
   `Fmp4MediaSource::onFrame()` 跳过 `metaFrame()`，配置主要进入 sample entry 或关键帧前聚合。Cheetah 需要统一依赖 `TrackInfo.extradata` 和 `cheetah-codec` 参数集缓存。

6. **MP3 ObjectType 按采样率区分**  
   SMS 中 `mp3` 当 samplerate > 24000 时使用 `MOV_OBJECT_MP1A`，否则使用 `MOV_OBJECT_MP3`。Cheetah 输出 MP3 使用 `0x69`，输入兼容 `0x69/0x6B` 并记录诊断。

7. **G711 使用非标准 sample entry**  
   SMS 使用 `alaw/ulaw`，Cheetah 保持兼容。

8. **VP8/VP9/AV1 in MP4 依赖现代浏览器/播放器能力**  
   Cheetah 首版承诺 mux/demux 稳定和 track/frame 识别，不承诺所有客户端可播放。

## 本项目现状

| 能力 | 当前位置 | 状态 |
|------|----------|------|
| fMP4 mux | `crates/protocols/hls/core/src/fmp4_mux.rs` | 可用于 HLS，尚未成为共享 codec API |
| fMP4 demux | `crates/protocols/hls/core/src/fmp4_demux.rs` | 可解析基础 init/segment，错误模型和鲁棒性不足 |
| MP4 sample entry helper | `crates/foundation/cheetah-codec/src/mp4.rs` | 仅有 `Mp4SampleEntry` 雏形 |
| HTTP/WS 长连接 | `crates/protocols/ts/`、`crates/protocols/http-flv/` | 可复用设计，不应复制业务状态 |
| TLS server | TS/HLS/HTTP-FLV driver | 可复用 driver 层封装 |
| pull client | TS/HTTP-FLV | TS 支持 http/https/ws/wss，HTTP-FLV 有更完整限流和 chunked 思路 |
| codec model | `cheetah-codec` | 已有 H264/H265/H266/AAC/G711/Opus/MP2/MP3/VP8/VP9/AV1，缺 MJPEG |

## 必须补齐的实现缺口

1. 将 HLS fMP4 mux/demux 提升到 `cheetah-codec`
2. 补齐 robust MP4 box parser：size 0、largesize、unknown box、nested box、bounded buffer
3. 补齐 sample entry 矩阵：H264/H265/H266/AAC/G711/Opus/MJPEG/MP2/MP3/VP8/VP9/AV1
4. 补齐 `sidx`、`styp`、`tfhd` default fields、`trun` flags、B-frame signed CTS
5. 补齐 MJPEG codec model 与 compat 名称
6. 新增独立 `cheetah-fmp4-core`
7. 新增独立 `cheetah-fmp4-driver-tokio`
8. 新增独立 `cheetah-fmp4-module`
9. HLS 改为复用 `cheetah-codec` fMP4 API
10. 建立 SMS/ffmpeg/VLC fixture 与故障样例

## 编码矩阵

| 编码 | 输出 sample entry | 输入兼容 | 配置 box | 样本格式 |
|------|-------------------|----------|----------|----------|
| H264 | `avc1` | `avc1/avc2/avc3/avc4` | `avcC` | 4-byte NALU length |
| H265 | `hvc1` | `hvc1/hev1/dvh1/dvhe` | `hvcC` | 4-byte NALU length |
| AAC | `mp4a` | `mp4a` | `esds` | raw AAC access unit |
| G711A | `alaw` | `alaw` | none | raw G711 packet |
| G711U | `ulaw` | `ulaw` | none | raw G711 packet |
| Opus | `Opus` | `Opus` | `dOps` | Opus packet |
| MJPEG | `mp4v` | `mp4v/jpeg/mjpa/mjpb` | `esds` with object `0x6C` | JPEG frame |
| MP2 | `mp4a` | `mp4a` object `0x6B` | `esds` | MP2 frame |
| MP3 | `mp4a` | `mp4a` object `0x69/0x6B` | `esds` | MP3 frame |
| VP8 | `vp08` | `vp08` | `vpcC` | VP8 frame |
| VP9 | `vp09` | `vp09` | `vpcC` | VP9 frame |
| AV1 | `av01` | `av01` | `av1C` | AV1 OBU |

## 互操作风险

- Safari/HLS 更偏好 `hvc1`，部分 DASH/MSE 内容使用 `hev1`；输出默认 `hvc1`，输入双兼容
- fMP4 中 MP2/MJPEG/G711 是弱播放器支持路径；测试重点是容器正确和跨协议稳定
- 每帧一个 fragment 延迟低但 overhead 高；默认不照搬 SMS 的极端 flush 策略
- WebSocket continuation 必须重组后再交给 demux，不能把 fragment frame 直接当完整 MP4 segment
- 远端可能重复发送 init segment；demux 应更新 track 并标记 discontinuity
