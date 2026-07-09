# Phase 04 — 兼容性、互操作与生产验证

- **状态**: 规划中
- **范围**: 建立 ZLM/ffmpeg/VLC 互操作矩阵、真实故障样例、fuzz/property tests、性能背压验证和文档同步。
- **完成标准**: fMP4 module 能在标准样例、ZLM 样例、故障样例和端到端播放/拉流测试中稳定运行。

## 4.1 Fixture 与 Corpus

新增测试样例目录：

```text
crates/protocols/fmp4/module/tests/testdata/fmp4/
  README.md
  manifests/
  captures/
  faults/
```

样例来源：

- ZLMediaKit HTTP-fMP4 输出。
- ZLMediaKit WS-fMP4 输出。
- ffmpeg 生成的 H264/AAC fMP4。
- ffmpeg 生成的 H265/AAC fMP4。
- audio-only AAC。
- G711/Opus/MP3/MP2。
- MJPEG/VP8/VP9/AV1。
- multi-track fMP4。
- repeated init segment。
- chunked HTTP body。
- WebSocket continuation binary。
- oversized box 和 malformed box。

策略：

- 标准样例做强断言。
- ZLM 样例做兼容断言。
- fault 样例只要求 bounded、无 panic、可诊断。

## 4.2 播放互操作矩阵

| Source | Egress | Client | 目标 |
|--------|--------|--------|------|
| RTMP H264/AAC | HTTP-fMP4 | ffplay | 可播放 |
| RTSP H265/AAC | HTTP-fMP4 | ffplay/VLC | 可播放或明确客户端限制 |
| audio-only AAC | HTTP-fMP4 | ffplay | 可播放 |
| RTSP G711 | HTTP-fMP4 | ffprobe | 可解封装 |
| local stream | WS-fMP4 | test client | 收到 binary init/fragment |
| local stream | HTTPS-fMP4 | test client | TLS 正常 |
| local stream | WSS-fMP4 | test client | TLS + WS 正常 |
| local stream | `.live.mp4` | ZLM-like client | 路由兼容 |

## 4.3 拉流互操作矩阵

| Remote | Ingest | Egress | 目标 |
|--------|--------|--------|------|
| ZLM HTTP-fMP4 | fMP4 pull | engine snapshot | tracks + frames 正确 |
| ZLM WS-fMP4 | fMP4 pull | engine snapshot | binary 输入正确 |
| ffmpeg HTTP-fMP4 | fMP4 pull | RTMP/RTSP/HLS | 跨协议可用 |
| chunked HTTP-fMP4 | fMP4 pull | engine snapshot | chunked 解码正确 |
| repeated init source | fMP4 pull | engine snapshot | track 更新正确 |

## 4.4 Robustness 测试

输入故障：

- box 前导垃圾。
- 单字节 chunk 切分。
- box size 小于 header。
- largesize 越界。
- `mdat` 缺失。
- `moof` 缺失。
- `traf` 缺少 `tfhd`。
- `traf` 缺少 `tfdt`。
- `trun` sample count 与 payload 不匹配。
- `trun.data_offset` 指向 `mdat` 外。
- H26x length prefix 越界。
- unknown sample entry。
- repeated init segment。
- track id 不存在。
- oversized box。
- oversized WebSocket message。

期望：

- 不 panic。
- buffer 不无界增长。
- 能恢复则恢复。
- 不能恢复则关闭单连接或重试单 pull job。
- diagnostic 包含 box type、track id、connection/job、stream key。

## 4.5 Property 与 Fuzz

Property tests：

- 任意 chunk 切分 demux 结果一致。
- 多轨 track id 唯一且稳定。
- mux/demux roundtrip 保持 track 和 frame 基本属性。
- `tfdt + trun` timestamp 展开保持合理单调。
- unknown box 不影响后续合法 box。

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

## 4.6 性能与背压

验证项：

- 单连接持续输出内存稳定。
- 100 个 HTTP-fMP4 播放连接下慢客户端不拖累快客户端。
- WS binary message 聚合不造成不可接受延迟。
- pull job 断线重试不泄漏 publisher lease。
- `max_box_bytes` 达上限后关闭单连接或重试单 job。
- 多轨 `moof/traf/trun` 生成不产生过量小分配。
- 每帧 fragment 与 1 秒 fragment 两种模式的 CPU/带宽开销可观测。

建议指标：

- active fMP4 sessions
- bytes sent / received
- mux fragments per second
- demux fragments per second
- init segment resend count
- box parse error count
- dropped unsupported frames
- slow client close count
- pull reconnect count
- max fragment bytes observed
- max box reassembly bytes observed

## 4.7 文档同步

需要更新：

- `SystemArchitecture.md`
- `dev-docs/SystemArchitecture.md`
- `README.md`
- `config.example.yaml`

文档必须说明：

- HTTP-fMP4 URL。
- WS-fMP4 URL。
- `.mp4` 与 `.live.mp4` 双兼容。
- HTTPS/WSS 配置。
- `pull_jobs` 配置。
- 弱播放器支持的 codec 限制。
- 推荐验证命令。

