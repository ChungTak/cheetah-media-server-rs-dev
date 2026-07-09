# 双时间线媒体架构故障排查手册（plans-4）

- 状态：已完成
- 范围：RTSP/RTMP 推拉流、双向转协议、RTSP TCP/UDP 差异、未来 SRT/WebRTC 接入。
- 目标：定位 source timeline、canonical timeline、egress timeline 中哪一层导致不秒开、启动快放、播放不顺、DTS/PTS 异常。

## 1. 排查输入条件

每次排查必须记录：

1. 场景：RTSP 同协议、RTMP 同协议、RTSP->RTMP、RTMP->RTSP、TCP/UDP。
2. 输入素材：是否有 B 帧、fps、音频编码、GOP 长度。
3. 推流命令和拉流命令。
4. server 配置：bootstrap window、queue capacity、pacing、transport。
5. 日志级别：至少保留 `-v debug` 或服务端结构化日志。
6. 抓包文件：RTMP 使用 TCP pcap，RTSP 使用 RTP/TCP interleaved 或 UDP pcap。

## 2. 常见现象与定位路径

### 2.1 播放不如旧版本顺畅

- 现象：日志不一定报错，但画面轻微顿挫、首段节奏不自然。
- 重点检查：RTSP 入站是否把 RTP timestamp 同时作为 PTS/DTS；canonical DTS 是否出现大量 `+1 tick` 修复。
- 修复方向：RTP timestamp 作为 source PTS，canonical DTS 按 AU 顺序和平滑步进生成。

### 2.2 GOP 秒开失败

- 现象：拉流后等待 1-2 秒才出画面。
- 重点检查：RingBuffer bootstrap 是否被错误 discontinuity 截断；RTMP/RTSP play 是否先发非关键帧。
- 修复方向：正常 B 帧重排不标记 discontinuity；bootstrap 从最新 keyframe 开始；play gate 等待关键帧。

### 2.3 首段快放

- 现象：刚播放出来的缓存帧速度很快，约 1 秒后恢复。
- 重点检查：egress 是否一次性冲出历史 GOP；pacing 是否使用 source RTP epoch 或未 rebased RTMP timestamp。
- 修复方向：首个媒体帧立即发送，后续按 canonical media timestamp pacing。

### 2.4 `Invalid timestamps` / `Non-increasing DTS` / `Negative cts`

- 现象：ffplay/ffprobe 或服务端日志报时间戳异常。
- 重点检查：RTMP egress timestamp 是否来自 canonical DTS；CTS 是否来自 PTS-DTS；负 CTS 是否只是目标封装限制导致。
- 修复方向：目标协议时间修复只发生在 egress view，不回写 engine frame。

### 2.6 正常 B 帧被误判为入站异常

- 现象：出现少量 PTS 重排时，日志被误解为“时间线修复异常”，但播放实际正常。
- 重点检查：`alert_class` 是否为 `source_disorder`（而非 `canonical_repair` / `discontinuity`）。
- 判定规则：
1. `source_disorder`：源时间线乱序观测（如 B 帧重排），默认不应作为故障升级。
2. `canonical_repair`：canonical 时间线被修复（如 `NonMonotonicDtsRepaired`），需要关注是否高频出现。
3. `discontinuity`：切段/大跳变，需结合推流重连、长跳变、抓包判断是否真实断流事件。
- 修复方向：保持 source/canonical 分层，不要把 `source_disorder` 当成 `canonical_repair` 处理。

### 2.5 RTSP TCP/UDP 表现不一致

- 现象：同一素材 TCP 正常、UDP 不正常，或反之。
- 重点检查：进入 engine 后 source/canonical 序列是否一致；UDP 是否有真实丢包或乱序。
- 修复方向：transport 差异只影响 packet loss/corruption/discontinuity，不改变正常时间模型。

## 3. 标准排查流程

1. 抓服务端日志，按 `stream_key/track_id/codec` 聚合。
2. 在 ingress 边界打印 source timestamp 与 canonical pts/dts。
3. 在 engine bootstrap 边界确认 keyframe、discontinuity、起播窗口。
4. 在 egress 边界打印目标协议 timestamp、CTS/RTP timestamp。
5. 用 pcap 验证 play 后首个 keyframe burst 是否立即发送。
6. 用 ffprobe 验证首包 keyframe、pts/dts、duration、packet interval。
7. 对比 B 帧和非 B 帧素材，确认问题是否只发生在 PTS-only/reorder 场景。

## 3.1 双时间线快速定位流程

当问题是“起播慢、首秒快放、偶发顿挫、日志无明显错误”时，按下面顺序定位：

1. 先看 source timeline：检查是否只有 `source_disorder`，且 canonical/egress repair 计数很低。若是，通常属于正常 B 帧重排观测，不直接判故障。
2. 再看 canonical timeline：若 `canonical_repair_events` 高频，优先排查 ingress normalizer 输入模式是否正确（视频应走 PTS-only，音频保留 DTS 语义）。
3. 最后看 egress timeline：若 `egress_repair_events` 高频但 canonical 基本稳定，优先排查协议导出和封装层修复策略，不要回改 ingress。
4. 对比 `first_keyframe_delay_ms` 与 `startup_latency_ms`：
   - `first_keyframe_delay_ms` 大：通常是 keyframe gate 或 bootstrap keyframe 选择问题。
   - `startup_latency_ms` 大但 keyframe delay 小：通常是 pull 播放链路阻塞或外部网络/缓冲问题。
5. 校验日志上下文完整性：repair 类日志必须同时包含 source timestamp 与 canonical `pts/dts`。缺失上下文时先补日志，再继续定位。

## 4. 常用命令

```bash
# 查关键异常
rg -in "invalid timestamps|non-increasing dts|negative cts|dts out of order|NonMonotonicDtsRepaired" \
  <push.log> <pull.log> <server.log>

# RTMP late join ffprobe
ffprobe -hide_banner -v debug -show_packets -select_streams v:0 \
  -read_intervals '%+#10' -print_format compact rtmp://127.0.0.1/live/test

# RTMP 抓包
tcpdump -i any -s 0 -w /tmp/cheetah_rtmp_late_join.pcap tcp port 1935

# RTSP UDP 抓包
tcpdump -i any -s 0 -w /tmp/cheetah_rtsp_udp.pcap udp
```

## 5. 验收清单

- [ ] 首包视频是 keyframe。
- [ ] 首包 `pts/dts` 从 0 或接近 0 开始。
- [ ] 首 1 秒平均帧间隔接近源 fps。
- [ ] 正常 B 帧输入不产生高频 `NonMonotonicDtsRepaired`。
- [ ] RTSP->RTMP 不出现 `Invalid timestamps` / `Negative cts`。
- [ ] RTSP 同协议播放顺畅度不低于旧版本。
- [ ] TCP/UDP 正常路径进入 engine 后 canonical 时间一致。

## 最新进展

- 2026-04-29：完成双时间线排障流程收口。手册新增“3.1 双时间线快速定位流程”，将 source/canonical/egress 三层定位顺序、`first_keyframe_delay_ms` 与 `startup_latency_ms` 的判读规则、repair 日志上下文字段完整性要求固化为统一排查路径。
- 2026-04-29：补充入站告警语义排障指引：新增 `alert_class` 分级判定（`source_disorder` / `canonical_repair` / `discontinuity`），明确“正常 B 帧重排不是入站故障升级条件”，用于避免误判和过度告警。
