# fMP4 协议完善计划（对标 ZLMediaKit）

- **状态**: 规划中
- **目标**: 在当前已实现 fMP4 基础上，完善标准 fMP4、HTTP(S)-fMP4、WS(S)-fMP4、多编码、多轨道、真实互操作和鲁棒性。
- **参考实现**: `vendor-ref/ZLMediaKit/src`
- **计划目录**: `dev-docs/plans-24-fmp4-zlm`

## 当前基线

当前仓库已经具备 fMP4 三段式 crate 和共享容器实现：

- `crates/protocols/fmp4/core`
- `crates/protocols/fmp4/driver-tokio`
- `crates/protocols/fmp4/module`
- `crates/protocols/fmp4/testing/property-tests`
- `crates/foundation/cheetah-codec/src/fmp4_mux.rs`
- `crates/foundation/cheetah-codec/src/fmp4_demux.rs`

已验证通过：

```bash
cargo test -p cheetah-codec -- fmp4
cargo test -p cheetah-fmp4-core -p cheetah-fmp4-driver-tokio -p cheetah-fmp4-module -p cheetah-fmp4-property-tests --no-fail-fast
```

因此本计划不是从零新增 fMP4，而是在现有基础上补齐 ZLMediaKit 落地行为、非标准兼容、TLS/WS/pull 细节和端到端互操作。

## V1 完善范围

1. 支持 HTTP-fMP4 / HTTPS-fMP4 直播播放。
2. 支持 WS-fMP4 / WSS-fMP4 直播播放。
3. 支持远端 HTTP(S)/WS(S)-fMP4 拉流发布到 engine。
4. 支持 H264/H265/AAC/G711/OPUS/MJPEG/MP3/VP8/VP9/AV1/MP2。
5. 支持多轨道模式，允许多个 video/audio track。
6. 输入侧兼容实际落地的非标准 fMP4：无 `styp`、无 `sidx`、重复 init、unknown box、任意 chunk 切分、WebSocket continuation、弱标准 sample entry。

## ZLMediaKit 关键参考

| 领域 | 文件 | 重点行为 |
|------|------|----------|
| fMP4 live source | `vendor-ref/ZLMediaKit/src/FMP4/FMP4MediaSource.h` | init segment 缓存、fragment ring、GOP cache、reader 变化 |
| fMP4 source muxer | `vendor-ref/ZLMediaKit/src/FMP4/FMP4MediaSourceMuxer.h` | `fmp4_demand`、无人观看清缓存、segment 输出 |
| HTTP/WS 播放入口 | `vendor-ref/ZLMediaKit/src/Http/HttpSession.cpp` | `.live.mp4` 路由，HTTP/WS 共用 fMP4 播放入口 |
| WebSocket framing | `vendor-ref/ZLMediaKit/src/Http/WebSocketSplitter.*` | 4 MiB message 上限、mask、continuation、ping/pong |
| fMP4 mux | `vendor-ref/ZLMediaKit/src/Record/MP4Muxer.*` | init segment、每帧前 flush 上个 segment、关键帧起播 |
| 协议开关 | `vendor-ref/ZLMediaKit/src/Common/config.cpp` | `enable_fmp4=1`、`fmp4_demand=0`、`max_track=2` 默认 |
| 多协议 mux | `vendor-ref/ZLMediaKit/src/Common/MultiMediaSourceMuxer.cpp` | 与 RTMP/RTSP/TS/HLS/MP4 并行输出 |

## 计划文件清单

| 文件 | 范围 |
|------|------|
| [fmp4-architecture.md](fmp4-architecture.md) | fMP4 总体架构、crate 边界、数据流、ZLM 对齐策略 |
| [fmp4-zlm-gap-analysis.md](fmp4-zlm-gap-analysis.md) | ZLM 行为、当前实现状态、缺口和非标准兼容点 |
| [phase-01-codec-fmp4-container.md](phase-01-codec-fmp4-container.md) | `cheetah-codec` fMP4 容器、codec matrix、多轨、box 鲁棒性 |
| [phase-02-core-driver-transport.md](phase-02-core-driver-transport.md) | HTTP/HTTPS、WS/WSS、TLS server、chunked pull、WebSocket framing |
| [phase-03-module-play-pull.md](phase-03-module-play-pull.md) | module 播放、拉流、demand mode、track/config 变化、多轨接入 |
| [phase-04-compat-interop-testing.md](phase-04-compat-interop-testing.md) | ZLM/ffmpeg/VLC 互操作、fault corpus、fuzz/property、生产验证 |

## 执行顺序

1. **Phase 01**: 先补齐共享 fMP4 容器语义，尤其 `sidx`、多 codec sample entry、重复 init、bounded demux。
2. **Phase 02**: 完善传输层，补 HTTPS/WSS server、HTTP chunked pull、WS continuation 和慢客户端隔离。
3. **Phase 03**: 完善 module 语义，补 ZLM 风格 demand mode、关键帧起播、track/config 变化和 pull supervisor。
4. **Phase 04**: 用 ZLM、ffmpeg、VLC、故障样例和性能测试收口。

## 总体验收

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec -- fmp4
cargo clippy -p cheetah-fmp4-core
cargo test -p cheetah-fmp4-core
cargo clippy -p cheetah-fmp4-driver-tokio
cargo test -p cheetah-fmp4-driver-tokio
cargo clippy -p cheetah-fmp4-module --tests
cargo test -p cheetah-fmp4-module
cargo test -p cheetah-fmp4-property-tests
(cd crates/protocols/fmp4/fuzz && cargo +nightly fuzz build)
```

