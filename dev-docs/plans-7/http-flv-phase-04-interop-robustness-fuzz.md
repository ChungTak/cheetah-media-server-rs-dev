# Phase 04: 互操作、鲁棒性与 Fuzz 收口

- 状态：计划中
- 范围：建立 HTTP-FLV/WS-FLV 真实互操作样例、端到端矩阵、PBT/fuzz 传输扰动、文档同步和最终检查。
- 完成标准：标准 HTTP-FLV/WS-FLV 播放和 pull job 有稳定回归；非标准、截断、乱序、chunked、WebSocket fault 输入只做 bounded robustness 并保持 module/engine 健康。

## 真实样例策略

不把大型原始 pcap 或完整媒体文件直接放进常规测试。首批使用两类可提交 fixture：

1. 小型 `.flvstream`：保存 FLV header + 若干完整 tag，来源可以是现有 RTMP fixture 经共享 adapter 转换，也可以从本地 SimpleMediaServer HTTP-FLV 抓包抽取。
2. 小型 HTTP/WS transport fixture：保存 HTTP body 分片、chunked body 分片或 WebSocket binary message 边界，用于 driver/core 传输扰动测试。

建议目录：

```text
crates/protocols/http-flv/testing/property-tests/tests/testdata/http-flv/
  README.md
  manifest.tsv
  standard/h264_aac.flvstream
  standard/h265_aac.flvstream
  standard/audio_only.flvstream
  probes/av1_probe.flvstream
  probes/vp9_probe.flvstream
```

`.flvstream` 格式可以直接是标准 FLV bytes，不再包一层自定义 record；transport fixture 才保存 record/message 边界。

## Manifest 字段

`manifest.tsv` 字段固定为：

```text
case	source	media_sig	role	fixture	expect_header	expect_metadata	expect_video_min	expect_audio_min	notes
```

字段规则：

- `role` 只允许 `standard_play`、`standard_pull`、`compat_probe`、`transport_fault_seed`。
- 标准样例必须能解析 FLV header，并至少满足对应音视频 tag 数。
- probe 样例只要求 demux bounded、不 panic、不 OOM。
- `fixture` 必须是 testdata 根目录下相对路径。

## PBT / Fuzz 输入视图

统一覆盖：

- `single_buffer`：完整 FLV bytes 一次输入。
- `original_records`：按 HTTP body / WS message 原始边界输入。
- `one_byte_chunks`：逐字节输入。
- `coalesced_n`：每 N 个 record 合并。
- `prefix_truncated`：前缀截断。
- `suffix_truncated_tag`：最后 tag 半截断。
- `duplicate_record`：重复 record。
- `swap_adjacent`：相邻 record 乱序。
- `drop_every_nth`：每 N 个 record 丢弃。
- `bad_previous_tag_size`：修改 previous tag size。
- `oversize_tag_header`：声明超大 payload，但输入受上界限制。
- `chunked_split_every_byte`：HTTP chunked 每字节一个 chunk。
- `ws_fragmented_binary`：WebSocket binary 分片。

断言边界：

- 未扰动 standard 样例做强断言。
- 一旦扰动、截断、乱序、重复或 oversize，只要求 bounded processing。
- fuzz target 不断言成功播放，只断言无 panic、无 OOM、无超时。

## 端到端矩阵

标准矩阵：

| 输入 | 输出 | 断言 |
| --- | --- | --- |
| RTMP publish H264/AAC | HTTP-FLV play | FLV header、metadata、video/audio tag、timestamp 单调 |
| RTMP publish H264/AAC | WS-FLV play | binary FLV bytes 可 demux，video/audio tag 存在 |
| RTSP publish H264/AAC | HTTP-FLV play | 跨协议 `AVFrame + TrackInfo` 到 FLV 输出正常 |
| HTTP-FLV pull H264/AAC | RTMP play | pull job 写入 engine 后 RTMP client 收到 media |
| WS-FLV pull H264/AAC | HTTP-FLV play | WS 输入到 engine 后 HTTP-FLV 输出可播放 |
| audio-only FLV pull | HTTP-FLV play | 不等待 keyframe，audio tag 存在 |

probe 矩阵：

- AV1/VP9/H266/VVC FLV enhanced 样例：允许播放器不成功，只要求 demux/adapter/module 健康。
- metadata 缺失样例：允许从 sequence header 和 media tag 推断 tracks。
- previous tag size mismatch 样例：记录 warn，不 panic。
- chunked/WS 截断样例：连接关闭可接受，module 必须可停止。

## 具体任务

### 4.1 建立互操作 fixture 和端到端测试

- [ ] 新增 `cheetah-http-flv-property-tests` 或在 module tests 中建立 `testdata/http-flv`。
- [ ] 生成 H264/AAC、H265/AAC、audio-only 标准 `.flvstream`。
- [ ] 生成 AV1/VP9/H266/VVC probe `.flvstream`。
- [ ] 增加 manifest 校验测试，确保 fixture 路径安全、大小有界、FLV header/tag 可解析。
- [ ] 增加端到端测试覆盖 RTMP/RTSP publish 到 HTTP-FLV/WS-FLV play。

### 4.2 建立 PBT/fuzz 传输扰动覆盖

- [ ] 新增 PBT helper 构造 FLV bytes 和 HTTP/WS transport fault views。
- [ ] 新增 core/property 测试：标准样例强断言，probe/fault 样例 bounded robustness。
- [ ] 新增 fuzz target `fuzz_flv_demux`，直接 fuzz `cheetah-codec::flv::FlvDemuxer`。
- [ ] 新增 fuzz target `fuzz_http_flv_transport`，fuzz HTTP chunked/body 分片到 core/driver parser。
- [ ] 新增 fuzz target `fuzz_ws_flv_frames`，fuzz WebSocket binary message 边界和 FLV demux。

### 4.3 文档、feature、CI 和 smoke 收口

- [ ] 同步 `SystemArchitecture.md`，说明 HTTP-FLV 三段式、独立 driver 和共享 adapter。
- [ ] 更新相关 README 或配置示例，加入 `http_flv.listen`、play URL、pull job 示例。
- [ ] 更新 `apps/cheetah-server` feature 说明，确认 `http-flv` 是否默认启用；首版按可选 feature 处理。
- [ ] 增加 dev script smoke：启动 RTMP + HTTP-FLV，推一条标准流，HTTP GET 拉取前几个 tag，WS 拉取前几个 binary message。
- [ ] 运行 workspace 相关检查和 fuzz smoke。

## 完成后检查

```bash
cargo fmt
cargo test -p cheetah-codec flv
cargo test -p cheetah-rtmp-core flv
cargo test -p cheetah-http-flv-core
cargo test -p cheetah-http-flv-driver-tokio
cargo test -p cheetah-http-flv-module
cargo test -p cheetah-http-flv-property-tests
cargo clippy -p cheetah-http-flv-core
cargo clippy -p cheetah-http-flv-driver-tokio
cargo clippy -p cheetah-http-flv-module
```

fuzz smoke：

```bash
cd crates/protocols/http-flv/fuzz
cargo +nightly fuzz build
cargo +nightly fuzz run fuzz_flv_demux -- -runs=128
cargo +nightly fuzz run fuzz_http_flv_transport -- -runs=128
cargo +nightly fuzz run fuzz_ws_flv_frames -- -runs=128
```
