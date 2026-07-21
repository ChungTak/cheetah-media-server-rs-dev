# 29 · GB28181 媒体能力完善计划 — 版本与能力清单 (AUD-01)

> 固定当前 media/signaling/reference 代码基线，建立能力清单，供后续任务追溯。

## 1. 代码基线

| 组件 | 位置/仓库 | 当前 revision | 日期 | 备注 |
| --- | --- | --- | --- | --- |
| Cheetah media server | `ChungTak/cheetah-media-server-rs-dev` | `1f3fff8877b470df635a979ecb7fac31faecefa0` | 2026-07-21 | `add dev-docs/plans-29-gb28181-impove` |
| 905 signaling control plane plan | `dev-docs/905_signaling_control_plane_plan` | 同 Cheetah media commit | 2026-07-21 | 审计基线，见 `512e4a5a4650231167c0eba04ff5a64e6892e459` |
| cheetah-signaling | 外部仓库 | 未挂载 | - | 当前构建环境未包含；P4 任务依赖其固定 tag/revision |
| ABLMediaServer | `/dataset/datavol/workspace/media_server/ABLMediaServer-src-2026-07-02/ABLMediaServer` | 未挂载 | - | 仅路径引用，当前环境不存在 |
| ZLMediaKit | `vendor-ref/ZLMediaKit` | 未挂载 | - | 当前环境不存在 |
| simple-media-server | `vendor-ref/simple-media-server` | 未挂载 | - | 当前环境不存在 |

## 2. 相关 crate 清单

| crate | 路径 | 当前职责 |
| --- | --- | --- |
| `cheetah-codec` | `crates/foundation/cheetah-codec` | 共享媒体内核；AVFrame/TrackInfo、时间戳、参数集、容器解析 |
| `cheetah-media-api` | `crates/sdk/cheetah-media-api` | runtime-neutral typed media 契约 |
| `cheetah-media-control-plane` | `crates/system/cheetah-media-control-plane` | 控制面状态、SQLite、幂等、容量算法 |
| `cheetah-media-grpc-adapter` | `crates/system/cheetah-media-grpc-adapter` | gRPC adapter、health/reflection |
| `cheetah-gb28181-core` | `crates/protocols/gb28181/core` | GB28181 Sans-I/O 状态机 |
| `cheetah-gb28181-driver-tokio` | `crates/protocols/gb28181/driver-tokio` | Tokio 网络/timer/任务驱动 |
| `cheetah-gb28181-module` | `crates/protocols/gb28181/module` | 引擎接入、资源分配、业务编排 |
| `cheetah-gb28181-property-tests` | `crates/protocols/gb28181/testing/property-tests` | 属性测试 |
| `cheetah-rtp-core` | `crates/protocols/rtp/core` | RTP/RTCP Sans-I/O 状态机 |
| `cheetah-rtp-driver-tokio` | `crates/protocols/rtp/driver-tokio` | RTP/RTCP 驱动 |
| `cheetah-rtp-module` | `crates/protocols/rtp/module` | RTP 引擎接入 |
| `cheetah-rtp-property-tests` | `crates/protocols/rtp/testing/property-tests` | RTP 属性测试 |

## 3. 当前能力清单

| 能力 | 状态 | 说明 |
| --- | --- | --- |
| `core + driver + module` 三段式 | 已具备 | GB/RTP 已按 Sans-I/O core、tokio driver、module 分层 |
| UDP/TCP active/passive | 已具备 | 含 RFC4571 两字节、`$` 四字节 framing |
| PS/TS/ES/raw payload | 已具备 | 基础 PSM 映射、重组上限 |
| H.264/H.265/AAC/G.711 | 已具备 | 基础解析路径 |
| 基础 RTCP | 已具备 | SR/RR/SDES/BYE 部分支持 |
| SSRC stream fallback | 已具备 | 单端口多 SSRC 回退 |
| 对讲 | 已具备 | 基础 voice talk 路径 |
| 端口池/有界队列 | 已具备 | 端口分配与队列限制 |
| Ehome2/JTT1078 | 部分 | 类型存在，主路径偏 v2013；Ehome5 未验证 |

## 4. 外部依赖缺失

- `cheetah-signaling` 仓库未挂载，无法获取当前 revision；P4 signaling 任务需外部固定 tag。
- 参考项目 `ABLMediaServer`、`ZLMediaKit`、`simple-media-server` 未挂载，兼容规则提取需在获取仓库后进行。

## 5. 后续任务关联

- 本清单作为 `01_audited_baseline_and_gap_register.md` 的审计基线补充。
- AUD-02 将基于本清单，在 `01_audited_baseline_and_gap_register.md` 中标记各 905/29 任务状态。
