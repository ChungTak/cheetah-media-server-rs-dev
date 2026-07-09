# fMP4 与 ZLMediaKit 差距分析

- **状态**: 规划中
- **范围**: 记录 ZLM fMP4 真实落地行为、当前 Cheetah 状态、缺口和后续修复优先级。
- **完成标准**: 后续阶段能按本文逐项补齐 HTTP/WS fMP4、容器鲁棒性、demand mode、多编码、多轨和互操作测试。

## ZLM 关键行为

| 领域 | ZLM 参考 | 行为 |
|------|----------|------|
| fMP4 live source | `FMP4/FMP4MediaSource.h` | 保存 init segment；media fragment 写入 ring；有视频时 GOP cache 以关键帧为边界 |
| fMP4 source muxer | `FMP4/FMP4MediaSourceMuxer.h` | `fmp4_demand` 控制是否按需生成；无人观看时标记清缓存 |
| HTTP/WS route | `Http/HttpSession.cpp` | `.live.mp4` 进入 fMP4 live；WebSocket 与 HTTP 共用 `checkLiveStreamFMP4` |
| WebSocket | `Http/WebSocketSplitter.*` | 支持粘包/半包/continuation；组合包默认 4 MiB 上限 |
| MP4 mux | `Record/MP4Muxer.*` | init segment 单独生成；每次输入新 frame 前 flush 上一段；有视频时首帧必须关键帧 |
| 协议配置 | `Common/config.cpp` | `enable_fmp4=1`，`fmp4_demand=0`，`max_track=2` |
| 多协议输出 | `Common/MultiMediaSourceMuxer.cpp` | fMP4 与 RTMP/RTSP/TS/HLS/MP4 并行挂接到同一源 |

## 当前 Cheetah 状态

| 能力 | 当前状态 | 备注 |
|------|----------|------|
| 三段式 crate | 已有 | `core/driver-tokio/module` 已在 workspace |
| HTTP `.mp4` / `.live.mp4` | 已有基础解析 | core 单测覆盖基本路由 |
| WS upgrade | 已有基础校验 | 需要补 Sec-WebSocket-Accept 校验、continuation、close 细节测试 |
| HTTP chunked 输出 | 已有基础编码 | 缺完整 server 集成测试 |
| HTTPS/WSS server | 配置已有，driver 仍是占位 | `tls.rs` 尚未实现 |
| pull client | 已有 HTTP/HTTPS/WS/WSS 基础 | HTTP chunked body 未解码；WS continuation 忽略；mask key 固定 |
| mux/demux | 已在 `cheetah-codec` | `include_sidx` 配置存在但尚未实际写 `sidx` |
| codec matrix | 多数已实现 | 需要补全测试和输入兼容样例 |
| 多轨 property test | 已有基础 | 需要补 multi-traf offset、track id remap、超限行为 |
| demand mode | 配置存在 | module 尚未按 ZLM 语义实现 |
| 端到端互操作 | 缺少 | 需要 ZLM/ffmpeg/VLC 样例 |

已通过的基线命令：

```bash
cargo test -p cheetah-codec -- fmp4
cargo test -p cheetah-fmp4-core -p cheetah-fmp4-driver-tokio -p cheetah-fmp4-module -p cheetah-fmp4-property-tests --no-fail-fast
```

## 标准 fMP4 要求

- init segment 使用 `ftyp + moov`。
- `moov` 包含 `mvex/trex`。
- media segment 使用 `styp + sidx + moof + mdat` 或兼容 `moof + mdat`。
- `moof` 包含 `mfhd` 和一个或多个 `traf`。
- `traf` 包含 `tfhd/tfdt/trun`。
- `tfhd` 使用 `default-base-is-moof`。
- `trun` 使用 `data-offset-present`。
- B-frame 使用 signed composition time offset。
- H264/H265 样本进入 MP4 前必须是 4 字节 length-prefixed NALU。
- 输出时间戳来自 canonical `AVFrame.pts/dts`，按 track timescale 写入。

## 实际落地兼容点

| 兼容点 | 计划处理 |
|--------|----------|
| ZLM 使用 `.live.mp4` 暴露 HTTP/WS fMP4 | 保持 `.mp4` 和 `.live.mp4` 双兼容 |
| HTTP-fMP4 是长连接流，不是普通文件下载 | 始终 chunked streaming，先 init 后 fragment |
| WebSocket-fMP4 仍使用 `.live.mp4` 路径 | WebSocket upgrade 后输出 binary fMP4 |
| ZLM init segment 单独缓存 | 每连接先发 muxer init；track/config 变化重发 init |
| ZLM 有视频时首帧要求关键帧 | module 起播等待关键帧 |
| ZLM demand mode 无人观看清缓存 | Cheetah 用 `demand_mode` 控制按需 mux 和重新起播 |
| ZLM WebSocket message 上限 4 MiB | driver/pull 统一 `max_ws_message_bytes`，默认 4 MiB |
| 输入端可能无 `styp`/`sidx` | demux 接受 `moof+mdat` |
| 输入端可能重复 init | demux 更新 tracks，module 标记 discontinuity |
| 弱标准 codec in MP4 | 容器层稳定，播放器支持限制写入文档和 diagnostic |

## 编码矩阵

| 编码 | 输出 entry | 输入兼容 | 配置 box | 样本格式 |
|------|------------|----------|----------|----------|
| H264 | `avc1` | `avc1/avc2/avc3/avc4` | `avcC` | 4-byte NALU length |
| H265 | `hvc1` | `hvc1/hev1/dvh1/dvhe` | `hvcC` | 4-byte NALU length |
| AAC | `mp4a` | `mp4a` object `0x40` | `esds` | raw AAC AU |
| G711A | `alaw` | `alaw` | none | raw G711 packet |
| G711U | `ulaw` | `ulaw` | none | raw G711 packet |
| Opus | `Opus` | `Opus` | `dOps` | Opus packet |
| MJPEG | `mp4v` | `mp4v/jpeg/mjpa/mjpb` | `esds` object `0x6C` | JPEG frame |
| MP2 | `mp4a` | object `0x6B` | `esds` | MP2 frame |
| MP3 | `mp4a` | object `0x69/0x6B` | `esds` | MP3 frame |
| VP8 | `vp08` | `vp08` | `vpcC` | VP8 frame |
| VP9 | `vp09` | `vp09` | `vpcC` | VP9 frame |
| AV1 | `av01` | `av01` | `av1C` | AV1 OBU |

## 必须补齐的缺口

1. `include_sidx=true` 时实际写 `sidx`，并补 demux skip/parse 测试。
2. 实现 fMP4 HTTPS/WSS server listener。
3. pull client 正确解析 HTTP chunked body，不把 chunk header 交给 demux。
4. pull client 和 server 支持 WebSocket continuation reassembly。
5. WebSocket client mask key 使用随机值，不能固定。
6. module 实现 `demand_mode` 的按需 mux 和清旧缓存语义。
7. track/config 变化时重建 muxer、重发 init、标记 discontinuity。
8. 多轨超过上限时跳过超限 track 并输出 diagnostic。
9. 增加 ZLM/ffmpeg/VLC 互操作 fixture 和 fault corpus。
10. 增加端到端 HTTP/WS/TLS/pull 测试，不只保留单元测试。

