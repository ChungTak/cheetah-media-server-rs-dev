# SRT 与 ZLMediaKit 差距分析

## ZLMediaKit 关键行为

### 代码位置

| 区域 | 路径 | 说明 |
|------|------|------|
| SRT 协议与业务 | `vendor-ref/ZLMediaKit/srt/` | **主参考** |
| 文档 | `srt/srt.md`、`srt/srt_en.md` | 特性、用法、FEC 未实现声明 |
| TS / 媒体 | `vendor-ref/ZLMediaKit/src/TS/*`、`src/Rtp/TSDecoder.*`、`src/Record/MPEG.*` | TS 源与解码 |
| 通用媒体 / 鉴权 | `src/Common/MediaSource.*` 等 | publish/play 广播鉴权 |

### 核心类职责

#### `SrtTransport`（协议传输）

- UDP 入站分流：handshake induction/conclusion、data、ACK、NAK、keepalive、shutdown、drop req、KM。
- 维护 send/recv packet queue、RTT、light ACK、周期性 NAK。
- 加密：`Crypto` + KeyMaterial 通告。
- 超时：alive ticker + configurable `timeoutSec`。
- 配置：`srt.port`、`timeoutSec`、`latencyMul`、`pktBufSize`、`passPhrase`。

#### `SrtTransportImp`（业务桥）

- `onHandShakeFinished`：解析 streamid，决定 pusher / player。
- `parseStreamid`：
  - 必须以 `#!::` 开头，否则失败断开。
  - `h` → vhost（空则 `DEFAULT_VHOST`）。
  - `r` → 按 `/` 拆成 `app` + `stream`（不足两段则失败）。
  - **其余字段（含 `m`）拼进 `params`**，供鉴权 webhook 使用。
- 模式：`params["m"] == "publish"` 才是推流，**否则拉流**（含无 `m`）。
- 推流：`DecoderImp::decoder_ts` + `MultiMediaSourceMuxer` + `BroadcastMediaPublish`。
- 拉流：`BroadcastMediaPlayed` 后 `MediaSource::findAsync` 找 **TS schema** 源并 attach ring。

#### `NackContext`

- `update`：把丢失区间 merge 进 map。
- `getLostList`：未发过 NAK 的立即发；已发过的按 **RTT** 间隔重发；处理 seq 回绕。
- `drop`：确认收到后清理丢失表。

#### `PacketQueue` / `PacketSendQueue`

- 接收队列：按 latency 与 TLPKTDROP 丢弃过期包。
- 发送队列：缓存用于 NAK 触发的重传；有大小上界。

#### `HSExt`

- HS 扩展：`srt_version`、`srt_flag`（TSBPDSND/RCV、CRYPT、TLPKTDROP、PERIODICNAK、PACKET_FILTER 等位）、TSBPD delay。
- Stream ID 扩展：4 字节字序特殊排布（ZLM 兼容实现细节）。
- KM 扩展：加密材料。
- 声明 `PACKET_FILTER` flag 位，但 **FEC 逻辑未实现**（文档写明）。

#### `SrtCaller` / `SrtPlayer` / `SrtPusher`

- Caller 侧对称握手、NAK、TS 收发。
- 用于主动拉远端 / 推远端场景。

### ZLM 文档声明的能力

来自 `srt/srt.md`：

- ✅ NACK（重传）
- ✅ Listener
- ✅ 推流仅 TS / 拉流仅 TS
- ✅ 协议参考 SRT draft
- ✅ 版本 `>=1.3.0`
- ❌ **FEC 没有实现**

用法与 streamid 语义与用户需求一致（本计划以该文档 + `parseStreamid` 源码为准）。

---

## 当前本地状态

### 已有能力（`plans-28-srt` 基线）

| Crate | 状态 |
|-------|------|
| `cheetah-srt-core` | URL / Stream ID 解析、配置与 session I/O 类型、错误类型 |
| `cheetah-srt-driver-tokio` | UDP listener/caller、`SrtConnection` 驱动、timer、加密、stats、背压、连接上限 |
| `cheetah-srt-module` | publish/play 分支、TS demux/mux、engine 发布/订阅、jobs、metrics HTTP |
| `shiguredo_srt` | LiveCC、ACK/NAK/ACKACK、TSBPD、TLPKTDROP、AES、Stream ID 扩展、stats |
| 测试 | core parser 测试、driver smoke、property tests、3 个 fuzz target、部分外部互操作记录 |

### 关键实现细节

**Stream ID**（`core/src/stream_id.rs`）：

- 支持 `#!::` 与 bare key。
- 字段：`stream_key`（来自 `r` 整体）、`mode`、`user`、`host`、`session`、`extras`。
- **没有** 显式 `vhost/app/stream` 三分。
- **没有** 把 `m` 强制并入 auth_params 模型（`m` 被消费为 mode）。

**默认模式**（`module/src/config.rs` + `classify_stream`）：

```text
ingress.default_mode 默认 = "publish"   // 与 ZLM 相反
mode = parsed.mode.unwrap_or(default_mode)
```

**StreamKey**（`stream_key_from_string`）：

```text
"live/test" → namespace=live, path=test
"test"      → namespace=live, path=test
```

`host`/`h` **不参与** key。

**鉴权**：

- `auth.enabled` + 全局 publish/request token + `users[]`。
- 从 `extras["token"]` 与 `user` 校验。
- **无** ZLM 风格「全部非 h/r 参数进 webhook」模型。

**NACK**：

- 完全依赖 `shiguredo_srt` 内部。
- driver stats 已含 `sender_total_retransmits`、`receiver_total_lost`、loss list 深度、RTT、jitter。
- 缺：与 ZLM 对等的弱网验收清单固化、TLPKTDROP 独立计数（库未必暴露）、配置化 nack 行为文档。

**版本**：

- 库默认 `srt_version: 0x010500`。
- 本地 **未** 配置 `min_peer_srt_version` 或拒绝逻辑测试。

**FEC**：

- 库 README / 源码无 Packet Filter / FEC。
- 本地无相关配置与代码。

**媒体路径**：

- 仅 `SrtPayloadKind::MpegTs`。
- 推：demux → engine；拉：engine → mux。能力上满足 TS-only，语义上不同于 ZLM TS ring 直通。

---

## 必须补齐的实现缺口

### 1. Stream ID 与资源定位

| 缺口 | 说明 | Phase |
|------|------|-------|
| `h` → vhost | 需进入 meta / 可选 key 策略 | 01 |
| `r` → app/stream | 显式拆分；严格模式两段校验 | 01 |
| 默认 mode = 拉流 | 改默认并提供兼容配置 | 01/03 |
| auth_params | 除 h/r 外所有 key（含 m） | 01 |
| bare / 非 `#!::` | ZLM 直接拒绝；本地接受 bare。需策略开关 | 01 |
| VLC 无 query streamid | streamid 仅在 HS 扩展，本地已支持 Connected.stream_id；需 E2E 验证 | 05 |

### 2. 鉴权与 webhook

| 缺口 | 说明 | Phase |
|------|------|-------|
| 参数模型 | 对齐 ZLM params 拼接/ map | 01 |
| webhook 转发 | 依赖 control 能力；可先结构 + hook | 01/03 |
| 推流/拉流分别鉴权 | 已有 token 分轨，需并入 auth_params | 01/03 |

### 3. NACK / 延迟 / 缓冲

| 缺口 | 说明 | Phase |
|------|------|-------|
| 行为验收矩阵 | 1%/5%/12% loss、reorder、限速 | 02 |
| 配置对齐 | latencyMul、pktBufSize 语义文档与字段 | 02 |
| TLPKTDROP 指标 | 库字段调研；不可得则文档说明 | 02 |
| 不自研 NACK | 除非库不足 | 02 |

### 4. Listener 与会话生命周期

| 缺口 | 说明 | Phase |
|------|------|-------|
| 非法 streamid 断开 | 对齐 ZLM onShutdown | 03 |
| 拉流无源断开 | ZLM findAsync 失败 shutdown | 03 |
| 推流鉴权失败断开 | 已有基础，需参数化完善 | 03 |
| max_connections / idle | 已有，补测试与 metrics | 02/03 |

### 5. 版本策略

| 缺口 | 说明 | Phase |
|------|------|-------|
| min peer version | 配置 + 拒绝 + 指标 | 01/02 |
| 单元/集成测试 | 构造低版本 HS 扩展（若可） | 02 |

### 6. FEC

| 缺口 | 说明 | Phase |
|------|------|-------|
| 全链路缺失 | 配置、协商、恢复、指标、互操作 | 04 |
| 库能力 | 需评估扩展路径 | 04 |
| 降级策略 | 对端不支持时 ARQ-only | 04 |

### 7. 工程卫生

| 缺口 | 说明 | Phase |
|------|------|-------|
| `module.rs` 过大 | 拆 stream_classify/auth/session/jobs | 03 |
| 文档与 ops | 收编到 Phase 05 + 链接 plans-28-srt ops | 05 |

---

## 编码与负载矩阵

本轮 **不扩展** codec 集合；沿用 `cheetah-codec` MPEG-TS 支持矩阵：

| 负载 | 推流 | 拉流 | 策略 |
|------|------|------|------|
| MPEG-TS | 必须 | 必须 | 唯一支持 |
| H264/H265/… 于 TS 内 | 透传 demux | 透传 mux | 不转码 |
| 非 TS 容器 | 拒绝 | 拒绝 | 配置与运行时校验 |

---

## 互操作风险

1. **默认 mode 变更** 导致旧客户端「无 m 却期望推流」失败 → 配置回退与迁移说明。
2. **严格 `#!::` + 两段 `r`** 与宽松 bare key 客户端冲突 → 开关。
3. **VLC** streamid 配置路径隐蔽，易测漏。
4. **OBS** 对 streamid URL 编码敏感（`#`、`!`、`:`）。
5. **FEC** 仅部分 libsrt/ffmpeg 构建支持，互操作矩阵必须标注构建选项。
6. **加密** passphrase 不匹配应稳定失败（已有基础，需保留回归）。
7. **单发布者租约**：重复 `m=publish` 同 key 必须 Conflict，禁止静默顶替。

---

## 与 `plans-28-srt` 的边界

| 已由 plans-28-srt 覆盖 | 本计划增量 |
|------------------------|------------|
| crate 脚手架 | 不重复 |
| 基础 listener/caller | 加固与观测 |
| TS demux/mux 主路径 | ZLM streamid 语义与默认 mode |
| 基础加密与 jobs | 鉴权参数模型 |
| 部分互操作记录 | 正式矩阵 + VLC/OBS + 默认 mode |
| 基础 retransmit stats | 弱网验收 + 配置对齐 |
| 无 FEC | **新增 FEC** |

---

## 建议优先级

```text
P0  Stream ID / 默认 mode / vhost / auth_params     → Phase 01
P0  publish/play 业务与 TS-only 硬约束             → Phase 03（依赖 01）
P1  NACK 观测 + 弱网 + latency/buffer              → Phase 02
P1  版本拒绝策略                                    → Phase 01/02
P2  FEC                                             → Phase 04
P2  完整互操作 / fuzz / ops 收口                    → Phase 05
```
