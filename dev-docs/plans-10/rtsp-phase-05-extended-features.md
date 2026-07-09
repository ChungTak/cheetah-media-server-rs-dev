# Phase-05: 扩展特性 — RTCP-XR + FEC 评估 + 未来扩展

## 目标

评估并实现高级 RTP/RTCP 扩展特性，为未来协议演进预留接口。

## 任务清单

### 5.1 RTCP-XR 基础支持

**范围**：`cheetah-rtsp-core` + `cheetah-rtsp-driver-tokio`

**参考**：RFC 3611 — RTP Control Protocol Extended Reports

**实现方案**：

1. core 层包模型：
   ```rust
   /// RFC 3611 Extended Report
   pub struct RtcpXr {
       pub sender_ssrc: u32,
       pub blocks: Vec<XrBlock>,
   }

   pub enum XrBlock {
       /// §4.1 Loss RLE Report Block
       LossRle { ... },
       /// §4.6 VoIP Metrics Report Block
       VoipMetrics {
           ssrc: u32,
           loss_rate: u8,
           discard_rate: u8,
           burst_density: u8,
           gap_density: u8,
           round_trip_delay: u16,
           end_system_delay: u16,
           jitter_buffer_nominal: u16,
           jitter_buffer_maximum: u16,
           jitter_buffer_abs_max: u16,
       },
       /// §4.7 Statistics Summary Report Block
       StatsSummary { ... },
       /// 未知 block type（前向兼容）
       Unknown { block_type: u8, data: Vec<u8> },
   }
   ```

2. driver 层：
   - 解析收到的 RTCP-XR 包，提取质量指标
   - 定期生成 RTCP-XR 发送给对端（可配置间隔）
   - 指标暴露给 module 层用于监控

3. 配置：
   ```yaml
   rtcp_xr:
     enabled: false          # 默认关闭，按需开启
     report_interval_ms: 5000
     blocks: [voip_metrics]  # 启用的 block 类型
   ```

4. 优先级：低。仅在有明确监控需求时实现。

### 5.2 FEC 可行性评估

**范围**：评估文档，不立即实现

**参考**：RFC 5109 — RTP Payload Format for Generic Forward Error Correction

**评估要点**：

| 维度 | 分析 |
|------|------|
| 适用场景 | UDP 传输、高丢包网络（无线、跨公网） |
| CPU 开销 | XOR FEC 开销低，RS FEC 开销中等 |
| 带宽开销 | 典型 10-30% 冗余 |
| 延迟影响 | 增加一个 FEC group 的延迟（通常 20-100ms） |
| 实现复杂度 | 中等（需要 FEC 编码器/解码器 + 分组管理） |
| 生态兼容性 | FFmpeg 支持 `fec://`，VLC 不支持，GStreamer 部分支持 |
| 替代方案 | NACK 重传（Phase-02 已实现）在低延迟场景更优 |

**结论**：
- 短期不实现 FEC，NACK 重传已覆盖主要丢包恢复需求
- 预留 FEC 接口设计，未来如有强需求可扩展
- 如果实现，放在 driver 层（FEC 编解码是 I/O 相关的分组操作）

### 5.3 RTSP REDIRECT 支持

**范围**：`cheetah-rtsp-core` + `cheetah-rtsp-module`

**参考**：RFC 2326 §10.10 REDIRECT

**实现方案**：

1. 服务端发起 REDIRECT：
   ```
   REDIRECT rtsp://example.com/live/test RTSP/1.0
   CSeq: 5
   Location: rtsp://backup.example.com/live/test
   Range: npt=now-
   ```

2. 使用场景：
   - 服务器负载均衡：将客户端重定向到其他节点
   - 源迁移：流从一个服务器迁移到另一个
   - 维护窗口：优雅地将客户端迁移走

3. 客户端处理：
   - 收到 REDIRECT 后，TEARDOWN 当前会话
   - 使用 `Location` 头中的新 URL 重新建立连接
   - Pull/Push 任务自动处理 REDIRECT

4. 配置：
   ```yaml
   redirect:
     enabled: false
     # 由 control API 触发，不在 RTSP 配置中静态配置目标
   ```

### 5.4 编解码器扩展能力

**范围**：`cheetah-rtsp-core` + `cheetah-codec`

**当前支持**：H.264/H.265/AV1/VP8/VP9/AAC/Opus/G.711A/G.711U/MP3

**扩展策略**：

1. 已支持编解码器（完整 depacketize + packetize）：
   - H.264, H.265, AV1, VP8, VP9
   - AAC (MPEG4-GENERIC + MP4A-LATM), Opus, G.711A, G.711U, MP3

2. 透传编解码器（仅转发 RTP，不解析 payload）：
   - H.266/VVC（未来补完整支持）
   - JPEG（静态图像，低优先级）
   - 任何未知 PT 的编解码器

3. 透传模式实现：
   ```rust
   pub struct PassthroughDepacketizer;

   impl Depacketizer for PassthroughDepacketizer {
       fn depacketize(&mut self, rtp: &RtpPacket) -> Option<AVFrame> {
           // 将 RTP payload 直接包装为 AVFrame
           // codec_id = Unknown, data = raw RTP payload
           // 可以在同协议间转发，但不能转协议
       }
   }
   ```

4. 编解码器注册表：
   - SDP `rtpmap` 名称 → CodecId 映射
   - 未知编解码器自动使用透传模式
   - 日志记录未识别的编解码器名称

### 5.5 未来扩展预留

以下特性不在本计划范围内，但架构设计应不阻碍其未来实现：

| 特性 | 预留接口 | 预计时间 |
|------|----------|----------|
| RTSP 2.0 (RFC 7826) | 版本协商在 core 层 | 远期 |
| SRTP/DTLS-SRTP | driver 层加密接口 | 中期 |
| VOD 点播 | module 层 seek/pause 语义 | 中期 |
| 录制 | module 层 sink 接口 | 中期 |
| 带宽自适应 (TMMBR) | RTCP-FB 扩展 | 远期 |
| SDP offer/answer | core 层 SDP 协商模型 | 远期 |

## 测试计划

| 测试类型 | 内容 |
|----------|------|
| 单元测试 | RTCP-XR 包编解码正确性 |
| 单元测试 | REDIRECT 消息解析 |
| 单元测试 | 透传 depacketizer 正确包装 |
| fuzz | RTCP-XR 畸形包解析不崩溃 |
| 集成测试 | REDIRECT 后客户端自动重连 |
| 集成测试 | 未知编解码器透传转发 |

## 完成标准

- [ ] RTCP-XR VoIP Metrics 正确生成和解析
- [ ] REDIRECT 触发后客户端自动迁移
- [ ] 未知编解码器不崩溃，透传转发正常
- [ ] FEC 评估文档完成，架构不阻碍未来实现
