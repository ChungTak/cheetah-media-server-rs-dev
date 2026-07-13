# 03. Engine 与 Provider 接入

## 1. EngineContext 扩展

在 `EngineContext` 中增加 runtime-neutral 的媒体服务入口，建议使用 `Arc<dyn MediaControlApi>` 或按能力注入的 facade：

```rust
pub struct MediaServices {
    pub control: Arc<dyn MediaControlApi>,
    pub publish_subscribe: Arc<dyn PublishSubscribeApi>,
    pub record: Arc<dyn RecordApi>,
    pub snapshot: Arc<dyn SnapshotApi>,
    pub proxy: Arc<dyn ProxyApi>,
    pub rtp: Arc<dyn RtpApi>,
}
```

若某能力未注册，必须在构造上下文时显式标记 unavailable；不能用空实现吞掉调用。

## 2. Provider 分工

### 2.1 Stream provider

桥接现有 `StreamManagerApi`、publisher/subscriber、codec track 信息和协议注册状态，负责：

- MediaKey 与 StreamKey 转换。
- 发布者独占 lease、订阅者计数和关闭原因。
- 同一媒体的多协议输出视图关联。
- 关键帧请求和 idle publisher 清理。

### 2.2 Record provider

包装 record module 的任务 registry、executor、文件索引和存储策略。它不再导出 ZLM 专用 DTO；ZLM adapter 在边界处转换 `RecordTask`、`RecordFile`。

### 2.3 RTP provider

包装 RTP module 的 server/client session。RTP 收发、分包、定时器和 socket 仍属于 driver；module/provider 只做资源分配、媒体绑定、鉴权和生命周期编排。

### 2.4 Proxy/output provider

为 RTSP/RTMP/SRT/HTTP-FLV/WebRTC 等 module 提供拉流代理、推流代理、FFmpeg/转码任务和输出 URL 的统一句柄。转码策略引用 codec 和 engine 能力，不把 FFmpeg 类型放入公共 API。

## 3. 注册与生命周期

启动顺序：`create -> init -> start`。媒体 provider 必须在自身 `start` 完成后注册 capability；关闭顺序为停止新请求、发布终止事件、等待有界 drain、释放会话和存储句柄。

模块重启遵守 `ModuleRestartRequired` 语义，由基础层执行重建；provider 不得自建绕过 module manager 的私有重启流程。

建议增加：

- `MediaProviderRegistry`：按 capability 注册 provider。
- `MediaCapabilitySet`：声明 record、snapshot、rtp、proxy、playback 等能力及版本。
- `MediaRequestContext`：request id、principal、deadline、source adapter、trace context。
- `IdempotencyStore`：对 start proxy、start record、open RTP 等重复请求提供确定结果。

## 4. 并发与背压

- 查询使用分页和快照时间点，不能无界返回全量 session/file。
- 事件订阅使用有界队列；慢订阅者收到 `Lagged` 或被断开，不能拖累媒体热路径。
- RTP、录制和代理任务的重试、缓存、重排窗口均有配置上限。
- 每个命令设置 deadline；超时后返回 `Timeout`，后台任务状态通过事件或任务查询获得。
- 不在每包路径持有 contended mutex；使用分片、所有权局部化和 `Arc<AVFrame>`/`Bytes`。

