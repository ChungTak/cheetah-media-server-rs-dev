# 09 · 对讲、回放与下载

## 1. 统一会话模型

对讲、回放和下载复用 04 的资源身份、admission、generation、idempotency 和 cleanup，不再维护
独立匿名 socket/map。业务命令由第三方控制系统决定，本项目只执行授权后的结构化媒体绑定。

## 2. Voice talk

| ID | 实施 | 完成证据 |
| --- | --- | --- |
| TALK-01 | typed request 明确 codec/PT/clock/channels/packet duration/duplex | validation tests |
| TALK-02 | PCMA/PCMU 优先；AAC 仅在协商和 provider capability 均满足时开放 | decoder/device fixtures |
| TALK-03 | RTP packetizer timestamp/sequence/SSRC 连续，RTCP 生命周期完整 | packet vector tests |
| TALK-04 | bounded audio queue、late/drop policy、slow device isolation | backpressure test |
| TALK-05 | reuse socket/peer 必须匹配 tenant/device/source/generation/security binding | cross-session denial |
| TALK-06 | 外部 cancel/stop、timeout、send failure 统一释放 task/socket/lease | leak tests |

本项目不知道也不处理 Broadcast/MANSCDP。第三方调用 `open_talk` 后，只有 RTP talk session 进入
Active 才报告最终 applied outcome。

## 3. Playback 与 download

| ID | 实施 | 完成证据 |
| --- | --- | --- |
| PLAY-01 | request 显式携带 record source、start/end、SSRC 和目标 transport | contract tests |
| PLAY-02 | record reader 输出 AVFrame，经 codec 导出 PS/TS/ES 视图后 packetize | player/decoder artifact |
| PLAY-03 | playback timeline 从 0 或约定基点单调推进，同时保留 source time metadata | seek/wrap tests |
| PLAY-04 | pause/resume/seek/speed 仅在 provider 支持时注册 capability | Unsupported tests |
| PLAY-05 | download 有独立 rate/backpressure/timeout，不拖累 live dispatcher | concurrent load test |
| PLAY-06 | output 使用独立 StreamKey/session，不覆盖源流或绕过 publisher lease | lease tests |

## 4. 媒体参数与传输要求

- live、playback、download 使用显式 media purpose、时间范围和 SSRC；Subject/SDP 由第三方处理。
- TCP active/passive role 必须与已授权 endpoint 一致；协商反转或地址变化需要新 generation。
- payload/container 由双方协商和 capability 交集确定，不能以 REST route 名称隐式决定。
- 设备只接受非标准 PT、2/4-byte framing 或 SSRC 格式时，由 compat profile 处理并输出 rule metric。

## 5. 失败语义

- record 不存在、codec 不支持、协商冲突、目标连接失败分别返回 typed error。
- 部分目标成功时返回 per-target outcome，不把失败目标伪装为成功。
- speed/seek 等未实现命令返回 `Unsupported`，不返回 200 后无动作。
- 外部 API response loss 通过 idempotency/get query 找回同一 session，不重复启动 reader/worker。
