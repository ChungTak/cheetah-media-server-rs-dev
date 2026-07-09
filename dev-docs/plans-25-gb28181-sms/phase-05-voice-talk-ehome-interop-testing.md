# Phase 05 — 双向语音、Ehome 兼容与互操作验证

- **状态**: 已完成
- **范围**: 补齐 GB28181 双向语音对讲、Ehome RTP 兼容、真实设备/SMS/故障样例验证、性能与运维指标
- **完成标准**: 国标语音对讲可用，Ehome 基础兼容可用，RTP/GB28181 通过 SMS、ffmpeg、设备样例和故障样例验证

---

## 5.1 双向语音对讲

能力目标：

- 本地音频流可通过 GB28181 会话推送到设备
- 设备上行音频可发布为本地流
- 支持 `G711A`、`G711U`、`AAC`、`Opus`，其中 G711 为主兼容路径

设计要求：

- 对讲会话使用独立 `talk session id`
- 可与视频流分离，只走音频单轨
- payload type、sample rate、channel 数由设备能力或 REST 参数显式指定
- 失败不会影响主视频播放/拉流会话

---

## 5.2 Ehome 兼容

目标：

- 引入 `ehome` payload mode
- 先支持媒体面兼容，再逐步扩展控制面
- 兼容 `vendor-ref/simple-media-server/Src/Ehome2`、`Ehome5` 的基础 RTP 形态

落地要求：

- 增加 Ehome payload probe 和 framing 兼容
- 兼容厂商附加头、非标准 PS/ES 组合、时间戳 quirks
- 兼容模式必须集中实现，不能散落在 module 热路径

---

## 5.3 Fixture 与互操作矩阵

样例来源：

- SMS RTP/GB28181 输出样例
- ffmpeg RTP-PS、RTP-TS、raw RTP 样例
- 真实 GB28181 设备或录包样例
- Ehome 样例
- 半包、粘包、乱序、丢包、前导垃圾、错误 SSRC、错误 source address 故障样例

互操作矩阵：

| Source | Ingest | Egress | 目标 |
|--------|--------|--------|------|
| GB28181 device | RTP/GB ingest | RTSP | 可播放 |
| GB28181 device | RTP/GB ingest | RTMP/HLS | 可播放 |
| local RTSP stream | RTP push client | SMS | 可被 SMS 接收 |
| local RTMP stream | GB send/create | device/platform | 可被远端接收 |
| Ehome sample | RTP ingest | engine snapshot | tracks + frames 正确 |
| talk session | local audio -> device | device 回放 | 音频链路可用 |

---

## 5.4 Robustness 与观测性

故障验证：

- SIP 缺头、重复 CSeq、错误 branch、无效 digest
- RTP over TCP 半包、粘包、超长 frame
- PS/ES 非法长度、乱序 start code、缺失参数集
- UDP source address 漂移
- RTCP 丢失或错序
- 设备反复注册/注销/掉线重连

期望：

- 不 panic
- buffer 不无界增长
- 连接或单会话失败不拖垮整个模块
- diagnostic 包含 device id、stream key、session key、ssrc、payload mode

建议指标：

- active rtp sessions
- active gb28181 dialogs
- talk sessions
- rtp bytes in/out
- rtcp packets in/out
- ps demux error count
- sip auth failure count
- device online/offline transitions
- slow client close count
- source address rebind count

---

## 5.5 文档同步

需要更新：

- `SystemArchitecture.md`
  - RTP 与 GB28181 crate 映射
  - 媒体模型与控制面边界
- `AGENTS.md`
  - 若 crate 命名、依赖方向或 runtime 抽象出现新增约束，需要同步
- README / 示例配置
  - `rtp`、`gb28181` feature
  - REST API 示例
  - 主动/被动拉流、语音对讲配置样例

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
