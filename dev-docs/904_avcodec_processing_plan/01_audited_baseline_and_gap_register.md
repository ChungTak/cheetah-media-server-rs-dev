# 01 · 审计基线与差距登记

## 1. 当前源码事实

| ID | 当前状态 | 本期动作 |
| --- | --- | --- |
| GAP-DEP-01 | Workspace 直接依赖 `image`；快照生产代码和测试使用它 | 删除直接依赖，统一走 avcodec-rs |
| GAP-IMG-01 | `ImageEncodeApi` 只描述 encode；快照只可靠支持 MJPEG | 升级为完整 `ImageProcessApi`，打通 H.264/H.265/MJPEG → JPEG |
| GAP-AUD-01 | `cheetah-codec::transcode` 定义 G711 查表、AAC/Opus traits 和 pipeline，但无生产 encoder/decoder | 删除执行器与临时重采样，处理模块使用 avcodec-rs |
| GAP-API-01 | `TranscodePolicy` 的宽高、G711→AAC、H.264 重编码字段未被生产路径消费 | 破坏性替换为类型化 `ProcessingPolicy`/`ProcessingJobSpec` |
| GAP-WEB-01 | WebRTC resolver 支持“可转码”决策，但桥接固定传入不可用 | 接入共享派生任务，完成 AAC/G711/MP3 → Opus |
| GAP-ABR-01 | HLS 能输出引用显式 StreamKey 的 master playlist，但服务器不生成码率梯度 | 处理模块发布每档派生流，HLS 复用现有 variant 配置 |
| GAP-CAP-01 | `MediaCapability::ImageEncode` 只有泛化 `encode` operation | 拆分 Audio/Video/Image Processing 能力和 operation |
| GAP-FFM-01 | SDK、Engine、Proxy、Native/ZLM adapter 维护 FFmpeg 进程 API | 本期彻底删除，不保留兼容 shim |
| GAP-RUN-01 | `RuntimeApi` 无 CPU/阻塞任务入口 | 增加 runtime-neutral `spawn_blocking` |
| GAP-SUB-01 | `MediaKind::Subtitle` 已存在，但无 WebVTT codec、CEA parser 和 HLS VTT muxer | 增加规范化字幕模型和真实 HLS 输出 |

## 2. 历史任务提取

以下任务明确因为缺少音频、视频或图片处理后端而延期，本期重新编号并纳入执行：

| 新任务 | 历史来源 | 固定交付 |
| --- | --- | --- |
| AUD-01 | `plans-12`、`plans-15` | G711A/U → AAC，服务 RTMP/HTTP-FLV/HLS |
| AUD-02 | `plans-14`、`plans-15` | AAC ↔ G711A/U、AAC ↔ Opus、G711 ↔ Opus |
| AUD-03 | `plans-28-srt` | AAC/MP3 → Opus，完成浏览器 WebRTC 音频 |
| IMG-01 | `plans-12`、903 IMG | 视频关键帧解码、缩放和 JPEG 快照 |
| VID-01 | `plans-12` | decode-filter-encode、图片/文字水印 |
| VID-02 | `TranscodePolicy` 当前公共契约 | 宽高、帧率、码率和 H.264/H.265 重编码 |
| ABR-01 | `plans-19`、`plans-21` | 服务端生成显式多码率派生流 |
| MIX-01 | `SystemArchitecture.md` Phase 4 | 有界音频混音与固定宫格 |
| SUB-01 | `plans-21-llhls-ome` | CEA-608/708 提取、WebVTT track 和 HLS packager |

历史文档中的 FFmpeg、fake backend 或“未来转码模块”方案不继承，只继承业务目标和互操作验收。

## 3. avcodec-rs 审计事实

审计基线为 avcodec-rs `0.2.0`、revision
`dd3190008f2b544b51a74a9f4a225d52befc120a`；实际集成必须更新为包含 MP3 上游工作的精确 merge revision。

- 官方集成入口是顶层 `avcodec`，视频使用 `VideoSdk + VideoProfile` 高层 V3 API。
- codec-only，不提供协议、容器、网络或 Cheetah 任务生命周期。
- session 使用 submit/poll/flush/reset；`Pending` 是正常背压状态，不是错误。
- session 必须由单一 worker 所有，不允许并发调用。
- `profile-native-free` 与 `profile-software` 可独立启用；默认 features 必须关闭。
- 音频稳定接口当前为 `AudioTranscoder + Registry`，尚无对等 `AudioSdk`；Registry 只能作为处理模块私有实现细节。
- 稳定图片算子包括 crop、resize、crop-resize、CSC、rotate、flip、pad、blend、OSD 和 resize-pad。
- JPEG 编码可用；PNG 解码可用但没有稳定 PNG 编码实现。
- 当前 `CodecId` 没有 MP3，故 AUD-03 的 MP3 分支必须先补上游。

## 4. 本期不做

- 不把解码后的 YUV/PCM 发布为 Engine 公共媒体帧。
- 不在协议 core、driver 或 `cheetah-codec` 中持有 avcodec session。
- 不自动创建未在配置或协议策略中允许的 CPU 转码。
- 不实现硬件 profile、GPU memory-domain 交付或零拷贝承诺。
- 不实现 PNG 输出、任意图层动画、动态模板文字或 ML Normalize。
- 不实现真正 SVC bitstream；多层输出使用独立编码派生流。
- 不把 DRM、DVR、SCTE-35/CUE、录制封装或无关协议重写夹带进本阶段。

## 5. 差距关闭规则

每个 GAP 只有在以下证据同时存在时才能关闭：

1. production provider 已注册且 capability operation 真实可调用；
2. 至少一个真实输入经数据面产生可被独立消费者验证的输出；
3. Unsupported、Unavailable、资源耗尽和取消路径有负向测试；
4. 默认 feature 构建仍不包含该可选能力；
5. 文档和发布证据记录具体命令、revision、profile 与制品。
