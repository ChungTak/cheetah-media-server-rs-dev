# GB28181 媒体能力矩阵

> 按 `13_release_evidence_template.md` 第三章扩展，作为 P0 DOC-02 交付物。
> 每个单元格只能填写 `Supported`、`Experimental`、`Unsupported` 或 `N/A`，并链接对应 fixture/evidence。

## 1. 能力 × Profile 矩阵

| 能力 | strict | gb-common | zlm | sms | abl | ehome | jtt1078 | 证据/说明 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| UDP receive/send | Supported | Supported | Supported | Supported | Supported | N/A | Supported | `crates/protocols/gb28181/driver-tokio` |
| TCP active/passive | Supported | Supported | Supported | Supported | Supported | N/A | Supported | RFC 4571 2-byte / RTSP-style 4-byte auto-detect |
| 2-byte/4-byte framing | Supported | Supported | Supported | Supported | Supported | N/A | N/A | `cheetah-rtp-driver-tokio` framing |
| PS/TS/ES | Unsupported | Experimental | Experimental | Experimental | Experimental | N/A | N/A | PS 基础路径已存在，PSM 缺失/异常 fallback 仍需 fixture 验证 (GAP-PS-01) |
| H264/H265/AAC/G711 | Unsupported | Supported | Supported | Supported | Supported | N/A | Supported | `cheetah-codec` 基础解复用 |
| RTP/RTCP | Unsupported | Experimental | Experimental | Experimental | Experimental | N/A | Experimental | RTP 收发 Supported；RTCP SR/RR/SDES/BYE/timeout 待补齐 (GAP-RTP-01) |
| source binding/NAT rebind | Unsupported | Experimental | Experimental | Experimental | Experimental | N/A | Unsupported | SSRC fallback 仅作为 profile-specific compat，默认严格模式禁用 |
| live/playback/download | Unsupported | Supported (live) | Supported (live) | Supported (live) | Supported (live) | N/A | Unsupported | 回放/下载为 P3 PLAY 任务 |
| voice talk | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | N/A | Unsupported | P3 TALK 任务 |
| JTT 2013/2019 | N/A | N/A | N/A | N/A | N/A | N/A | Experimental (2013) | v2019 parser 存在，主 assembler/packetizer 路径仍偏 v2013 (GAP-JTT-01) |
| Ehome2/Ehome5 | N/A | N/A | N/A | N/A | N/A | Experimental (Ehome2) | N/A | Ehome2 路径有限，Ehome5 无 fixture，标记 Unsupported |

## 2. 语义约定

- `Supported`：已有生产级实现 + 回归测试/fixture，当前 main 可通过相关 crate 测试。
- `Experimental`：代码存在但缺少足够真实 fixture、互操作证据或边界覆盖，使用时会返回明确效果或 typed error。
- `Unsupported`：明确不实现或尚未实现的能力；调用方会收到 `Unsupported`/`Unavailable`，不会 fallback 到未声明行为。
- `N/A`：该 profile 不覆盖此项能力。

## 3. 与 Profile 的映射

| Profile | 默认 | 关键行为 |
| --- | --- | --- |
| `strict` | 否 | 严格长度、明确 PT/容器、禁止 SSRC 推导和自动 rebind |
| `gb28181_common` | 是 | 标准 GB28181 + 已验证的常见设备宽容项 |
| `zlm` | 否 | 2/4-byte、SSRC fallback、PT/PS/TS 探测、有限 resync |
| `sms` | 否 | SMS 风格 RTP/PS/PT/SSRC 媒体参数规范化 |
| `abl` | 否 | ABL framing/PT/JTT 行为，使用 Cheetah 有界安全实现 |
| `hikvision_ehome` | 否 | 仅启用已验证 Ehome2 framing；Ehome5 单独 capability gate |
| `jtt1078` | 否 | SIM/channel、2013/2019 header、fragment/timestamp 规则 |

## 4. 已知未验证能力

以下能力当前不得在生产中宣称可用，后续任务需补充 fixture：

- Ehome5 全部媒体路径。
- JTT 1078 v2019 双向 egress。
- RTCP 双向 SR/RR/SDES/BYE/timeout。
- PS 流 PSM 动态变更、PES zero/split/stuffing/private 路径。
- TCP 错帧无限扫描（已禁止，但 recovery fixture 不足）。

## 5. 更新规则

- 任何新增或修改 codec/RTP/compat 行为的 PR 必须同步更新本矩阵。
- `Experimental` 转 `Supported` 必须附带真实 device/wire fixture 和解码/播放证据。
- 能力降级（`Supported` → `Experimental`/`Unsupported`）必须在 PR 中说明并更新 release evidence。
