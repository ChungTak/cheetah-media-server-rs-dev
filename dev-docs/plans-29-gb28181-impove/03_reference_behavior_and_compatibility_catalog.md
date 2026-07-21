# 03 · 参考行为与兼容目录

## 1. 使用规则

参考项目用于提取 wire behavior、异常输入与工程兼容策略，不复制其全局状态、无界缓存、不安全
内存管理、跨层耦合或许可证不明的大段实现。每条兼容规则必须有最小 fixture、来源说明、配置开关
和规范化结果。

## 2. 行为对照

| 能力 | ABLMediaServer | ZLMediaKit | simple-media-server | Cheetah 决策 |
| --- | --- | --- | --- | --- |
| SIP 行结束符/重复头 | 宽容 CRLF/LF/CR 和重复参数 | 设备实战兼容 | 完整 GB SIP/XML 业务 | legacy parser 有界兼容；生产业务归 signaling |
| TCP RTP framing | 2-byte/`$` 4-byte 自动识别 | 2/4-byte + Ehome2 | active/passive 收发 | per-connection sticky detect + bounded resync |
| TCP 错帧恢复 | SSRC/PS 特征恢复 | 两处匹配 SSRC + PS system header | 多种数据源兼容 | 采用有界双证据恢复，禁止无限扫描 |
| PS/ES/PT | PS、AAC、H264/H265、G711 | configurable PT + PS/TS sniff | PS/RTP/JTT/Ehome | 有界 sniff，置信后锁定并上报 provenance |
| RTP/RTCP | TCP RTP/RTCP、时间戳 | sort/loss/RR timeout/sender | 收发与对讲 | 补齐 reorder/loss/SR/RR/SDES/BYE |
| stream identity | SSRC 路径 | SSRC fallback、single-port mux | device/channel/session 映射 | 外部显式 binding 优先，SSRC fallback 仅 compat |
| publish auth cache | 无统一控制面语义 | 鉴权前短暂缓存 | hook/API 编排 | 只在 admission 后有界缓存，不复制先分配行为 |
| JT/T 1078 | 2013/2019 ingress/egress | 非主能力 | 2013/2019 媒体路径 | 完成版本自动识别和双向 fixture |
| Ehome | Ehome2 兼容 | Ehome2 256-byte prefix | Ehome2/Ehome5 目录 | Ehome2 验证；Ehome5 未有 fixture 则 Unsupported |
| 对讲 | raw/PS、常用音频 PT | RtpSender/voice talk | SIP Broadcast + media | signaling 发起，media 执行 typed talk session |

## 3. 兼容 Profile

| Profile | 默认 | 行为 |
| --- | --- | --- |
| `strict` | 否 | 严格长度、明确 PT/容器、禁止 SSRC 推导和自动 rebind |
| `gb28181_common` | 是 | 标准 GB28181 + 已验证的常见设备宽容项 |
| `zlm` | 否 | 2/4-byte、SSRC fallback、PT/PS/TS 探测、有限 resync |
| `sms` | 否 | SMS 风格 REST aliases、Subject/y/设备媒体参数规范化 |
| `abl` | 否 | ABL framing/PT/JTT 行为，但使用 Cheetah 有界安全实现 |
| `hikvision_ehome` | 否 | 仅启用已验证 Ehome2 framing；Ehome5 单独 capability gate |
| `jtt1078` | 否 | SIM/channel、2013/2019 header、fragment/timestamp 规则 |

Profile 只能改变入口兼容与 wire 导出，不得绕过 admission、tenant、fencing、单发布者、资源上限
或错误报告。一次 session 固定 resolved profile；运行时更改只影响新 generation。

## 4. Compat rule 登记字段

```text
rule_id / profile / reference_project / reference_revision
wire_sample / trigger / normalized_result
bounds / security impact / metric label
unit/property/fuzz/interop test
enablement / rollback / owner
```

规则命名使用 `compat::<profile>::<behavior>`，指标仅暴露低基数 rule ID，不记录设备凭据、原始
报文或完整 device ID。

## 5. 明确不采用

- 不采用无界 TCP/PS 扫描、固定大数组、裸指针生命周期或在热路径全局互斥。
- 不因未知 PT 就永久逐包 sniff；探测达到上限仍不确定时终止 session 并返回 typed error。
- 不接受任意服务器路径、URL、命令行或未经授权的外部资源。
- 不用厂商 profile 将 Deny 改为 Allow，不用缓存伪造鉴权成功。
- 不因参考项目目录或枚举存在就声明 Ehome5、硬件编码等能力可用。
