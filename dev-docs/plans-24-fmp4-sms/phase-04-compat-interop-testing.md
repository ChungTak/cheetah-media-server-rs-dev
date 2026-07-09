# Phase 04 — 兼容性、互操作与生产化验证

- **状态**: 规划中
- **范围**: 建立 SMS/ffmpeg/VLC 互操作矩阵、真实故障样例、fuzz/property tests、性能背压验证和文档同步
- **完成标准**: fMP4 module 通过标准样例、SMS 样例、故障样例和端到端播放/拉流测试，并具备可诊断的生产运行指标

---

## 4.1 Fixture 与 Corpus

新增目录：

```text
crates/protocols/fmp4/module/tests/testdata/fmp4/
  README.md
  manifests/
  captures/
  faults/
```

样例来源：

- SMS `Fmp4Muxer` 输出样例
- ffmpeg 生成的 H264/AAC、H265/AAC、audio-only fMP4
- 手工构造 multi-track fMP4
- repeated init segment 样例
- `moof/mdat` 半包切分样例
- `trun.data_offset` 异常样例
- oversized box / largesize 样例
- 非标准 G711/Opus/MJPEG/VP8/VP9/AV1/MP2 样例

fixture policy：

- 标准样例做强行为断言
- 真实设备/SMS 样例做兼容断言
- fault 样例只要求 bounded、无 panic、可诊断

---

## 4.2 互操作矩阵

播放验证：

| Source | Egress | Client | 目标 |
|--------|--------|--------|------|
| RTMP H264/AAC | HTTP-fMP4 | ffplay | 正常播放 |
| RTSP H265/AAC | HTTP-fMP4 | ffplay/VLC | 正常播放或明确 codec 限制 |
| RTMP audio-only AAC | HTTP-fMP4 | ffplay | 正常播放 |
| RTSP G711 | HTTP-fMP4 | ffplay/VLC | 可解封装，播放能力按客户端 |
| local stream | WS-fMP4 | 测试客户端 | binary init/fragment 完整 |
| local stream | HTTPS/WSS-fMP4 | 测试客户端 | TLS 正常 |
| local stream | HTTP-fMP4 | SMS client | 能解析 init + fragment |

拉流验证：

| Remote | Ingest | Egress | 目标 |
|--------|--------|--------|------|
| ffmpeg HTTP-fMP4 | fMP4 pull | engine snapshot | tracks + frames 正确 |
| SMS HTTP-fMP4 | fMP4 pull | RTMP/RTSP/HLS | 跨协议可用 |
| WS-fMP4 test source | fMP4 pull | engine snapshot | binary 输入可用 |
| chunked HTTP-fMP4 | fMP4 pull | engine snapshot | chunked 可用 |
| repeated init source | fMP4 pull | engine snapshot | track 更新与 discontinuity 正确 |

编码覆盖：

- H264
- H265
- AAC
- G711A / G711U
- Opus
- MJPEG
- MP2
- MP3
- VP8
- VP9
- AV1

MJPEG/MP2/G711/VP8/VP9/AV1 in fMP4 是弱播放器支持路径，测试重点是 mux/demux 稳定和 track/frame 识别。

---

## 4.3 Robustness 测试

输入故障：

- box 前导垃圾
- 单字节/随机 chunk 切片
- box size 小于 header
- 64-bit largesize 越界
- `mdat` 缺失
- `moof` 缺失
- `traf` 缺少 `tfhd`
- `traf` 缺少 `tfdt`
- `trun` sample count 与 payload 不匹配
- `trun.data_offset` 指向 `mdat` 外
- H26x length prefix 越界
- unknown sample entry
- repeated init segment
- track id 不存在
- oversized box
- oversized fragment

期望：

- 不 panic
- buffer 不无界增长
- 能恢复则恢复
- 不能恢复则关闭单连接或重试单 pull job
- diagnostic 包含 box type、track id、connection/job、stream_key

---

## 4.4 Fuzz / Property-Based Testing

Property tests：

- 任意 chunk 切分 demux 结果一致
- 多轨 track id 唯一且稳定
- mux/demux roundtrip 保持 track 和 frame 基本属性
- `tfdt + trun` timestamp 展开单调性满足 frame ordering
- tolerant unknown box 不会拒绝其他合法 box

Fuzz targets：

- `fuzz_fmp4_box_parser`
- `fuzz_fmp4_init_segment`
- `fuzz_fmp4_media_fragment`
- `fuzz_fmp4_http_request`
- `fuzz_fmp4_ws_frames`

运行：

```bash
cargo test -p cheetah-fmp4-property-tests
(cd crates/protocols/fmp4/fuzz && cargo +nightly fuzz build)
```

---

## 4.5 性能与背压验证

验证项：

- 单连接持续输出无明显增长内存
- 100 个 HTTP-fMP4 播放连接下慢客户端不拖累快客户端
- WS binary message 聚合策略不造成高延迟
- pull job 断线重试不泄漏 publisher lease
- `max_box_bytes` 达上限后可关闭连接/重试
- 多轨道 `moof/traf/trun` 生成不产生过量小分配
- 每帧 fragment 与 1 秒 fragment 两种模式的 CPU/带宽开销可观测

建议指标：

- active fMP4 sessions
- bytes sent / received
- mux fragments / sec
- demux fragments / sec
- init segment resend count
- box parse error count
- dropped unsupported frames
- slow client close count
- pull reconnect count
- max fragment bytes observed
- max box reassembly bytes observed

---

## 4.6 文档同步

需要更新：

- `SystemArchitecture.md`
  - fMP4 crate 映射
  - fMP4 capability snapshot
  - CI/check baseline
- `dev-docs/SystemArchitecture.md`
  - fMP4 属于协议三段式
  - ISO BMFF/fMP4 容器能力归属 `cheetah-codec`
- README / 示例配置
  - fMP4 module feature
  - HTTP/WS 播放 URL
  - pull_jobs 配置样例

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo clippy -p cheetah-fmp4-core
cargo test -p cheetah-fmp4-core
cargo clippy -p cheetah-fmp4-driver-tokio
cargo test -p cheetah-fmp4-driver-tokio
cargo clippy -p cheetah-fmp4-module --tests
cargo test -p cheetah-fmp4-module
cargo test -p cheetah-fmp4-property-tests
(cd crates/protocols/fmp4/fuzz && cargo +nightly fuzz build)
```
