# SRT ZLM 兼容行为规范（自包含知识库）

> 本文档把成熟流媒体服务器（ZLMediaKit 风格）的 SRT 业务语义、握手标志、NACK 策略、客户端用法 **完整提取** 为可执行规范。  
> **实现智能体无需也不应依赖任何 vendor 目录**；只需阅读本目录文档 + 本仓库 `crates/protocols/srt/**` + `AGENTS.md` + [SRT RFC draft](https://haivision.github.io/srt-rfc/draft-sharabayko-srt.html)。

---

## 1. 产品特性清单（目标行为）

| 特性 | 要求 |
|------|------|
| NACK / 重传 | 接收侧检测丢包并发送 NAK；发送侧按 seq 重传；可按 RTT 重复 NAK |
| Listener | 服务端 UDP 监听，接受多个 Caller |
| 推流封装 | **仅** MPEG-TS over SRT |
| 拉流封装 | **仅** MPEG-TS over SRT |
| 协议 | 遵循 SRT draft（握手 HS v5、Live、ACK/NAK、TSBPD 等） |
| 版本 | 支持并对齐 **SRT >= 1.3.0**（本端可宣告 1.5.0） |
| FEC | 目标实现 Packet Filter / FEC（参考实现未必有；本项目要做） |

---

## 2. Stream ID 语法与语义（强制对齐）

### 2.1 格式

SRT 握手扩展中的 Stream ID 字符串采用 Access Control 风格：

```text
#!::key1=value1,key2=value2,key3=value3,...
```

- 前缀固定为 4 字符：`#` `!` `:` `:`  
- 其后为逗号分隔的 `key=value` 对  
- key/value 可做 percent-encoding（`%XX`）；`+` 为字面量，不是空格  

### 2.2 特殊 key

| Key | 名称 | 规则 |
|-----|------|------|
| `h` | vhost | 可选。缺失时使用服务器默认 vhost（常见默认名 `__defaultVhost__`） |
| `r` | resource | **必填**。值为 `app/stream` 形式，**至少两段**（第一个 `/` 分割：app=前，stream=后）。不足两段 → 整个 streamid **非法**，应断开连接 |
| `m` | mode | 可选。`m=publish` → **推流**；**其它任何值或缺省** → **拉流** |
| 其它 key | 鉴权/业务参数 | 与 `m` 一起进入鉴权参数集合（webhook / 本地 auth） |

### 2.3 解析伪代码（必须按此语义实现）

```text
function parse_zlm_stream_id(streamid, default_vhost):
    if not streamid.starts_with("#!::"):
        return Error("stream id must start with #!::")

    body = streamid[4..]
    map = parse_csv_kv(body, sep=',', kv='=')  // 每段 percent-decode

    vhost = map.remove("h") or default_vhost
    resource = map.remove("r")
    if resource is None:
        return Error("missing r")

    parts = split(resource, "/", maxsplit=1)  // 仅按第一个 / 也可，但 app 与 stream 都非空
    // 参考行为：split 后 size < 2 则失败
    if parts.length < 2 or parts[0] empty or parts[1] empty:
        return Error("r must be app/stream")

    app = parts[0]
    stream = parts[1]

    // 剩余全部 key（含 m,u,token,...）进入 auth_params
    auth_params = map  // 注意：m 仍留在 auth_params 里

    is_publish = (auth_params.get("m") == "publish")
    mode = Publish if is_publish else Play   // 缺省拉流

    return Ok({ vhost, app, stream, mode, auth_params })
```

### 2.4 权威样例

| 输入 streamid | vhost | app | stream | 模式 |
|---------------|-------|-----|--------|------|
| `#!::h=zlmediakit.com,r=live/test,m=publish` | zlmediakit.com | live | test | **推流** |
| `#!::r=live/test,m=publish` | default_vhost | live | test | **推流** |
| `#!::r=live/test` | default_vhost | live | test | **拉流** |
| `#!::r=live/test,m=request` | default_vhost | live | test | **拉流** |
| `#!::r=live/test,m=play,token=abc` | default_vhost | live | test | **拉流**；auth 含 m,token |
| `#!::m=publish`（无 r） | — | — | — | **非法** |
| `#!::r=live`（单段） | — | — | — | **非法** |
| `live/test`（无前缀） | — | — | — | **严格模式非法**（可用兼容开关放行） |

### 2.5 客户端用法（互操作必测）

**OBS 推流**

```text
srt://192.168.1.105:9000?streamid=#!::r=live/test,m=publish
```

**FFmpeg 推流（mpegts）**

```bash
ffmpeg -re -stream_loop -1 -i test.ts -c:v copy -c:a copy -f mpegts \
  "srt://192.168.1.105:9000?streamid=#!::r=live/test,m=publish"
```

**ffplay 拉流**

```bash
ffplay -i "srt://192.168.1.105:9000?streamid=#!::r=live/test"
```

**VLC 拉流**

1. 偏好设置 → 输入与编解码器 / 串流输出 → 访问输出 → **SRT**  
2. 设置 streamid，例如：`#!::r=live/test`  
3. 打开网络串流时 URL **只填** `srt://192.168.1.105:9000`（streamid 不在 URL query 里，而在 HS 扩展中）

---

## 3. 握手后业务状态机（Listener 侧）

握手完成拿到 `stream_id` 后：

```text
1. parse streamid
2. if parse fail → 关闭连接（reject reason: bad stream id）
3. if mode == publish:
     a. 推流鉴权（auth_params + 资源定位）
     b. 鉴权失败 → 关闭
     c. 申请单发布者租约（同 StreamKey 冲突 → 关闭）
     d. 创建 MPEG-TS demuxer
     e. 之后每个 SRT 数据 payload → demux → 发布音视频帧
     f. 断开时 flush demuxer，释放租约
4. else:  // 拉流
     a. 拉流鉴权
     b. 查找本地流（StreamKey）
     c. 找不到 / 超时 → 关闭
     d. 订阅媒体 → MPEG-TS mux → SRT SendPayload
     e. 源 detach / 订阅结束 → 关闭
```

### 3.1 推流侧约束

- 仅接受 TS 负载；非推流连接上收到媒体 payload 应忽略或断开（参考：player 连接忽略推流数据）。
- 鉴权完成前到达的帧可短队列缓存（参考上限约 200 帧），过长丢弃。
- 同一 `app/stream`（映射后的 StreamKey）**单发布者独占**。

### 3.2 拉流侧约束

- 源不存在必须失败关闭，不能空挂死连接（可配置等待超时，默认建议 15s）。
- 输出必须是 MPEG-TS 字节流。

---

## 4. NACK / ARQ 行为规范

协议层（SRT）负责：

1. **接收侧**  
   - 按 packet sequence 检测空洞 → 丢失区间列表  
   - 对未 NAK 过的 seq **立即**加入待发 NAK  
   - 已 NAK 过的 seq：若距上次 NAK 时间 **> RTT**，则再次 NAK  
   - 收到补包或确认后从丢失表删除  
   - 注意 32-bit seq 回绕（半窗口 `MAX_SEQ/2` 逻辑）

2. **发送侧**  
   - 维护有界重传缓存  
   - 收到 NAK 后重发对应 seq  
   - 结合 TSBPD / TLPKTDROP：过期包可丢弃，避免无限缓冲

3. **周期**  
   - 握手 flags 常包含 `PERIODICNAK`、`TLPKTDROP`、`TSBPDSND/RCV`

**本项目实现策略**：不在 module 手写 NACK 状态机；依赖 `shiguredo_srt` 内置 ACK/NAK/retransmit。必须做到：

- 配置 `latency_ms`（TSBPD delay）与缓冲上界  
- 暴露 retransmit / lost / rtt / jitter / loss_list 指标  
- 弱网场景自动化或可重复验收  

---

## 5. 握手扩展与版本

### 5.1 HS 扩展类型（SRT）

| 值 | 名称 | 用途 |
|----|------|------|
| 1 | HSREQ | 握手请求扩展 |
| 2 | HSRSP | 握手响应扩展 |
| 3/4 | KMREQ/KMRSP | 密钥材料 |
| 5 | SID | Stream ID |
| 6 | CONGESTION | 拥塞控制 |
| 7 | FILTER | Packet Filter / FEC |
| 8 | GROUP | 组（本阶段非目标） |

### 5.2 HS Message flags 位

| 位掩码 | 名称 | 含义 |
|--------|------|------|
| `0x00000001` | TSBPDSND | 发送侧 TSBPD |
| `0x00000002` | TSBPDRCV | 接收侧 TSBPD |
| `0x00000004` | CRYPT | 加密 |
| `0x00000008` | TLPKTDROP | 过期包丢弃 |
| `0x00000010` | PERIODICNAK | 周期 NAK |
| `0x00000020` | REXMITFLG | 重传标志 |
| `0x00000040` | STREAM | stream 模式 |
| `0x00000080` | PACKET_FILTER | Packet Filter / FEC |

HS Message 布局（16 字节 body）：

```text
u32 srt_version
u32 srt_flags
u16 recv_tsbpd_delay_ms
u16 send_tsbpd_delay_ms
```

### 5.3 版本编码

```text
version_u32 = (major << 16) | (minor << 8) | patch
1.3.0 = 0x00010300
1.5.0 = 0x00010500
```

- 本端宣告建议：`1.5.0`（与主流 libsrt / shiguredo_srt 默认一致）  
- 策略：peer 版本 **< 1.3.0** 时拒绝（可配置 `min_peer_srt_version`）  
- 无版本扩展时：默认兼容放行，或由 `require_peer_version_extension` 控制  

### 5.4 配置项对照（运维语义）

| 配置语义 | 建议默认 | 说明 |
|----------|----------|------|
| listen port | 9000 | UDP |
| timeoutSec / idle | 5–30s | 无活动断开 |
| latency / TSBPD | 120 ms | 延迟缓冲 |
| latencyMul | 4 | 文档建议 delay≈(3~4)×RTT；可配置展示 |
| pktBufSize | 8192 | 包缓冲上界量级 |
| passPhrase | 空 | AES 加密口令 |

---

## 6. FEC / Packet Filter（本项目目标；参考实现常缺失）

- SRT 通过 HS 扩展类型 FILTER（7）与 flag `PACKET_FILTER` 协商过滤器。  
- 常见 FEC 配置形态（libsrt 风格字符串，供互操作文档使用）：  
  `fec,cols:10,rows:5`  
- 行为目标：  
  - 发送侧按矩阵生成校验包  
  - 接收侧用校验恢复丢失数据包  
  - 协商失败可降级为纯 ARQ（除非 `fec.required=true`）  
- 当前依赖库 `shiguredo_srt` **无 FEC**；实现需扩展库或可控 vendor patch（见 Phase 04）。

---

## 7. 媒体路径差异（Cheetah 必须遵守）

参考实现拉流时常直接转发 **TS 包 ring**。Cheetah 统一媒体模型：

```text
推流: SRT payload(TS) → MpegTsDemuxer → AVFrame+TrackInfo → engine
拉流: engine AVFrame+TrackInfo → MpegTsMuxer → SRT payload(TS)
```

禁止：

- 在 SRT module 复制一套 NALU/时间戳修正  
- 绕过 engine 做跨协议私有直连（除非未来单独设计 TS 直通并标注）  
- 支持非 TS 容器 over SRT  

---

## 8. 拒绝原因（实现时应可观测）

| 场景 | 建议 reason 字符串或枚举 |
|------|--------------------------|
| streamid 非法 | `invalid_stream_id` |
| 鉴权失败 | `auth_rejected` |
| 发布租约冲突 | `publish_conflict` |
| 拉流无源/超时 | `stream_not_found` / `play_timeout` |
| peer 版本过低 | `peer_version_too_old` |
| 连接数满 | `max_connections` |
| FEC 必需但未协商 | `fec_required` |
| 加密口令错误 | `bad_secret`（库/握手层） |

---

## 9. 与本仓库本地现状的差异摘要

（详细见 [srt-zlm-gap-analysis.md](srt-zlm-gap-analysis.md)）

| 项 | 兼容规范 | 本地当前（改之前） |
|----|----------|-------------------|
| 无 `m` | 拉流 | `ingress.default_mode` 默认 `publish` |
| `r` | 必须 app/stream 两段 | 整串 stream_key，可单段 |
| `h` | vhost | 解析为 host 字段，不参与 StreamKey |
| 鉴权参数 | 除 h/r 外全部 key 含 m | 主要 token/u |
| FEC | 需要 | 无 |
| 版本下限 | >=1.3.0 策略 | 库默认 1.5.0，无拒绝策略 |

---

## 10. 外部规范链接（仅公开 URL）

- SRT RFC draft: https://haivision.github.io/srt-rfc/draft-sharabayko-srt.html  
- shiguredo_srt docs: https://docs.rs/shiguredo_srt/  
- shiguredo_srt crate: https://crates.io/crates/shiguredo_srt  
