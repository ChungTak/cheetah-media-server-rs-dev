# 05 · PS、RTP、RTCP 与时间线

## 1. 目标

把协议间重复的媒体规范化能力收敛到 `cheetah-codec`，保持 GB/RTP core 只负责协议状态推进，
module 不再私有维护时间戳、NALU、参数集或 PS 解析分支。

## 2. PS/PES/PSM

| ID | 实施 | 失败语义 | 完成证据 |
| --- | --- | --- | --- |
| CODEC-PS-01 | 将 pack/system/map/PES parser 拆成增量 Sans-I/O 模块，支持 start code 跨输入边界 | malformed/limit typed error | split-at-every-byte property test |
| CODEC-PS-02 | 支持 PSM 缺失、延迟、重复、版本变化和 track add/remove | 不确定时 bounded wait，不硬编码 H264 | real fixture matrix |
| CODEC-PS-03 | 识别 H264/H265/AAC/PCMA/PCMU；输出 TrackInfo change event | 超过探测预算返回 UnsupportedPayload | no-PSM fixtures |
| CODEC-PS-04 | 正确处理 PES 长度 0、跨 PES Access Unit、stuffing/private stream | 严格上限，禁止无限重组 | malformed/fuzz corpus |
| CODEC-PS-05 | 参数集缓存与关键帧前补发统一使用 codec API | 缺参数集可等待或报告 discontinuity | decoder validation |
| CODEC-PS-06 | 每 session 限制 tracks、pending bytes、PES/AU size 和 probe packets | LimitExceeded 并清理状态 | boundary tests |

入口 compat 可以容忍缺 system header、错误 PSM 顺序和厂商 stream type，但规范化后必须输出稳定
codec/timebase/track identity。兼容分支放 `compat` 子模块并记录 rule ID。

## 3. RTP payload 与时间线

| ID | 实施 | 完成证据 |
| --- | --- | --- |
| CODEC-RTP-01 | seq 扩展、重复检测、乱序窗口和 wrap 统一实现 | loss/reorder/wrap property tests |
| CODEC-RTP-02 | timestamp unwrap、timebase 转换、DTS/PTS/discontinuity 进入共享 timeline | long-run/wrap fixtures |
| CODEC-RTP-03 | PT resolver 按 SDP binding → profile mapping → bounded sniff 排序 | ambiguous/unknown PT tests |
| CODEC-RTP-04 | payload 类型达到置信阈值后锁定；中途变化生成 format-change 或终止 | spoof/PT flip tests |
| CODEC-RTP-05 | PS/TS/ES depacketizer 输出 `AVFrame + TrackInfo` | real decoder/player artifact |

禁止根据 PT=96 永久假定 PS。已协商 PT 优先；未协商时只在配置允许的 probe window 内使用
PS pack header、TS sync pattern、NALU/header 等多证据识别。

## 4. RTCP

| ID | 实施 | 完成证据 |
| --- | --- | --- |
| RTCP-01 | compound packet parser/encoder 支持 SR、RR、SDES、BYE | roundtrip/property tests |
| RTCP-02 | 从 RTP state 生成 fraction lost、cumulative loss、highest seq、jitter、LSR/DLSR | RFC vector tests |
| RTCP-03 | SR 映射 RTP timestamp 到 NTP/media timeline | injected clock tests |
| RTCP-04 | sender/receiver 定时器、timeout、BYE 与取消均为显式 action | FakeClock state tests |
| RTCP-05 | RTP/RTCP mux、分端口及 TCP framing 由 driver 适配 | network integration matrix |

NACK/PLI/XR 只有在明确的下游消费方和 fixture 存在时进入 capability；未实现时不得无声忽略
对端依赖的反馈，应通过 capability/metrics 标明 unsupported。

## 5. JT/T 1078 与 Ehome

- `Jtt1078FrameAssembler` 必须从 version/header 实际选择 2013/2019 parser，而不是始终走默认版本。
- packetizer 与 assembler 对称支持已声明版本、SIM/channel、data type、fragment、timestamp 和 codec。
- fragment queue 按 stream/channel 隔离并限制 frame count、bytes、age；缺片超时输出 discontinuity。
- Ehome2 256-byte prefix 解析、长度、session identity 与后续 RTP framing 使用真实抓包验证。
- Ehome5 单列 capability，只有 ingress/egress fixture、错误样例和互操作通过后才能从 Unsupported
  调整为 Experimental/Supported。

## 6. 模块拆分

优先拆分超过 800 行的 PS/RTP/TS/JTT 文件为 parser、packetizer、timeline、reorder、track map、
compat 和 limits 子模块。拆分提交只做机械迁移与等价测试，功能变更在后续独立提交完成。
