# Phase-02: 媒体管线增强 — 静音音频 + RTP 重排序 + RTCP-FB

## 目标

提升媒体传输质量：为纯视频流自动注入静音音频、完善 RTP 包重排序、支持 RTCP 反馈机制请求关键帧和重传。

## 任务清单

### 2.1 静音音频生成器

**范围**：`cheetah-codec` + `cheetah-rtsp-module`

**参考**：ZLMediaKit `MuteAudioMaker`（`src/Common/MediaSink.h`）

**实现方案**：

1. `cheetah-codec` 新增 `MuteAudioMaker`：
   - 生成 AAC-LC 静音帧（固定 profile=LC, sample_rate=44100, channels=2）
   - 输入：视频帧时间戳 → 输出：对应时间段的 AAC 静音帧序列
   - 帧间隔：约 23ms（1024 samples / 44100Hz）
   - 内部维护已生成的音频时间戳，避免重复
   - 纯计算，无 I/O，无 async

2. `cheetah-rtsp-module` 集成：
   - 发布会话检测到只有视频轨时，创建 `MuteAudioMaker`
   - 每收到视频帧，调用 `MuteAudioMaker::fill_until(video_pts)` 生成静音帧
   - 生成的 AAC 帧通过正常 AVFrame 路径注入引擎
   - 自动为流添加 AAC 音频 TrackInfo

3. 配置：
   ```yaml
   enable_mute_audio: true
   mute_audio:
     codec: aac           # 目前仅支持 AAC
     sample_rate: 44100
     channels: 2
   ```

**约束**：
- 仅在流没有音频轨时激活
- 如果后续收到真实音频轨（如推流端延迟发送音频），停止静音注入
- 静音帧不参与时间戳归一化（已经是正确时间戳）

### 2.2 RTP 重排序缓冲区状态机

**范围**：`cheetah-rtsp-core`（逻辑）+ `cheetah-rtsp-driver-tokio`（timer 驱动）

**参考**：ZLMediaKit `PacketSortor`（`src/Rtsp/RtpReceiver.h`）

**实现方案**：

1. core 层新增 `RtpReorderBuffer`（Sans-I/O）：
   ```rust
   pub struct RtpReorderBuffer {
       // 有界优先队列，按 seq 排序
       buffer: BoundedBinaryHeap<RtpPacket>,
       next_expected_seq: u16,
       max_packets: usize,
   }

   pub enum ReorderOutput {
       /// 包已排好序，可以交付
       Deliver(Vec<RtpPacket>),
       /// 需要设置 timer（超时后强制交付）
       SetTimer { deadline_ms: u64 },
       /// 检测到 seq 重置，清空缓冲区
       SeqReset,
   }

   impl RtpReorderBuffer {
       pub fn push(&mut self, pkt: RtpPacket) -> ReorderOutput;
       pub fn timeout(&mut self) -> ReorderOutput;
   }
   ```

2. core 层 seq 回绕/重置检测：
   - 正常回绕：seq 从 65535 → 0，距离 < 阈值（如 1000）
   - seq 重置：seq 突然跳变到远处（距离 > 阈值），判定为发送端重启
   - 重置时清空缓冲区，重新开始排序

3. driver 层集成：
   - UDP 收包后送入 `RtpReorderBuffer`
   - 根据 `ReorderOutput::SetTimer` 设置 tokio timer
   - timer 触发时调用 `timeout()` 强制交付
   - TCP 传输不经过重排序（TCP 保证有序）

4. 配置：
   ```yaml
   reorder_buffer:
     enabled: true           # 仅 UDP 生效
     max_packets: 64         # 缓冲区上界
     timeout_ms: 100         # 超时强制交付
     seq_reset_threshold: 5000  # seq 跳变超过此值判定为重置
   ```

### 2.3 RTCP-FB NACK 发送/接收

**范围**：`cheetah-rtsp-core`（包模型）+ `cheetah-rtsp-driver-tokio`（重传缓冲）

**参考**：RFC 4585 Generic NACK

**实现方案**：

1. core 层新增 RTCP-FB 包模型：
   ```rust
   /// RFC 4585 §6.2.1 Generic NACK
   pub struct RtcpNack {
       pub sender_ssrc: u32,
       pub media_ssrc: u32,
       pub lost_packets: Vec<NackEntry>,  // PID + BLP
   }

   pub struct NackEntry {
       pub pid: u16,       // 丢失包的 seq
       pub blp: u16,       // 位图，后续 16 个包的丢失状态
   }
   ```

2. driver 层发送端（服务器 PLAY 方向）：
   - 维护最近发送的 RTP 包缓冲（有界环形缓冲区）
   - 收到 NACK 后从缓冲区查找并重传
   - 配置 `nack_buffer_size: 512`（保留最近 512 个包）

3. driver 层接收端（服务器 RECORD/客户端 PLAY 方向）：
   - 检测到 seq gap 时生成 NACK
   - NACK 发送频率限制（避免风暴）
   - 等待重传超时后放弃，交付给上层（带丢包标记）

### 2.4 RTCP-FB PLI/FIR 请求关键帧

**范围**：`cheetah-rtsp-core` + `cheetah-rtsp-module`

**实现方案**：

1. core 层包模型：
   ```rust
   /// RFC 4585 §6.3.1 Picture Loss Indication
   pub struct RtcpPli {
       pub sender_ssrc: u32,
       pub media_ssrc: u32,
   }

   /// RFC 5104 §4.3.1 Full Intra Request
   pub struct RtcpFir {
       pub sender_ssrc: u32,
       pub media_ssrc: u32,
       pub seq_nr: u8,
   }
   ```

2. module 层关键帧请求调度：
   - 新订阅者加入时，如果 GOP cache 为空，发送 PLI 给发布者
   - 收到 PLI/FIR 后，通知发布端请求关键帧
   - 对于 RTSP 推流源：通过 RTCP-FB 转发 PLI
   - 对于引擎内部源：通过 `EngineContext` 请求关键帧
   - PLI 发送频率限制（最小间隔 1s）

### 2.5 RTP seq 回绕与重置检测

**范围**：`cheetah-rtsp-core`

**实现方案**：

已在 2.2 中作为 `RtpReorderBuffer` 的一部分实现。额外补充：

1. 独立的 `SeqTracker` 用于非重排序场景（TCP 传输）：
   ```rust
   pub struct SeqTracker {
       last_seq: u16,
       wrap_count: u32,      // 回绕次数
       total_packets: u64,
       total_lost: u64,
   }

   pub enum SeqEvent {
       Normal,
       Wrap,           // 正常回绕 65535→0
       Reset,          // 发送端重启
       Duplicate,      // 重复包
       OutOfOrder,     // 乱序（TCP 不应出现）
   }
   ```

2. 统计信息用于 RTCP RR 的 `fraction_lost` 和 `cumulative_lost` 计算

## 测试计划

| 测试类型 | 内容 |
|----------|------|
| 单元测试 | MuteAudioMaker 帧时间戳正确性、ReorderBuffer 排序正确性 |
| 属性测试 | 任意 seq 序列经过 ReorderBuffer 后输出有序 |
| 属性测试 | 任意丢包模式下 NACK 生成正确 |
| fuzz | 畸形 RTCP-FB 包解析不崩溃 |
| 集成测试 | UDP 乱序包经重排序后正确交付 |
| 集成测试 | 纯视频推流后拉流包含 AAC 音频轨 |
| 端到端 | PLI 请求后发布端发送关键帧 |

## 完成标准

- [ ] 纯视频 RTSP 推流，拉流端收到 AAC 静音音频
- [ ] UDP 传输下乱序包正确重排序
- [ ] seq 重置后不卡死，自动恢复
- [ ] NACK 重传在可接受延迟内完成
- [ ] PLI 触发关键帧请求，新订阅者快速出画面
- [ ] 所有缓冲区有上界，不会 OOM
