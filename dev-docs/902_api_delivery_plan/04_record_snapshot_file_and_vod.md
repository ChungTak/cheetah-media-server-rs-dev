# 04 · Record、Snapshot、File 与 VOD

## 1. Record provider

保留现有 record module 作为唯一录制 provider，补齐以下语义：

- `start_record` 使用 MediaKey、format、template、storage policy 和 idempotency key。
- 相同 idempotency key 与相同有效请求重复调用时返回原 task；参数不同则 Conflict。
- `stop_record` 对已完成 task 返回最终 task，不重复生成文件。
- task/file query 在 provider 内完成稳定排序和分页，不能先无界收集再分页。
- record task 必须保存完整 MediaKey，包括 vhost；不得用 `__fallback__` 构造停止结果。
- module stop 有界停止任务并生成明确 terminal state。

支持格式由 capability 声明，至少覆盖当前真实可写格式；未启用 writer 时返回 Unsupported。

## 2. VOD 与 playback control

record playback 由 MP4/VOD module 提供独立 `PlaybackApi`，record provider 只桥接：

- `pause`、`resume` 无 value。
- `scale` 要求有限正数范围，默认建议 `0.25..=16.0`。
- `seek` 使用显式毫秒时间，超出 duration 返回 InvalidArgument。
- file/task 必须处于可播放状态。
- playback session 注册到 session directory，可查询和关闭。

不得在 record module 内重写 MP4 demux、时间戳或参数集处理。

## 3. Snapshot provider

新增 `cheetah-snapshot-module` 或等价职责清晰的 system module，注册真实 SnapshotApi。流程固定为：

1. 解析并授权 MediaKey。
2. 打开有界 frame subscriber。
3. 等待已就绪视频轨和随机访问帧。
4. 在 deadline 内调用注入的 image encoder/FFmpeg service。
5. 原子写入受管理文件存储。
6. 注册 SnapshotInfo 和 FileHandle。
7. 发布 SnapshotCompleted 或失败事件并关闭 subscriber。

无视频轨、关键帧超时、编码器缺失、磁盘满分别返回 Unsupported/Timeout/Unavailable/StorageFailed，不能统一为 Internal。

## 4. File store

新增 runtime-neutral `MediaFileStoreApi`：

- `register_file` 返回不可猜测的 FileHandle。
- `resolve_for_read` 检查 principal、resource scope、过期时间和文件状态。
- `delete` 只删除 registry 内资源，禁止接受任意绝对路径。
- 下载支持 content type、长度、可选 range 和安全 filename。
- record/snapshot payload 中不暴露服务器绝对路径。

目录删除转换为按 MediaKey/时间/类型查询后的批量删除；每批有上限并返回成功/失败计数。

## 5. 事件

- task 启动后发布 RecordStarted。
- segment flush 或固定间隔发布 RecordProgress，频率有上限。
- 文件完全关闭并进入 registry 后发布 RecordCompleted。
- 快照原子写入后发布 SnapshotCompleted。
- webhook 失败不得回滚已完成媒体文件。

## 6. 路由映射

native 必须完成 record tasks/files/playback、snapshots、file download。ZLM 必须完成 start/stop/isRecording/getMP4RecordFile/deleteRecordDirectory、setRecordSpeed、seekRecordStamp、controlRecordPlay、getSnap、deleteSnapDirectory、downloadFile；`loadMP4File` 委托 VOD capability。

## 7. 任务与验收

| ID | 任务 | DoD |
| --- | --- | --- |
| S3-T1 | record 幂等和 vhost | 重复请求稳定、无 fallback key |
| S3-T2 | 分页与存储上界 | 大文件索引不无界加载 |
| S3-T3 | playback provider | 四命令真实改变播放状态 |
| S3-T4 | snapshot provider | 在线视频生成可解码图片 |
| S3-T5 | file store/download | 授权、过期、range、安全路径 |
| S3-T6 | record/snapshot events | 顺序和幂等可测 |

```bash
cargo test -p cheetah-record-module
cargo test -p cheetah-snapshot-module
cargo test -p cheetah-mp4-module
cargo test -p cheetah-media-module record
cargo test -p cheetah-media-module snapshot
```

若 snapshot crate 最终合并到现有职责相符的 module，命令按实际 package 调整并同步本文档。

