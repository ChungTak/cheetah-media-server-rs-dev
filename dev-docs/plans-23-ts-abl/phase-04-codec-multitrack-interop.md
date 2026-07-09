# Phase 04 — 编码、多轨道、互操作与鲁棒性验收

- **状态**: 计划中
- **范围**: 支持用户要求的编码矩阵、多轨道模式、ABL/ZLM/ffmpeg/VLC 互操作、故障样例、fuzz/property 和性能背压。
- **完成标准**: 每个目标编码和传输模式都有自动测试或明确的手动验收脚本，真实脏数据不会导致 panic、无限缓存或跨连接连带失败。

---

## 4.1 编码矩阵

必须覆盖：

| 编码 | mux | demux | HTTP/WS 输出 | RTP-TS 输入 | 说明 |
|------|-----|-------|--------------|-------------|------|
| H264 | 必测 | 必测 | 必测 | 必测 | Annex-B、AUD、SPS/PPS、keyframe |
| H265 | 必测 | 必测 | 必测 | 必测 | VPS/SPS/PPS、keyframe |
| AAC | 必测 | 必测 | 必测 | 必测 | ADTS/raw 转换、ASC 推导 |
| G711A | 必测 | 必测 | 必测 | 必测 | stream_type `0x90` |
| G711U | 必测 | 必测 | 必测 | 必测 | stream_type `0x91` |
| OPUS | 必测 | 必测 | 必测 | 可选 | `0x06+"Opus"` 与 `0x9C` |
| MP3 | 必测 | 必测 | 必测 | 可选 | stream_type `0x04` |
| MP2 | 必测 | 必测 | 必测 | 可选 | stream_type `0x03` |
| VP8 | 必测 | 必测 | 可选 | 可选 | 私有 stream_type，播放器兼容有限 |
| VP9 | 必测 | 必测 | 可选 | 可选 | 私有 stream_type/descriptor |
| AV1 | 必测 | 必测 | 可选 | 可选 | 私有 stream_type/descriptor |

---

## 4.2 多轨道模式

**要求**:

1. PMT 支持多个 video/audio PID。
2. PID 分配稳定：video 从 `0x0100`，audio 从 `0x0110`，同类按 `TrackId` 排序。
3. PCR PID 选择第一个 video；无 video 时选择第一个 audio。
4. `max_tracks` 超限时输出 diagnostic，不 panic。
5. pull/ingest 发现新 track 时追加到 `update_tracks()`，不得覆盖旧 track。
6. 多 program MPTS 首版只发布配置选中的 program；未配置时选择第一个有效 program 并诊断。

**组合测试**:

1. H264 + AAC
2. H265 + G711A + G711U
3. H264 + AAC + OPUS
4. H264/H265 双视频 + AAC/MP3 双音频
5. audio-only AAC
6. audio-only G711

---

## 4.3 ABL/ZLM/ffmpeg/VLC 互操作

**手动验收示例**:

```bash
ffprobe -hide_banner http://127.0.0.1:8082/live/test.ts
ffplay -fflags nobuffer http://127.0.0.1:8082/live/test.ts
ffprobe -hide_banner http://127.0.0.1:8082/live/test.live.ts
```

**ABL 对照验收**:

1. ABL 风格 HTTP-TS URL 能被本地 pull。
2. 本地 HTTP-TS 能被 ABL 或 ffmpeg 拉取。
3. RTP-TS fixture 可复现海康/国标兼容场景。
4. G711/AAC/H264/H265 输入输出时间戳不倒退。

**fixture 要求**:

新增 `crates/protocols/ts/module/tests/fixtures/README.md`，记录来源、编码、轨道数、是否 B 帧、是否含非标准 stream_type、是否来自 ABL/ZLM/ffmpeg。

---

## 4.4 故障输入

| 故障 | 期望 |
|------|------|
| RTP 非 v2 | diagnostic 后丢包 |
| RTP header extension 越界 | diagnostic 后丢包 |
| RTP payload 非 188 对齐 | 尝试重同步 |
| TS 前导垃圾 | `SyncLoss` 后重同步 |
| TS sync byte 损坏 | 丢坏包继续 |
| PAT/PMT CRC 错误 | loose 诊断继续，strict 拒绝 |
| continuity gap | 诊断，下一 PUSI 重同步 |
| PES 超上限 | 清 PID buffer |
| AAC ADTS 长度错误 | 诊断并跳过坏 frame |
| WebSocket unmasked client frame | 关闭连接 |
| WebSocket frame 超上限 | 关闭连接 |
| HTTP chunked 截断 | pull error 并重试 |
| 空 body EOF | pull error |
| 慢客户端 | 只关闭该客户端 |

---

## 4.5 Fuzz / Property

建议新增：

1. `crates/protocols/ts/testing/property-tests`
2. `crates/protocols/ts/fuzz/`，默认不加入根 workspace

目标：

1. 任意 bytes 输入 TS demux 不 panic。
2. RTP parser 任意 bytes 不 panic。
3. mux 输出总是 188 对齐。
4. 任意切片方式喂 demux，与一次性喂入事件数量一致。
5. continuity counter 对每 PID 单调 wrap。
6. reassembly、websocket frame、write queue 内存不超过配置上限。

---

## 4.6 性能与观测

**性能验收**:

1. 100 个 HTTP-TS 播放者同时播放同一流。
2. 100 个 WS-TS 播放者同时播放同一流。
3. 1 个慢客户端不影响其它 99 个客户端。
4. 1000 个 RTP SSRC session 超过配置上限时拒绝新 session。

**指标**:

1. 当前 HTTP-TS / WS-TS 连接数。
2. 每连接写队列长度和关闭原因。
3. RTP session 数、sequence gap、sync loss、unknown payload 计数。
4. demux diagnostic 计数。
5. pull job 状态、重试次数、最近错误。
6. 每 stream 估计帧率。

---

## 验证命令

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo clippy -p cheetah-ts-core
cargo clippy -p cheetah-ts-driver-tokio
cargo clippy -p cheetah-ts-module
cargo test -p cheetah-codec ts_
cargo test -p cheetah-ts-core
cargo test -p cheetah-ts-driver-tokio
cargo test -p cheetah-ts-module
```
