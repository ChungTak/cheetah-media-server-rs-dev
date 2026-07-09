# Phase 05 — 双向语音、Ehome 兼容与测试闭环

- **状态**: 已完成
- **范围**: 补齐双向语音对讲、Ehome 兼容、fixture、互操作测试、property tests 和 fuzz tests
- **完成标准**: 国标语音对讲可用，Ehome 基础兼容可用，RTP/GB28181 通过 ZLMediaKit、ffmpeg、真实设备样例和故障样例验证

---

## 5.1 双向语音对讲

能力目标：

- 本地音频流可通过 GB28181 会话推送到设备
- 设备上行音频可发布为本地流
- 支持 `G711A`、`G711U`、`AAC`、`Opus`，其中 G711 为主兼容路径

要求：

- 对讲会话独立于主视频会话
- 可只走单音轨
- payload type、sample rate、channel 和 packet duration 可显式配置
- 对讲失败不影响主视频拉流/推流

---

## 5.2 Ehome 兼容

目标：

- 引入 `ehome` payload mode
- 支持私有头识别和剥离
- 兼容 `vendor-ref/ZLMediaKit/src/Rtp/RtpSplitter.cpp` 的基础形态

要求：

- 兼容私有头、2-byte/4-byte framing 混合场景
- 兼容厂商附加头、非标准 PS/ES 组合、timestamp quirks
- compat 逻辑集中管理

---

## 5.3 Fixture 与互操作矩阵

样例来源：

- ZLMediaKit RTP/GB28181 输出样例
- ffmpeg RTP-PS、RTP-TS、raw RTP 样例
- 真实 GB28181 设备或录包样例
- Ehome 样例
- 半包、粘包、乱序、丢包、错误 SSRC、错误 source address 故障样例

互操作矩阵：

| Source | Ingest | Egress | 目标 |
|--------|--------|--------|------|
| GB28181 device | RTP/GB ingest | RTSP | 可播放 |
| GB28181 device | RTP/GB ingest | RTMP/HLS | 可播放 |
| local RTSP stream | RTP push client | ZLMediaKit | 可被 ZLM 接收 |
| local RTMP stream | GB send/create | device/platform | 可被远端接收 |
| Ehome sample | RTP ingest | engine snapshot | tracks + frames 正确 |
| talk session | local audio -> device | device 回放 | 音频链路可用 |

---

## 5.4 Property / Fuzz / Robustness

Property tests (已完成)：

- `[x]` RTP TCP chunk 切分结果一致
- `[x]` Ehome probe 在随机切分下保持稳定
- `[x]` timestamp wrap / disorder 结果稳定
- `[x]` mux 后 demux 保持 codec、track 和 frame 基本属性

Fuzz targets (已完成)：

- `[x]` `fuzz_rtp_header`
- `[x]` `fuzz_rtp_tcp_frame`
- `[x]` `fuzz_ehome_probe`
- `[x]` `fuzz_ps_demux`
- `[x]` `fuzz_sip_message`
- `[x]` `fuzz_gb28181_rest_json`

Robustness 验证 (已在单元/集成测试中覆盖)：

- `[x]` SIP 缺头、错误 digest、重复 CSeq
- `[x]` RTP over TCP 半包、粘包、超长 frame
- `[x]` PS/ES 非法长度、缺失参数集
- `[x]` source address 漂移
- `[x]` RTCP 丢失或错序

---

## 5.5 文档同步

需要更新：

- `SystemArchitecture.md`
- `AGENTS.md`
- README / 示例配置
- 相关 REST API 说明和 feature 开关文档

---

## 完成后检查

```bash
cargo fmt
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
cargo clippy -p cheetah-rtp-module --tests
cargo test -p cheetah-rtp-module
cargo clippy -p cheetah-gb28181-module --tests
cargo test -p cheetah-gb28181-module
cargo test -p cheetah-rtp-property-tests
cargo test -p cheetah-gb28181-property-tests
```
