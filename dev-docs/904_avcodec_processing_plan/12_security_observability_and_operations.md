# 12 · 安全、观测与运维

## 1. 授权与输入安全

- `CreateProcessingJob` 在任何配额、subscriber、publisher、worker 或幂等成功记录前调用 admission。
- principal 必须同时拥有全部 source 的 play 权限和全部 target 的 publish 权限。
- list/get/update/stop/delete 过滤到 owner 或显式 resource grant。
- 保留 namespace `__cheetah_derived` 禁止外部 publish 和直接 Job target。
- logo/font/image 只能通过授权 FileHandle；禁止任意路径、URL、DNS 和 shell。
- encoded bytes、图片尺寸、像素率、字体大小、文字长度、overlay 数、输入数和 JSON 深度全部有上限。
- 错误和日志不得输出完整媒体 payload、密钥、URL credential 或字体内容。

## 2. Deadline、幂等与取消

- adapter 和 provider 入口都检查 deadline；等待 source、preflight、lease 和 worker startup 均计算剩余时间。
- create 使用现有指纹幂等 repository；同 key 同 spec 重放，同 key 异 spec Conflict。
- update 使用 expected generation；先 preflight next spec，再原子切换。
- cancel token 贯穿 subscriber、worker、publisher 和共享任务引用。
- stop/delete 可重复调用；终态重放不创建新资源。

## 3. Preflight 与 capability honesty

启动时对已编译 profile 执行：

- backend/profile 是否启用
- required codec decode/encode
- image operator/JPEG encode
- audio resample/channel adapt
- flush/reset
- memory domain 和 buffer path

`ProcessingPreflightReport` 记录 revision、feature、profile、operation、available、selection 和 reason。单 operation 失败只移除该 operation；核心 provider 无法初始化时 module startup 失败。Health 显示 Disabled、Ready 或 Degraded，不把 feature 未编译当作故障。

## 4. 结构化日志

Job 生命周期日志至少包含：

- job id、kind、owner、generation
- avcodec revision、profile、codec、尺寸、pixel/sample format
- backend selection 摘要和 memory domain
- source/target StreamKey（按日志脱敏规则）
- startup/first-output/drain latency
- packets/frames/bytes、Pending、drop、flush/reset
- queue high-watermark、retry、shared refcount
- terminal state 和稳定 error code

不在逐帧正常路径写 info/warn；高频错误聚合后按阈值输出。

## 5. Metrics

至少提供：

- `media_processing_jobs{kind,state,profile}`
- `media_processing_frames_total{direction,media,codec}`
- `media_processing_drops_total{reason,media}`
- `media_processing_pending_total{stage}`
- `media_processing_queue_depth{stage}`
- `media_processing_latency_ms{stage}`
- `media_processing_preflight{profile,operation}`
- `media_processing_shared_refs`
- `media_processing_restarts_total{reason}`
- `media_processing_resource_reserved{kind}`

label 不得包含 job id、完整 StreamKey 或其他高基数字段。

## 6. Resource leak 与运维

扩展 `ResourceLeakReport`：

- non-terminal processing jobs
- live blocking workers
- derived publishers/subscribers
- shared-task references
- reserved processing resources

配置热更新：

- 纯上限增加可 live apply。
- profile/features、默认 backend、上限降低到当前使用量以下返回 `ModuleRestartRequired`。
- module 不实现私有 restart 流程。

运维文档必须给出 preflight 查询、创建/停止 Job、定位 Unsupported、观察队列/丢帧、检查动态库/SBOM 和安全下线步骤。

## 7. 故障注入

- backend selection failure
- 连续 corrupt packets
- decoder/encoder Pending storm
- worker panic
- source reconnect / target lease conflict
- output queue blocked
- deadline/cancel at every lifecycle step
- module restart and engine shutdown

所有场景必须产生稳定状态、诊断和空 leak report，不能 abort 进程或留下派生流。
