# SRT 协议完善设计与渐进式开发计划（对标 ZLMediaKit）

- **状态**: 草案 / 待执行
- **目标**: 在现有 `cheetah-srt-core`、`cheetah-srt-driver-tokio`、`cheetah-srt-module` 基础上，对标 ZLMediaKit SRT 的真实工程行为，补齐 Stream ID 语义、Listener 推拉流、NACK/ARQ 观测、版本策略、鉴权参数与 FEC，使 OBS / FFmpeg / ffplay / VLC / libsrt 可按 ZLM 文档方式稳定互操作。
- **方法**: 协议状态机继续复用 Sans-I/O crate `shiguredo_srt`；行为参考 `vendor-ref/ZLMediaKit/srt/` 与 `vendor-ref/ZLMediaKit/src` 中的 TS / MediaSource / 鉴权实践；MPEG-TS demux/mux、时间戳归一化与 codec 识别回到 `cheetah-codec`。
- **完成标准**: ZLM 风格 streamid 推/拉可用；TS-only 硬约束生效；NACK/重传与弱网可观测可测；peer 版本 `<1.3.0` 可拒绝；FEC 可协商并可降级；单元 / 集成 / 互操作 / fuzz 通过。
- **基线**: 本计划是对 [plans-28-srt](../plans-28-srt/index.md) 已落地 SRT 主路径的 **ZLM 对齐增强**，不重写 crate 骨架。

---

## V1 完善范围

本轮是对现有 SRT 实现的协议与兼容完善，不重写项目总架构。

1. **Stream ID（ZLM Access Control）**: 解析 `#!::key=value,...`；`h`/`r`/`m` 特殊语义；其余 key 与 `m` 作为鉴权参数。
2. **推流 / 拉流判定**: `m=publish` 为推流，否则为拉流；**不存在 `m` 时默认拉流**（对齐 ZLM，可通过配置回退）。
3. **vhost / app / stream 映射**: `h` 为 vhost（缺省默认 vhost）；`r=app/stream` 映射本地 `StreamKey`。
4. **Listener**: 单端口 UDP listener 接受多 caller；握手后按 streamid 分流。
5. **TS-only**: 推流只接受 MPEG-TS over SRT；拉流只输出 MPEG-TS over SRT。
6. **NACK / ARQ**: 依赖并观测 `shiguredo_srt` 的 ACK/NAK/retransmit；配置 latency / buffer；弱网矩阵验收。
7. **版本策略**: 本端宣告 SRT `>=1.3.0`（建议 1.5.0）；拒绝过旧 peer 并输出诊断。
8. **FEC**: 实现 Packet Filter / FEC（**超越 ZLM**；ZLM `srt.md` 明确未实现），协商失败可降级 ARQ-only。
9. **Caller jobs**: 保持并完善 ingress / egress / relay 与重试语义。
10. **互操作**: OBS 推流、FFmpeg 推流、ffplay 拉流、VLC 偏好设置 streamid 拉流。
11. **运维**: metrics、握手拒绝原因、FEC/NACK 计数、运维文档。

本轮不做：

1. FileCC、Rendezvous、Group Membership（`shiguredo_srt` / 本阶段均非目标）。
2. 任意二进制 payload 跨协议转换；v1 仍只支持 MPEG-TS。
3. 在 module 中复制一套 TS demux/mux 或参数集缓存；统一走 `cheetah-codec`。
4. 绕过 `RuntimeApi` 在 SDK / engine / module 公共接口暴露 `tokio::*` 类型。
5. 复制 ZLM 的 C++ 自研握手 / NACK 状态机；协议状态仍由 `shiguredo_srt` 承担，除非库能力证明不足且无法升级。

---

## ZLMediaKit 关键参考

> **路径说明**: SRT 协议实现主体在 `vendor-ref/ZLMediaKit/srt/`，**不在** `src/`。`src/` 提供 TS 封装、MediaSource、HTTP/鉴权广播等共享能力。用户需求中的 “参考 `src`” 应理解为 **媒体桥与工程实践**；协议状态机与 streamid 业务以 `srt/` 为准。

| 领域 | ZLMediaKit 文件 | 重点行为 |
|------|-----------------|----------|
| 文档 / 用法 | `srt/srt.md`、`srt/srt_en.md` | streamid 语义、OBS/ffmpeg/ffplay/VLC、NACK/listener/TS-only；FEC 未实现 |
| Transport | `srt/SrtTransport.*` | 握手、ACK/NAK/ACKACK、keepalive、drop req、crypto、超时、统计 |
| 业务桥 | `srt/SrtTransportImp.*` | `parseStreamid`、publish/play 分支、TS decoder、TSMediaSource ring |
| NACK | `srt/NackContext.*` | 丢包表、RTT 周期再 NAK、seq 回绕 drop |
| 收发队列 | `srt/PacketQueue.*`、`PacketSendQueue.*` | latency、TLPKTDROP、重传缓存上界 |
| 握手扩展 | `srt/HSExt.*`、`Packet.*` | version/flags、Stream ID 编码、KM、reject reason |
| Caller | `srt/SrtCaller.*`、`SrtPlayer.*`、`SrtPusher.*` | caller 推/拉对称实现 |
| Session | `srt/SrtSession.*` | UDP session 与 transport 绑定 |
| TS 媒体 | `src/TS/*`、`src/Rtp/TSDecoder.*`、`src/Record/MPEG.*` | TS 源与 demux |
| 鉴权广播 | `src/Common/MediaSource.*` 等 | publish/play webhook 风格鉴权 |

协议规范：

- [SRT RFC draft (Sharabayko)](https://haivision.github.io/srt-rfc/draft-sharabayko-srt.html)
- `shiguredo_srt`：https://docs.rs/shiguredo_srt/ / https://github.com/shiguredo/srt-rs

---

## 与本地实现对比后的主要缺口

| 能力 | ZLM 参考 | 本地状态 | 计划处理 |
|------|----------|----------|----------|
| Listener 多连接 | `SrtTransportManager` + Session | ✅ `spawn_driver` UDP listener / connection map | Phase 02/03 加固 |
| NACK/重传 | `NackContext` + send queue | ⚠️ 库内 ACK/NAK/retransmit；已有 retransmit/loss stats；缺弱网正式矩阵与配置化观测 | Phase 02 |
| Stream ID `h/r/m` | `SrtTransportImp::parseStreamid` | ⚠️ 解析 `h/r/m/u/s/extras`；`h` 未参与流定位；`r` 未强制 `app/stream` | Phase 01 |
| 默认模式 | 无 `m` → **拉流** | ❌ 默认 `ingress.default_mode = "publish"` | Phase 01/03 |
| 鉴权参数 | 其它 key + `m` → webhook | ⚠️ 仅 `token`/`u` + 静态 token 表 | Phase 01/03 |
| TS-only 推/拉 | decoder_ts / TSMediaSource | ✅ MPEG-TS via `cheetah-codec`；路径经 engine 非 TS 直通 ring | Phase 03 文档化 + 硬校验 |
| 版本 >=1.3.0 | HS v5，`srt_version=1.5.0` | ⚠️ 库默认 `0x010500`；未显式拒绝过旧 peer | Phase 01/02 |
| FEC | **未实现** | ❌ 无 Packet Filter/FEC | Phase 04（超越 ZLM） |
| Caller jobs | Player/Pusher | ✅ ingress/egress/relay | Phase 03 对齐 streamid |
| OBS/ffmpeg/VLC 矩阵 | `srt.md` 用法 | ⚠️ 已有部分 FFmpeg 验证；缺 ZLM 默认 mode 与 VLC 场景 | Phase 05 |
| module 体量 | 分层文件 | ⚠️ `module.rs` ~1400 行 | 各 phase 拆分 |

---

## 标准与非标准兼容点

### 标准基线

- SRT handshake（HS version 5）、LiveCC、ACK/NAK/ACKACK、TSBPD、TLPKTDROP、AES-128/256 KM。
- Stream ID Access Control 语法：`#!::k1=v1,k2=v2,...`。
- 媒体：MPEG-TS over SRT；进入 engine 后统一 `AVFrame + TrackInfo`。
- 本端与 peer 版本策略：支持 / 要求 `>= 1.3.0`。

### ZLM / 真实落地兼容优先

- `#!::h=...,r=app/stream,m=publish|request|play` 语义对齐 ZLM。
- 无 `m` 默认拉流（配置可改回 publish 以兼容旧部署）。
- OBS：`srt://host:9000?streamid=#!::r=live/test,m=publish`。
- FFmpeg 推 mpegts；ffplay 拉；VLC 在偏好设置中写 streamid，URL 仅 `srt://host:9000`。
- 入口允许脏 streamid / 缺字段时返回明确拒绝，不 panic。
- FEC 为增强能力：对端不支持时降级，不阻断主路径。

---

## 风险与迁移

1. **默认 mode 变更**（publish → request）会改变「无 `m` 的 caller」行为。必须：
   - 配置项 `ingress.default_mode` 保留；
   - 默认值改为 `request` 时在 CHANGELOG / 配置注释中标注；
   - 允许运维设回 `publish` 兼容 `plans-28-srt` 部署。
2. **`r` 两段强制** 与当前「任意 stream key」宽松策略冲突；建议默认对齐 ZLM 两段，另开 `stream_id.allow_bare_key` 兼容 bare/`live/test` 已有解析。
3. **FEC** 依赖 `shiguredo_srt` 扩展或本仓库 filter 层，工作量大，放在 Phase 04，主路径不阻塞。
4. **Webhook 基建**：若 control 面尚未统一 webhook，Phase 01 先落 `auth_params` 结构与 module 钩子，HTTP 回调可接现有 control 或后续补齐。

---

## 计划文件清单

| 文件 | 状态 | 范围 |
|------|------|------|
| [srt-zlm-architecture.md](srt-zlm-architecture.md) | 草案 | 总体架构、crate 边界、数据流、streamid、配置、API、观测 |
| [srt-zlm-gap-analysis.md](srt-zlm-gap-analysis.md) | 草案 | ZLM 行为拆解、本地现状、实现缺口、风险 |
| [phase-01-streamid-version-auth.md](phase-01-streamid-version-auth.md) | 待执行 | Stream ID、vhost、默认模式、鉴权参数、版本策略 |
| [phase-02-nack-arq-latency-stats.md](phase-02-nack-arq-latency-stats.md) | 待执行 | NACK/ARQ 观测、latency/buffer、Listener 加固、弱网 |
| [phase-03-module-publish-play-ts-bridge.md](phase-03-module-publish-play-ts-bridge.md) | 待执行 | publish/play 业务、TS-only、租约/订阅、module 拆分 |
| [phase-04-fec-packet-filter.md](phase-04-fec-packet-filter.md) | 待执行 | FEC / Packet Filter 协商、编解码、降级、指标 |
| [phase-05-interop-ops-fuzz.md](phase-05-interop-ops-fuzz.md) | 待执行 | OBS/ffmpeg/ffplay/VLC 矩阵、fuzz、运维收口 |

---

## 渐进式执行顺序

1. **Phase 01** — 先固定 streamid / vhost / 默认模式 / 鉴权参数 / 版本策略，防止后续 module 反复改业务语义。
2. **Phase 02** — 补齐 NACK/ARQ 观测、latency/buffer 配置、Listener 超时与弱网验收。
3. **Phase 03** — module 层对齐 ZLM 推/拉、TS-only、租约与跨协议桥，并拆分过大源文件。
4. **Phase 04** — 实现 FEC（超越 ZLM），协商失败可降级。
5. **Phase 05** — 外部实体互操作、fuzz/corpus、运维指标与文档收口。

---

## 总体验收

每个阶段完成后至少运行：

```bash
cargo fmt
cargo clippy -p cheetah-srt-core
cargo clippy -p cheetah-srt-driver-tokio
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-core
cargo test -p cheetah-srt-driver-tokio
cargo test -p cheetah-srt-module
cargo test -p cheetah-srt-property-tests
```

影响 `cheetah-codec` 的 MPEG-TS 路径时追加：

```bash
cargo clippy -p cheetah-codec
cargo test -p cheetah-codec
```

外部互操作（示例，对齐 ZLM `srt.md`）：

```bash
# OBS 推流地址
# srt://192.168.1.105:9000?streamid=#!::r=live/test,m=publish

# FFmpeg 推流
ffmpeg -re -stream_loop -1 -i test.ts -c:v copy -c:a copy -f mpegts \
  "srt://127.0.0.1:9000?streamid=#!::r=live/test,m=publish"

# ffplay 拉流
ffplay -i "srt://127.0.0.1:9000?streamid=#!::r=live/test"

# VLC 拉流：偏好设置 -> 串流输出 -> 访问输出 -> SRT 中设置 streamid
# 例如 #!::r=live/test ；播放地址仅填 srt://127.0.0.1:9000
```

---

## 阅读顺序建议

1. 本索引 `index.md`
2. [srt-zlm-gap-analysis.md](srt-zlm-gap-analysis.md) 了解缺口
3. [srt-zlm-architecture.md](srt-zlm-architecture.md) 锁定目标架构
4. 按 Phase 01 → 05 执行
5. 基线细节回看 [plans-28-srt](../plans-28-srt/index.md)
