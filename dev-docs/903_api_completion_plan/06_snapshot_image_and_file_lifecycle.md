# 06 · 快照、图片编码与文件生命周期

## 1. 编码接口

新增 runtime-neutral `ImageEncodeApi::encode(ctx, ImageEncodeRequest) -> ImageArtifact`。请求包含 `Arc<AVFrame>`、`TrackInfo`、`ImageFormat::{Jpeg,Png}`、quality 1–100、可选最大宽高；结果包含 `Bytes`、content_type、format、width、height。

规则：MJPEG 到 JPEG 可在验证完整帧后透传；其他 codec 必须经实际解码/编码 backend。backend 未编译、未启动或不支持 codec 时返回 Unsupported/Unavailable，Snapshot 不得落盘。不得将原始 H264/H265/VP8 字节命名为 jpg 或作为 Completed。

图片 backend 放在 system/runtime adapter，不把 FFmpeg C 类型或进程类型泄漏进 SDK。编码任务受 deadline/cancellation 和并发 semaphore 限制。

## 2. 原子生命周期

流程固定为：验证请求 → 订阅并等待可用关键帧 → 编码 → 校验 magic/尺寸 → 写同目录临时文件 → flush/rename → 注册 `FileHandle` → 登记 snapshot → 发布 `SnapshotCompleted`。任一步失败必须清临时文件和半成品元数据，事件只发布一次。

文件 API 只接受内部 handle。物理删除只能作用于已登记且位于配置 managed root 下的 canonical path；符号链接、路径逃逸和非本 owner 文件拒绝。

增加：

```rust
struct DeleteBatchResult {
    matched: u64,
    deleted: u64,
    failed: u64,
    failures: Vec<DeleteFailure>,
}
```

新增 `delete_snapshots` 返回该结果；旧 `delete_snapshot_directory` 委托新方法并在 `failed > 0` 时返回错误，一个版本后删除。成功项同时删除物理文件、file-store 条目和 snapshot registry。

## 3. HTTP 与事件

下载必须使用登记的 MIME、Content-Length 和安全文件名，支持授权后的 GET，不直接暴露服务器路径。删除接口返回 batch result；部分失败用 200 加逐项结果，整个请求非法才使用 4xx。

`SnapshotCompleted` 只携带 handle、媒体键、格式、尺寸、大小和时间，不携带绝对路径。

## 4. 任务与验收

- `IMG-01`：定义并注册 ImageEncodeApi，接入至少一个真实 backend。
- `IMG-02`：重写 snapshot 原子提交和失败清理。
- `IMG-03`：实现 handle 受控物理删除与批量结果。
- `IMG-04`：native/兼容下载、删除和错误映射。

测试覆盖 MJPEG、H264/H265（backend 支持时）、无 backend、非关键帧等待、超时、并发上限、写失败、路径逃逸、部分删除失败。每个成功 JPEG 必须检查 magic 并由独立 decoder 解码。

