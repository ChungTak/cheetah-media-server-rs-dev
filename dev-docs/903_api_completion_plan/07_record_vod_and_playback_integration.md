# 07 · 录制、VOD 与回放整合

## 1. Domain API

新增独立 `PlaybackApi`：

```rust
async fn open(ctx, OpenPlaybackRequest) -> Result<PlaybackSession>;
async fn get(ctx, &PlaybackSessionId) -> Result<PlaybackSession>;
async fn list(ctx, PlaybackQuery) -> Result<Page<PlaybackSession>>;
async fn control(ctx, &PlaybackSessionId, PlaybackControl) -> Result<PlaybackSession>;
async fn stop(ctx, &PlaybackSessionId) -> Result<()>;
```

`OpenPlaybackRequest` 使用 `FileHandle`、目标 `MediaKey`、`start_position_ms`、`scale`；不接受任意路径。control 是 `Pause/Resume/Seek{position_ms}/SetScale{scale}`。scale 首期固定支持 0.5、1、2、4，其他值 Unsupported。session 包含状态、duration、position、scale、generation、output_key、last_error。

## 2. 实现归属

由 MP4 module 实现 PlaybackApi，复用现有 MP4 Sans-I/O VOD session、Tokio file driver、session registry 和 engine bridge。不得在 record module 再实现 parser、时间调度或内存播放器。

Record provider 负责：查询 `RecordFileId`、校验 owner/授权、解析为 `FileHandle`，然后调用 PlaybackApi。旧 `control_record_playback(file_id, command)` 保留一个版本：首次调用为该文件创建或查找兼容 session，再委托 control。

MP4 module start 后注册 Playback provider 和 capability，stop/restart 时先取消 session、撤销 publisher、释放 handle，再注销 provider。输出继续遵守单发布者租约；目标 key 已占用返回 Conflict。

## 3. 正确性

- 文件由 file-store 授权打开；删除中的文件不能新建回放。
- seek 由 core 重建 sample cursor，时间戳以新 discontinuity 输出，不复用旧 DTS。
- pause 不读新 sample；resume 不突发补发积压帧；scale 只改变调度，不改媒体 timebase。
- EOF 转 Completed 并清 publisher；格式损坏转 Failed，保留稳定错误。
- deadline 只约束 open/control 请求；session 生命周期由显式 stop、EOF、module stop 管理。

## 4. 任务与验收

- `VOD-01`：Domain 类型、MediaServices slot、facade 和 capability。
- `VOD-02`：MP4 provider 接线及 record compatibility shim。
- `VOD-03`：native/兼容 playback 路由。
- `VOD-04`：真实 MP4 文件 E2E。

E2E 必须断言 track、帧顺序、暂停期间无帧、seek 后时间位置、倍率调度、EOF、stop 和 restart 清理。仅检查 position 字段变化不合格。

