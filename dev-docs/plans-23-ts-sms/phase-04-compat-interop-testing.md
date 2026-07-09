# Phase 04 — 兼容性、互操作与生产化验证

- **状态**: 规划中
- **范围**: 建立 SMS/ffmpeg/VLC 互操作矩阵、真实故障样例、fuzz/property tests、性能背压验证和文档同步
- **完成标准**: TS module 通过标准样例、SMS 样例、故障样例和端到端播放/拉流测试，并具备可诊断的生产运行指标

---

## 4.1 Fixture 与 Corpus

新增目录：

```text
crates/protocols/ts/module/tests/testdata/ts/
  README.md
  manifests/
  captures/
  faults/
```

样例来源：

- SMS `TsMuxer` 输出样例
- ffmpeg 生成的 H264/AAC、H265/AAC、audio-only TS
- 手工构造 PAT/PMT version change
- continuity counter 缺包样例
- 前导垃圾/错位 sync 样例
- PES length 0 样例
- 非标准 G711/Opus/VP8/VP9/AV1 stream_type 样例

fixture policy：

- 标准样例做强行为断言
- 真实设备/SMS 样例做兼容断言
- fault 样例只要求 bounded、无 panic、可诊断

---

## 4.2 互操作矩阵

播放验证：

| Source | Egress | Client | 目标 |
|--------|--------|--------|------|
| RTMP H264/AAC | HTTP-TS | ffplay | 正常播放 |
| RTSP H265/AAC | HTTP-TS | ffplay/VLC | 正常播放或明确 codec 限制 |
| RTMP audio-only AAC | HTTP-TS | ffplay | 正常播放 |
| RTSP G711 | HTTP-TS | ffplay/VLC | 可解封装，播放能力按客户端 |
| local stream | WS-TS | 测试客户端 | binary TS packet 对齐 |
| local stream | HTTPS/WSS-TS | 测试客户端 | TLS 正常 |

拉流验证：

| Remote | Ingest | Egress | 目标 |
|--------|--------|--------|------|
| ffmpeg HTTP-TS | TS pull | engine snapshot | tracks + frames 正确 |
| SMS HTTP-TS | TS pull | RTMP/RTSP/HLS | 跨协议可用 |
| WS-TS test source | TS pull | engine snapshot | binary 输入可用 |
| chunked HTTP-TS | TS pull | engine snapshot | chunked 可用 |

编码覆盖：

- H264
- H265
- AAC
- G711A / G711U
- Opus
- MP2
- MP3
- VP8
- VP9
- AV1

VP8/VP9/AV1 in TS 是非标准/弱播放器支持路径，测试重点是 mux/demux 稳定和 track/frame 识别。

---

## 4.3 Robustness 测试

输入故障：

- TS packet 前导垃圾
- 单字节/随机 chunk 切片
- sync byte 丢失
- PAT 缺失
- PMT 缺失
- CRC 错误
- continuity counter 跳变
- adaptation field 长度越界
- PES header 不完整
- PTS/DTS marker bit 错误
- oversized PES
- unknown PID / unknown stream_type
- PMT version change

期望：

- 不 panic
- buffer 不无界增长
- 能恢复则恢复
- 不能恢复则关闭单连接或重试单 pull job
- diagnostic 包含 PID、stream_type、connection/job、stream_key

---

## 4.4 Fuzz / Property-Based Testing

Property tests：

- TS muxer 输出总是 188 字节对齐
- 任意 chunk 切分 demux 结果一致
- 多轨 PID 唯一且不落入保留 PID
- PTS/DTS unwrap 单调性满足 frame ordering
- tolerant CRC 模式不会拒绝其他合法 PID

Fuzz targets：

- `fuzz_mpeg_ts_demux`
- `fuzz_mpeg_ts_pat_pmt`
- `fuzz_mpeg_ts_pes`
- `fuzz_ts_http_request`
- `fuzz_ts_ws_frames`

运行：

```bash
cargo test -p cheetah-ts-property-tests
(cd crates/protocols/ts/fuzz && cargo +nightly fuzz build)
```

---

## 4.5 性能与背压验证

验证项：

- 单连接持续输出无明显增长内存
- 100 个 HTTP-TS 播放连接下慢客户端不拖累快客户端
- WS binary message 聚合策略不造成高延迟
- pull job 断线重试不泄漏 publisher lease
- `max_reassembly_bytes` 达上限后可关闭连接/重试
- 多轨道 PMT 和 packetization 不产生过量小分配

建议指标：

- active TS sessions
- bytes sent / received
- mux packets / sec
- demux packets / sec
- sync loss count
- continuity gap count
- crc error count
- dropped unsupported frames
- slow client close count
- pull reconnect count

---

## 4.6 文档同步

需要更新：

- `SystemArchitecture.md`
  - TS crate 映射
  - TS capability snapshot
  - CI/check baseline
- `dev-docs/SystemArchitecture.md`
  - TS 属于协议三段式
  - MPEG-TS 容器能力归属 `cheetah-codec`
- README / 示例配置
  - TS module feature
  - HTTP/WS 播放 URL
  - pull_jobs 配置样例

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo clippy -p cheetah-ts-core
cargo test -p cheetah-ts-core
cargo clippy -p cheetah-ts-driver-tokio
cargo test -p cheetah-ts-driver-tokio
cargo clippy -p cheetah-ts-module --tests
cargo test -p cheetah-ts-module
cargo test -p cheetah-ts-property-tests
(cd crates/protocols/ts/fuzz && cargo +nightly fuzz build)
```
