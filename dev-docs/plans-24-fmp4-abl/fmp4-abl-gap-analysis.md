# fMP4 与 ABLMediaServer 差距分析

- **状态**: 规划中
- **范围**: 记录 ABL C++ 实现和 `版本信息.txt` 中与 fMP4 直接相关的行为、当前 Cheetah 状态和后续缺口。
- **完成标准**: 后续阶段能按本文逐项补齐直播兼容、时间戳、录像后续阶段和互操作测试。

## ABL 关键行为

| 领域 | ABL 参考 | 行为 |
|------|----------|------|
| HTTP-MP4 直播 | `NetServerHTTP_MP4.cpp` | `Content-Type: video/mp4` + `Transfer-Encoding: chunked`，先 init 后 fragment |
| fMP4 录像 | `StreamRecordFMP4.cpp` | 关键帧起播，写 init segment，再持续写 fMP4 片段到单文件 |
| H26x 处理 | `VideoFrameToFMP4File` | Annex-B 转 MP4 length-prefixed；参数集未就绪时不输出 |
| 音频处理 | `SendAudio` | AAC/G711 为主；AAC 从 ADTS 抽 ASC，G711 直接写入 |
| 时间戳 | `版本信息.txt` | 多次修正真实帧率、音视频同步、回放时间戳计算 |
| 快速起播 | `ForceSendingIFrame` | 播放新连接时优先推最近 I 帧缓存 |
| 录像切片 | `fileSecond` / `recordFileCutType` | 按 wall-clock 或帧数/帧率切割新文件 |
| 合并下载 | `download_speed` | 多段录像拼接成单 HTTP-MP4 下载流 |

## 版本信息里与 fMP4 直接相关的结论

重点变化：

1. **2026-03-27**：
   MP4/fMP4 写入改为使用真实帧率，不再依赖“无音频才按帧率算”的旧条件。
2. **2025-12-30 / 2025-12-24**：
   录像回放时视频和音频 DTS/PTS 单独修正，live 与 replay 走不同时间戳路径。
3. **2025-12-26**：
   音视频同步逻辑按 500ms 周期调整 `nVideoStampAdd`，说明 ABL 很看重实况 A/V 漂移修正。
4. **2025-09-30**：
   引入 H265 fMP4 切片优化，并把大块 HTTP-MP4/HLS 发送缓冲提升到 `256 KiB`。
5. **2025-05-22**：
   修正 fMP4 切片完成通知中的结束时间。
6. **2025-04-01**：
   增加多段录像合并下载 URL：`?download_speed=`.

这些结论意味着：Cheetah 后续不仅要“能封装 fMP4”，还要把 live/replay 时间戳、关键帧起播和录像切片边界当作明确设计点。

## 当前 Cheetah 状态

| 能力 | 当前状态 | 备注 |
|------|----------|------|
| 三段式 crate | 已有 | `core/driver-tokio/module` 已在 workspace |
| fMP4 mux/demux | 已有 | `cheetah-codec` 已支持 `styp/sidx/moof/mdat`、多编码、多轨 |
| HTTP `.mp4` / `.live.mp4` | 已有 | core 支持解析、HEAD、OPTIONS、WebSocket upgrade |
| HTTPS/WSS server | 已有 | `tls.rs` 已能加载 PEM 并接入 driver |
| HTTP chunked pull | 已有 | pull client 已解码 chunked body |
| WebSocket Accept / mask / continuation | 已有基础 | 仍需要补更多集成与边界测试 |
| 关键帧起播 | 已有 | module 在有视频时等待关键帧 |
| track/config 变化重发 init | 已有基础 | 还需要更细的 ABL 兼容测试 |
| 录像切片/回放/下载 | 缺失 | 当前没有 ABL 风格 DVR 能力 |
| GOP/I 帧缓存快速起播 | 缺失 | 当前只依赖 live tail + 等关键帧 |
| live/replay 双时间戳路径 | 缺失 | 当前只有直播 fMP4 主路径 |

## 已完成但需要修正文档认知的点

以下能力在此前计划中被当作“缺口”，但实际已实现：

1. `include_sidx=true` 已实际写出 `sidx`，并有 mux/demux 测试。
2. fMP4 TLS server 已可启动，不再是 `tls.rs` 占位。
3. pull client 已正确读取 HTTP chunked body。
4. WebSocket pull 已校验 `Sec-WebSocket-Accept`，且 client mask key 已随机化。

ABL 计划应从这些已实现能力继续往前走，而不是重复立项。

## 必须补齐的缺口

1. 对齐 ABL 的真实帧率驱动时间戳策略，并补专门测试。
2. 增加最近关键帧/GOP bootstrap 策略，缩短新观看者起播时间。
3. 把 ABL 风格 HTTP-MP4 大块 chunk 发送、慢连接隔离做成明确 driver 策略。
4. 区分 live 与 future replay 的时间戳路径，避免把 replay 语义硬塞进 live session。
5. 规划并实现 fMP4 录像切片、回放、合并下载的独立后续阶段。
6. 增加 ABL 回归样例：H265 fMP4、AAC/G711 音频、重复 init、参数集晚到。

## 兼容矩阵结论

应保持的编码兼容：

| 编码 | 输出 entry | 输入兼容 | 说明 |
|------|------------|----------|------|
| H264 | `avc1` | `avc1/avc2/avc3/avc4` | 参数集就绪后输出 |
| H265 | `hvc1` | `hvc1/hev1/dvh1/dvhe` | ABL 特别强调 H265 fMP4 切片优化 |
| AAC | `mp4a + esds(0x40)` | `mp4a` | 从 ADTS/ASC 构建配置 |
| G711A | `alaw` | `alaw` | 弱标准但必须兼容 |
| G711U | `ulaw` | `ulaw` | 弱标准但必须兼容 |
| MP3 | `mp4a + esds(0x69)` | `0x69/0x6B` | 输入兼容两类 object type |
| MP2 | `mp4a + esds(0x6B)` | `0x6B` | 主要是弱播放器兼容 |
| MJPEG | `mp4v + esds(0x6C)` | `mp4v/jpeg/mjpa/mjpb` | ABL 风格弱标准路径 |
| Opus | `Opus + dOps` | `Opus` | 浏览器兼容由播放器决定 |

## 风险点

- ABL 录像逻辑与直播逻辑耦合很深，直接照搬会污染三段式边界。
- 真实帧率动态变化会影响 fragment duration、音视频同步和 replay seek 计算。
- 强制最近 I 帧快速起播需要和单发布者、慢订阅者隔离、bootstrap policy 协调。
- 录像下载合并流不能复用直播长连接路径，否则 live/replay 语义会混乱。
