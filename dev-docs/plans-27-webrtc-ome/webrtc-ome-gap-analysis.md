# WebRTC OME 差距分析

- **状态**: 已完成
- **范围**: 设计文档

## 已有能力

本地 WebRTC 已具备以下基础，后续应复用而不是重写：

- `core + driver + module` 三段式 crate。
- WHIP/WHEP、SMS/ZLM/ABL 兼容入口与 `compat` 层。
- UDP/TCP listener、route directory、多 shard、candidate 统计与迁移基础设施。
- BWE、REMB、RTCP、simulcast、bootstrap、P2P、DataChannel、互操作脚手架。
- `cheetah-codec` 已具备参数集补发、时间戳归一化、WebRTC ingress/egress contract。

## 主要缺口

| 缺口 | OME 依据 | 本地风险 | 优先级 |
| --- | --- | --- | --- |
| 自定义 WebSocket 信令 | `rtc_signalling_server.*`、文档 `request-offer` 流程 | 只能走 WHIP/WHEP，无法对齐 OvenPlayer/OME 自定义客户端 | P0 |
| `?direction=` 兼容 | OME WebSocket/WHIP URL 模式 | publish/play 路径与现有 URL 不兼容 | P0 |
| `?transport=` 与 `DefaultTransport` | OME `udp/tcp/relay/udptcp/all` 策略 | 候选输出与客户端预期不一致 | P0 |
| relay-only `iceServers` 与 `TcpRelayForce` | OME relay 模式 | 防火墙环境无法对齐 OME 行为 | P1 |
| playlist/rendition/WebRtcAutoAbr | `rtc_session.cpp`、`rtc_stream.cpp` | 现有播放模型缺少 OME 风格 ABR 语义 | P1 |
| JitterBuffer/PlayoutDelay | OME publisher config | 弱网或播放器缓冲提示行为不一致 | P1 |
| RtcpBasedTimestamp/CompositionTime | `webrtc_stream.cpp` | H264/H265 B 帧、回放、A/V 对齐仍有兼容缺口 | P1 |
| RED/ULPFEC 配置与 SDP | `rtc_stream.cpp`、媒体描述构造 | 弱网恢复能力与 SDP 协商不完整 | P1 |
| 周期性 FIR | provider config `FIRInterval` | 长 GOP 或上游不主动出关键帧时恢复慢 | P2 |
| OME fixtures 与测试工具 | `OvenRtcTester.go`、文档 SDP/URL 样例 | 缺少对 OME 真实行为的回归保护 | P2 |

## 非标准兼容清单

后续实现应显式命名为 OME compat，避免隐藏在主路径条件分支中：

- 接受 `ws[s]://host/app/stream?direction=send` 自定义信令 publish。
- 接受 `http[s]://host/app/stream?direction=whip` 的 OME URL 风格别名。
- 支持 `?transport=tcp` 表示 direct TCP ICE，而不是 TURN relay。
- 支持 `?transport=relay` / `TcpRelayForce=true` 时只下发 relay 信息和 relay-only 兼容响应。
- 在播放侧暴露 playlist/rendition 语义，后续可映射到本地 simulcast 或多流选择。
- 在 offer/answer 与 extmap 处理中兼容 `CompositionTime`、`playout-delay`、`video-timing`、`framemarking` 等扩展。

## 风险与约束

- OME 的 ABR playlist 是播放产品模型，不应直接污染 core；应由 module 用引擎与 bridge 能力承接。
- relay 行为如果没有真正 TURN server，必须在文档和实现里区分“兼容信令输出”和“完整 relay 数据面”。
- RED/ULPFEC 可能超出 `str0m` 当前可直接利用的范围，需要先以 SDP/观测/fixture 为主，小步推进。
- `RtcpBasedTimestamp=true` 涉及时间模型，必须优先落在 `cheetah-codec` 或 bridge timestamp policy，不得在 module 四处修补。

## 完成定义

本计划完成时应达到：

- OME URL、信令和 `transport` 候选策略有明确 compat 路径。
- 播放侧 ABR/jitter/playout 行为有可实现的模块边界，而不是留在概念层。
- RED/RTX/ULPFEC/TWCC/REMB/FIR 的实现与测试优先级清晰。
- OME 文档样例、SDP fixtures、测试工具进入本地回归体系。
