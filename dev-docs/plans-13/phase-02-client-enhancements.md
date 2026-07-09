# Phase 02 — RTMP 客户端增强

- **状态**: 未开始
- **范围**: 音视频选择性禁用、302 重定向、推流参数集帧跳过
- **完成标准**: Pull/Push job 支持过滤音视频轨；拉流支持 302 跟随

---

## 2.1 拉流/推流音视频选择性禁用

**问题**: 某些场景只需要视频（如监控截图）或只需要音频（如对讲），需要过滤不需要的轨道。

**ABLMediaServer 方案**: `disableVideo`/`disableAudio` 参数控制。

**本地实现方案**:

在 Pull/Push job 配置中增加过滤选项：

```yaml
modules:
  rtmp:
    pull_jobs:
      - name: camera_video_only
        source_url: rtmp://source/live/cam1
        target_stream_key: live/cam1
        disable_audio: true   # 不拉取音频
    push_jobs:
      - name: audio_only_push
        source_stream_key: live/mic1
        target_url: rtmp://target/live/mic1
        disable_video: true   # 不推送视频
```

**实现位置**: `cheetah-rtmp-module` pull/push job 管线，在帧分发前按 `MediaKind` 过滤。

---

## 2.2 RTMP 拉流 302 重定向支持

**问题**: 部分 CDN 或源站在 RTMP connect 阶段返回 302 重定向，客户端需要跟随。

**ABLMediaServer 方案**: 检测 RTMP redirect 响应并重新连接到新地址。

**本地实现方案**:

在 RTMP 客户端连接流程中，检测 `_result` 中的 redirect 信息：

```rust
// 在 ClientStateChanged 处理中
// 如果 connect _result 包含 "redirect" 或 "ex.redirect" 字段
// 提取新 URL，关闭当前连接，重新连接到新地址
```

**限制**: 最多跟随 3 次重定向，防止循环。

**实现位置**: `cheetah-rtmp-module` pull job 连接逻辑。

---

## 2.3 推流客户端参数集帧跳过

**问题**: 某些编码器在 I 帧前单独发送 SPS/PPS 小帧，而 I 帧本身也包含 SPS/PPS，导致重复。

**ABLMediaServer 方案**: 跳过 <2048 字节的纯 SPS/PPS I 帧（因为后续 I 帧已包含）。

**本地实现方案**:

在 RTMP push job 发送管线中，检测并跳过纯参数集帧：

```rust
fn should_skip_redundant_config_frame(frame: &AVFrame) -> bool {
    frame.media_kind == MediaKind::Video
        && frame.flags.contains(FrameFlags::KEY)
        && frame.payload.len() < 2048
        && is_parameter_set_only(&frame.payload)
}
```

**实现位置**: `cheetah-rtmp-module` push job 发送循环。
