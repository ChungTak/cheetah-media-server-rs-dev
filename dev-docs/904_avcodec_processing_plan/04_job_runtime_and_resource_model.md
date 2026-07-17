# 04 · 任务、运行时与资源模型

## 1. RuntimeApi 扩展

增加 object-safe、runtime-neutral 的 blocking task 入口：

```rust
fn spawn_blocking(
    &self,
    name: &str,
    task: Box<dyn FnOnce() + Send + 'static>,
) -> Result<Box<dyn JoinHandle>, RuntimeError>;
```

Tokio adapter 映射到 `tokio::task::spawn_blocking`。公共 handle 继续使用 SDK 自有 join/cancel/outcome 类型；不得暴露 Tokio handle。取消通过共享 runtime-neutral token 通知 worker，join 负责等待资源释放。

## 2. Worker 所有权

- 每个 avcodec decode/process/encode graph 从创建到 drop 固定在一个 blocking worker。
- 异步订阅任务只把 `Arc<AVFrame>` 放入有界输入队列；worker 输出压缩 `AVFrame` 到有界返回队列。
- 任何 session 方法不得跨 worker 或被 mutex 包裹后并发调用。
- `Pending` 触发继续 poll 或等待下游空间，不计入 error。
- EOS 顺序为停止接收 → drain input → flush decoder/processor/encoder → publish terminal discontinuity → 释放 publisher。
- reset 只用于声明为可恢复的 discontinuity 或任务重启；reset 后必须重新输出 codec config 和随机访问点。

## 3. Job 生命周期

1. adapter 校验 principal、deadline、idempotency 和结构上限。
2. 调用 `MediaAdmissionApi::authorize(CreateProcessingJob)`。
3. 执行 capability/preflight，不创建任何租约。
4. 预留全局资源配额。
5. 打开所有 source subscriber。
6. 原子获取所有 target publisher lease；任一冲突则回滚。
7. 启动 worker，状态进入 `Starting`；首个有效输出和 Track Ready 后进入 `Running`。
8. stop/cancel/source terminal 进入 `Draining`，完成 flush 和清理后进入 `Stopped`。
9. 不可恢复错误进入 `Failed`，但仍必须先释放 subscriber、publisher、worker 和配额。

同一 idempotency key + 同一 spec 返回同一 Job；同 key 不同 spec 返回 `Conflict`。

## 4. 显式与共享任务

- API/配置创建的显式任务持久存在，直到 stop/delete 或配置移除。
- 协议 `Auto` 创建内部共享任务，canonical fingerprint 覆盖源 StreamKey、输入轨、目标 codec/profile、尺寸、码率、帧率、音频参数和全部 filter。
- 内部目标为 `StreamKey::new("__cheetah_derived", hex_sha256(fingerprint))`。
- registry 对共享任务计数；最后消费者释放后等待默认 10 秒 grace，再取消任务。grace 内新消费者复用原任务。
- 相同源但不同 overlay、码率或 profile 绝不复用。
- 用户不能向保留 namespace 发布，也不能通过公共 Job API读取其他 principal 的内部任务。

## 5. 资源上限

模块配置必须提供并校验：

| 限制 | 默认值 |
| --- | --- |
| 最大活跃任务 | 8 |
| 最大排队任务 | 32 |
| 单任务视频输入队列 | 8 frames |
| 单任务音频输入队列 | 64 frames |
| 单任务输出队列 | 16 frames |
| 单 ABR 梯度 | 4 variants |
| AudioMix 输入 | 16 |
| VideoMosaic 输入 | 9 |
| 单帧最大像素 | 3840 × 2160 |
| 总视频像素率 | 124,416,000 pixels/s |
| 单图片输入 | 32 MiB |
| 自动任务 grace | 10 s |

默认值可配置降低或提高，但不能设为无上限。资源 reservation 在 publisher/subscriber 前完成，释放使用 scope guard 保证所有错误分支一致。

## 6. Backpressure

- 视频队列满时先丢 droppable delta frame，等待下一随机访问点；不得丢 codec config 后继续输出不可解码 delta。
- 音频优先短暂背压；达到上限后按完整 frame 丢弃并输出 discontinuity，禁止截断 packet。
- 慢派生输出不能阻塞源 Dispatcher 或其他订阅者。
- mixer/mosaic 使用任务输出时钟；过晚输入丢弃，短缺音频补静音，视频使用最近帧，超过 stale threshold 后填黑。
- 所有 drop、queue high-watermark 和重同步进入 metrics。

## 7. 故障与恢复

- 源流暂时不存在时 Job 保持 `Pending`；deadline 到期转 `Failed`。
- 已运行源流中断时，配置任务按 `250 ms → 500 ms → 1 s → 2 s → 5 s` 有界退避重连，默认最多 5 次；API 临时任务默认失败不重启。
- preflight/codec/非法 bitstream 错误不重试。
- module restart 由基础生命周期重建 provider；旧 generation registration 不得注销新 provider。
- shutdown 先阻止新 Job，再取消内部自动任务和显式任务，等待 bounded drain，超时后标记 Failed 并强制释放 Cheetah 资源。

## 8. 验收

- [ ] blocking worker 不占用协议/runtime async 热路径。
- [ ] 每个错误点都能证明配额、subscriber、publisher 和 worker 数回到基线。
- [ ] Auto 任务同 spec 复用、异 spec 隔离、grace 后删除。
- [ ] 慢消费者、连续 Pending、源流重启和 cancel 不死锁、不无界增长。
- [ ] resource leak report 包含处理 Job/worker/派生流，并在 shutdown 后为空。
