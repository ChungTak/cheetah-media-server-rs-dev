# fMP4 协议完善计划（对标 ABLMediaServer）

- **状态**: 规划中
- **目标**: 在当前已实现 fMP4 基础上，对齐 `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer` 的 HTTP-MP4、fMP4 录像、时间戳与兼容行为，补齐本地缺失能力。
- **参考实现**:
  - `vendor-ref/ABLMediaServer-src-2026-05-09/ABLMediaServer`
  - `vendor-ref/ABLMediaServer-src-2026-05-09/版本信息.txt`
- **计划目录**: `dev-docs/plans-24-fmp4-abl`

## 当前基线

当前仓库已经具备完整 fMP4 三段式 crate 和共享容器实现：

- `crates/protocols/fmp4/core`
- `crates/protocols/fmp4/driver-tokio`
- `crates/protocols/fmp4/module`
- `crates/protocols/fmp4/testing/property-tests`
- `crates/foundation/cheetah-codec/src/fmp4_mux.rs`
- `crates/foundation/cheetah-codec/src/fmp4_demux.rs`

已确认通过的基线命令：

```bash
cargo test -p cheetah-codec -- fmp4
cargo test -p cheetah-fmp4-core -p cheetah-fmp4-driver-tokio -p cheetah-fmp4-module -p cheetah-fmp4-property-tests --no-fail-fast
```

因此本计划不是从零实现 fMP4，而是在现有实现上补 ABL 风格兼容、录像相关后续阶段和真实互操作行为。

## ABL 参考重点

本轮对齐主要阅读并吸收以下内容：

| 领域 | 文件 | 重点 |
|------|------|------|
| HTTP-MP4 直播 | `ABLMediaServer/NetServerHTTP_MP4.cpp` | `.mp4` 长连接播放、chunked 编码、关键帧起播、AAC/G711/H264/H265 处理 |
| fMP4 录像 | `ABLMediaServer/StreamRecordFMP4.cpp` | fMP4 切片写盘、init segment 写入、切片完成通知 |
| 配置 | `ABLMediaServer-发表版本配置.ini` | `httpMp4Port`、`pushEnable_mp4`、`videoFileFormat`、`fileSecond`、`ForceSendingIFrame` |
| 版本演进 | `版本信息.txt` | 真实帧率驱动时间戳、录像回放修正、下载/回放控制、H265 fMP4 切片优化 |

## V1 完善范围

第一阶段固定覆盖：

1. HTTP(S)-fMP4 / WS(S)-fMP4 直播播放的 ABL 兼容行为补齐。
2. 远端 HTTP(S)/WS(S)-fMP4 拉流发布的鲁棒性补齐。
3. `cheetah-codec` 中与 ABL 相关的时间戳、关键帧起播、参数集、sample entry 兼容补强。
4. 非标准输入兼容：重复 init、无 `styp/sidx`、任意 chunk 切分、unknown box、弱标准 sample entry。
5. ABL 录像切片/回放/合并下载能力作为后续阶段单列规划，不阻塞直播能力完善。

第一阶段不直接实现：

1. 录像控制 HTTP API。
2. 多文件录像合并下载。
3. 单文件录像 seek / pause / scale 控制。
4. `on_record_*` hook 全链路。

这些能力统一放到后续 `Phase 04`。

## 与现有 ZLM/SMS 计划的关系

- 继续复用 `plans-24-fmp4-zlm` / `plans-24-fmp4-sms` 中已经成立的三段式边界和共享容器方向。
- ABL 计划只新增差量结论，不回退已经完成的 `sidx`、TLS server、chunked pull、WebSocket Accept 校验等能力。
- ABL 更强调录像、真实帧率、I 帧快速起播和下载回放，这些是本目录的新增重点。

## 计划文件清单

| 文件 | 范围 |
|------|------|
| [fmp4-architecture.md](fmp4-architecture.md) | ABL 对齐总体架构、crate 边界、直播与录像能力归属 |
| [fmp4-abl-gap-analysis.md](fmp4-abl-gap-analysis.md) | ABL 行为、版本信息结论、当前实现状态、缺口 |
| [phase-01-codec-fmp4-container.md](phase-01-codec-fmp4-container.md) | `cheetah-codec` 容器、时间戳、参数集、关键帧起播、sample entry |
| [phase-02-core-driver-transport.md](phase-02-core-driver-transport.md) | HTTP/HTTPS、WS/WSS、chunked、WebSocket、TLS、拉流字节流 |
| [phase-03-module-play-pull.md](phase-03-module-play-pull.md) | 播放会话、pull supervisor、bootstrap/GOP、track 变化 |
| [phase-04-recording-replay-compat.md](phase-04-recording-replay-compat.md) | fMP4 录像切片、回放、下载、hook/config 后续阶段 |
| [phase-05-interop-testing.md](phase-05-interop-testing.md) | ABL/ffmpeg/VLC/故障样例/fuzz/回归验证 |

## 执行顺序

1. **Phase 01**: 先补共享容器和时间戳语义，避免 ABL 兼容逻辑散落在 module/driver。
2. **Phase 02**: 完善 HTTP-MP4 / WS-fMP4 传输与拉流行为，对齐 chunked 和 WebSocket 实战细节。
3. **Phase 03**: 完善 module 的关键帧起播、bootstrap、pull job 和 track/config 变化。
4. **Phase 04**: 再引入 ABL 风格录像切片、录像回放、合并下载与 hook 语义。
5. **Phase 05**: 用 ABL 样例、ffmpeg、VLC、fault corpus 和 fuzz 收口。

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
