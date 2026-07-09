# Phase 04 — ABL 风格录像、回放与下载兼容

- **状态**: 后续阶段
- **范围**: 把 ABL 的 fMP4 录像切片、录像回放、合并下载和相关配置/hook 纳入 Cheetah，但保持与直播主路径解耦。
- **完成标准**: 录像能力是独立边界，复用 `cheetah-codec` 容器实现，不污染 `cheetah-fmp4-core`。

## 4.1 录像切片

参考 ABL 配置：

- `pushEnable_mp4`
- `fileSecond`
- `videoFileFormat`
- `recordFileCutType`
- `fileKeepMaxTime`

Cheetah 后续设计：

- 新增独立 recording 组件，接收 canonical `AVFrame + TrackInfo`。
- 使用 `Fmp4Muxer` 写单文件 fMP4 segment 文件。
- 支持两类切片策略：
  - 按 wall-clock 时长。
  - 按视频帧数 / 帧率推导时长。
- 每个新文件都带 init segment。

## 4.2 回放

ABL 把 replay 和 live 混在 `NetServerHTTP_MP4`。Cheetah 后续拆开：

- 新建 replay reader path，读取一个或多个 fMP4 切片文件。
- replay 路径按文件内 frame index 或 sample 时间恢复 DTS/PTS。
- replay 会话支持后续 seek / pause / scale 扩展，但不纳入直播模块。

## 4.3 合并下载

ABL 支持 `?download_speed=` 把多段录像文件串成单 HTTP-MP4 下载流。

后续实现要求：

- 只用于录像 download path，不复用 live chunked streaming session。
- 下载路径可选择 `Content-Length` 或有界流式拼接策略。
- 多文件拼接时统一输出可播放的连续下载格式，不破坏 init/segment 顺序。

## 4.4 Hook 与配置

ABL 参考 hook：

- `on_record_mp4`
- `on_delete_record_mp4`
- `on_record_progress`

Cheetah 后续只在 module/control 面定义这些行为：

- 录像段完成通知。
- 录像覆盖通知。
- 录像进度通知。

不把这些通知塞进 `cheetah-codec` 或 `cheetah-fmp4-core`。

## 4.5 风险

- live/replay 时间戳路径不同，必须显式区分。
- 多文件下载若直接拼裸字节，容易破坏播放器兼容性。
- 录像存储和回放会引入新的文件 I/O、索引和 retention 策略，需要独立计划验证。
