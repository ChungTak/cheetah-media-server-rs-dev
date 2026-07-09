# Phase 06 — 快照/截图 + 视频水印

- **状态**: 未开始
- **范围**: I 帧解码输出 JPEG/PNG、视频文字叠加
- **完成标准**: 可通过 API 获取任意直播流的截图；可配置文字水印

---

## 6.1 快照/截图 API

**功能**: 对指定直播流截取当前画面，输出 JPEG 或 PNG。

**API**:
```
POST /api/snap
{
  "stream_key": "live/test",
  "format": "jpeg",
  "width": 640,
  "height": 360,
  "timeout_ms": 5000
}
→ 200 OK (image/jpeg body)
```

**实现设计**:
- 订阅目标流，等待下一个关键帧
- 使用视频解码器（FFmpeg 绑定或纯 Rust 解码器）解码 I 帧
- 缩放到目标分辨率
- 编码为 JPEG/PNG 输出
- 超时返回 408

**架构归属**: 独立模块 `cheetah-snap-module`，通过 feature flag 可选编译。

---

## 6.2 视频水印/文字叠加

**功能**: 在视频流上叠加文字（时间戳、频道名等）。

**实现设计**:
- 需要完整的解码→滤镜→编码管线
- 依赖 FFmpeg 绑定（`ffmpeg-next` crate）
- 作为转码模块的扩展功能
- 通过 API 动态配置水印参数

**优先级**: 低。依赖转码基础设施（Phase 02），建议在转码模块成熟后再实现。
