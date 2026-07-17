# 10 · 协议集成

## 1. 通用规则

协议 module 只通过 `MediaProcessingApi` 请求或复用派生流，不直接依赖 avcodec-rs。protocol core/driver 不新增处理依赖。

每次自动处理：

1. 完成播放/代理 admission 和原始流授权。
2. 根据协议输出矩阵解析 `ProcessingPolicy`。
3. `Passthrough` 只使用原流。
4. `Auto/Transcode` 请求处理 provider，以规范化 spec 查找共享任务。
5. 等待派生 Track Ready，再启动协议输出。
6. session 结束释放共享引用；不能直接 stop 仍被其他消费者使用的任务。

## 2. Snapshot

- Snapshot module 只依赖 `ImageProcessApi`。
- MJPEG/H.264/H.265 关键帧输出 JPEG。
- provider 缺失返回 `Unavailable`；codec/profile/PNG 输出缺失返回 `Unsupported`。
- capability/URL 只在 Snapshot provider 与 ImageProcessing/JPEG operation 都可用时对外声明。

## 3. RTMP 与 HTTP-FLV

- 输出目标固定支持 H.264/AAC 主路径。
- G711/Opus 音频在 `Auto/Transcode` 下请求 AAC 派生流。
- H.265/MJPEG 视频需要 H.264 时请求视频派生流；禁止在 RTMP module 内解码。
- disable audio/video 迁移为 `TrackSelection`，纯过滤不创建转码任务。
- HTTP-FLV 与 RTMP 复用同一派生 StreamKey 和 codec config，不各建一套任务。
- 没有处理 capability 时 `Auto` 忽略不兼容轨并报告 diagnostic；`Transcode` 失败。

## 4. WebRTC

- `codec_policy` 不再接收硬编码 `transcode_available=false`，而是读取 operation capability。
- browser audio：AAC/G711/MP3 → Opus。
- browser video：不兼容 codec 在策略允许时转为协商出的 H.264；已兼容 H.264 保持 passthrough。
- 当前 `Auto` 在音频不可处理时继续保留视频并丢弃音频；显式 `Transcode` 必须失败。
- PLI/FIR 转发到派生 encoder keyframe request。
- 同源同目标的 WHEP sessions 复用一个派生任务。
- 多档独立 H.264 派生流可供层选择；不宣称 SVC。

## 5. Pull Proxy

- 删除 `FfmpegProxyRequest` 和 FFmpeg proxy operation。
- `PullProxyRequest.transcode_policy` 替换为类型化 `processing_policy`。
- 拉流成功并发现 Ready tracks 后才规范化处理 spec；随后创建显式目标派生流。
- 若 destination 同时是 pull ingress 和处理 output，内部为 ingress 分配保留临时 StreamKey，避免两个 publisher 争用。
- proxy stop 顺序为停止外部拉流 → drain processing Job → 删除临时 ingress；失败路径同样清理。
- `Passthrough` 维持现有 RTSP pull/RTMP push 数据路径。

## 6. HLS/LL-HLS

- HLS 不创建 encoder，只消费显式 ABR/Caption Job 输出。
- master playlist 只引用 Running 且 Track Ready 的 variants。
- 任一显式 ladder 失败时 master 不继续广告残缺梯度。
- WebVTT 与 reference video segment 对齐，并使用同一鉴权/输出 registry。
- processing feature 关闭时，现有单流 HLS 和手工配置的外部 variant 行为不变。

## 7. 必需 E2E

| ID | 输入 | 处理 | 输出/消费者 |
| --- | --- | --- | --- |
| E2E-IMG | H.264/H.265/MJPEG live | keyframe → JPEG | Snapshot API + 独立 JPEG decoder |
| E2E-FLV | G711 或 Opus live | audio → AAC；必要时 video → H.264 | RTMP 与 HTTP-FLV player |
| E2E-WEB | SRT/RTSP H.264 + AAC/MP3 | audio → Opus | Chrome WHEP，真实 inbound audio |
| E2E-PRX | RTSP pull H.265/G711 | H.264/AAC 派生流 | RTMP/HLS consumer |
| E2E-HLS | 单路高清 + CEA | 三档 H.264 + WebVTT | HLS master 切档和字幕播放 |

测试外部 FFmpeg/ffprobe 只作为客户端/验证器，不由生产服务启动。

## 8. 完成标准

- [ ] 五条 E2E 使用 production provider 和真实协议对端。
- [ ] feature 关闭、provider 缺失、profile 不支持三种情况行为可区分。
- [ ] 自动任务正确复用并在最后 session grace 后清理。
- [ ] 协议 module 无 avcodec、FFmpeg、Tokio 新依赖，core 保持 Sans-I/O。
