# 01. 工程基线与边界

## 1. 目标

把“媒体能力”从具体 HTTP 协议中分离出来，使同一个媒体核心可以同时服务于 native 控制面、ZLMediaKit 兼容面、未来二进制 RPC，以及外部信令项目。

## 2. 当前工程基线

- `cheetah-sdk` 已经提供 `StreamKey`、`StreamManagerApi`、`EngineContext`、`SystemEvent` 和框架无关的 HTTP module 契约。
- `cheetah-engine` 提供内存型 stream manager、event bus、module manager 及基础服务注册；新的媒体 API 应注入到该上下文，而不是让 HTTP adapter 直接创建另一套状态。
- `cheetah-codec` 统一 `AVFrame`、`TrackInfo`、时间基、参数集、Access Unit 和时间戳处理。媒体 API 只引用媒体元数据和句柄，不重新定义私有帧模型。
- RTP module 已有 RTP server/client 与 `/api/v1/rtp` 方向的接入基础；新的领域端口需要将其能力包装为统一 `RtpSession`，不重写 RTP 状态机。
- record module 已有录制任务、文件查询和旧式 ZLM record route；旧 route 是迁移对象，不是新的领域边界。
- control crate 当前使用 Axum，但 Axum 只能存在于应用/adapter 实现中，不能泄漏到 `cheetah-media-api` 或 SDK 公共契约。

## 3. 依赖方向

```text
HTTP/RPC adapter
       ↓
cheetah-sdk / EngineContext bridge
       ↓
cheetah-media-api (domain ports + models + events)
       ↓
media provider implementations
       ↓
cheetah-engine / cheetah-codec / protocol module
```

实际 crate 依赖必须保持单向：

- `cheetah-media-api` 不依赖 Axum、Tokio、数据库、具体协议 module 或 `cheetah-sdk`。
- `cheetah-sdk` 可以依赖 `cheetah-media-api`，并在 `EngineContext` 中暴露 `MediaControlApi` 的注入能力。
- adapter 可以依赖 `cheetah-sdk` 和 `cheetah-media-api`。
- record、RTP 等 provider 可以依赖 SDK、codec 和自身 module，但不得让 domain 依赖 provider。
- 二进制 RPC adapter 与 ZLM HTTP adapter 之间不得互相调用。

## 4. 命名和运行时规则

- crate 名称为 `cheetah-media-api`；若拆分实现，使用 `cheetah-media-provider-*`、`cheetah-media-http-*` 等清晰名称。
- 类型、目录、注释使用 `module`，不引入其他模块命名。
- 所有公共异步 trait 使用 runtime-neutral future；可以使用 `async-trait` 或等价的 boxed future，但不得返回 `tokio::JoinHandle`、`tokio::sync` channel 或 `axum::Response`。
- 时间由调用方传入或由注入的 clock 负责；domain 不直接调用系统时间。
- 每个分页、队列、缓存、RTP 重传窗口和订阅队列都必须有上界。

## 5. 明确不属于本项目

- SIP、GB28181 目录树、设备注册、ONVIF SOAP、HomeKit HAP、Matter Interaction Model。
- 摄像机发现、云台协议编解码、设备鉴权协议。
- 具体 HTTP 路由实现中的 Axum extractor、cookie、query parser。
- 直接把 ZLMediaKit 的 JSON 结构作为内部模型。

这些项目需要的媒体动作必须在 08 中有端口契约；具体信令代码由外部项目完成。
