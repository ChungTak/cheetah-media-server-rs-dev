# Phase 04 — 兼容性、互操作、故障与性能验证

- **状态**: 未开始
- **范围**: ZLMediaKit/ffmpeg/VLC 互操作、真实故障样例、fuzz/property tests、慢客户端、内存上限、文档同步
- **完成标准**: 所有用户要求的编码和传输模式有可复现测试或手动验收脚本，真实脏数据不会导致 panic、无限缓存或拖垮其它连接

---

## 4.1 ZLMediaKit 互操作样例

**样例来源**:

- 本项目生成 HTTP-TS，ZLMediaKit `TsPlayer` 拉取
- ZLMediaKit 生成 `.live.ts`，本项目 pull job 拉取
- 本项目 WS-TS，浏览器 WebSocket 客户端读取 binary frame
- ZLMediaKit WS-TS，本项目 WS pull 拉取

**验收场景**:

| 场景 | 命令/动作 | 通过标准 |
|------|-----------|----------|
| cheetah -> ZLM HTTP-TS | ZLM `TsPlayer` play cheetah URL | 首包后 play success |
| ZLM -> cheetah HTTP-TS pull | pull `http://zlm/app/stream.live.ts` | engine 出现 tracks + frames |
| cheetah -> browser WS-TS | JS WebSocket binary | 收到 binary frame 且 payload `0x47` |
| ZLM -> cheetah WS-TS pull | pull `ws://zlm/app/stream.live.ts` | demux 出 frames |

**测试资产**:

- 新增 `crates/protocols/ts/module/tests/fixtures/README.md`
- 记录每个 fixture 的来源、codec、track 数、是否含 B 帧、是否含非标准 stream_type

---

## 4.2 编码矩阵

**目标编码**:

- H264
- H265
- AAC
- G711A
- G711U
- OPUS
- MP3
- VP8
- VP9
- AV1
- MP2

**每个编码至少验证**:

1. mux 输出 188 对齐
2. PMT stream_type 正确
3. demux 输出 codec 正确
4. PTS/DTS 不倒退
5. HTTP-TS 播放不 panic
6. WS-TS 播放 binary frame 正确

**多轨道组合**:

- H264 + AAC
- H265 + G711A + G711U
- H264 + AAC + OPUS
- 双视频 H264/H265 + 双音频 AAC/MP3
- audio-only AAC
- audio-only G711

---

## 4.3 故障样例

**必须覆盖的故障输入**:

| 故障 | 期望行为 |
|------|----------|
| 前导垃圾 | `SyncLoss` diagnostic 后重同步 |
| TS packet sync byte 损坏 | 丢弃坏包并继续 |
| PAT CRC 错误 strict=false | 诊断并继续 |
| PAT CRC 错误 strict=true | 拒绝该 PAT |
| PMT 指向未知 stream_type | 诊断并跳过该 track |
| continuity counter gap | 诊断，下一 PUSI 重新同步 PES |
| PES 超过重组上限 | 清空该 PID buffer |
| adaptation field 越界 | 丢包，不 panic |
| WebSocket unmasked client frame | 协议错误关闭 |
| WebSocket payload 超上限 | 关闭连接 |
| HTTP chunked 截断 | pull 返回错误并重试 |
| 空 body EOF | pull 返回失败 |

**测试形式**:

- codec 层单元测试覆盖 TS 字节故障
- driver 层集成测试覆盖 HTTP/WS 故障
- module 层端到端测试覆盖 pull retry 和 lease release

---

## 4.4 Fuzz / Property Tests

**codec fuzz 目标**:

- `MpegTsDemuxer::push()` 任意 bytes 不 panic
- `MpegTsDemuxer::push()` 内存不超过配置上限
- `MpegTsMuxer` 输出总是 188 对齐
- PAT/PMT section length 与 CRC property

**建议 crate**:

- `crates/protocols/ts/testing/property-tests`
- fuzz harness 放在 `crates/protocols/ts/fuzz/`，默认不加入根 workspace

**property 示例**:

- 任意 frame payload mux 后输出长度 `% 188 == 0`
- 任意切片方式喂给 demux，与一次性喂入得到相同 TrackFound/Frame 数
- continuity counter 对每 PID 单调 wrap

---

## 4.5 性能与背压

**ZLMediaKit 参考**:

- `TSMediaSource` ring reader detach
- `HttpSession::setSocketFlags()` 直播场景优化发送

**本项目验收**:

- 100 个 HTTP-TS 播放者同时播放同一流
- 100 个 WS-TS 播放者同时播放同一流
- 1 个慢客户端不会影响其它 99 个客户端
- 单连接写队列达到上限后关闭该连接
- pull job 断线重连不会泄漏 publisher lease
- `max_reassembly_bytes` 限制有效
- `websocket_max_frame_bytes` 限制有效

**指标记录**:

- 每连接发送字节数
- 当前 TS 播放连接数
- pull job 状态和重试次数
- demux diagnostic 计数
- 慢客户端关闭计数

---

## 4.6 文档同步

实现完成后同步：

- `dev-docs/SystemArchitecture.md`
- TS module README 或配置示例
- `apps/cheetah-server` feature 说明
- 互操作验收命令文档

**最低文档内容**:

- HTTP-TS URL: `http://host:port/{app}/{stream}.ts`
- ZLM 兼容 URL: `http://host:port/{app}/{stream}.live.ts`
- WS-TS URL: `ws://host:port/{app}/{stream}.ts`
- HTTPS/WSS TLS 配置示例
- pull job 配置示例
- 支持编码列表和已知播放器限制

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

手动互操作：

```bash
ffprobe -hide_banner http://127.0.0.1:8082/live/test.ts
ffplay -fflags nobuffer http://127.0.0.1:8082/live/test.ts
ffprobe -hide_banner http://127.0.0.1:8082/live/test.live.ts
```
