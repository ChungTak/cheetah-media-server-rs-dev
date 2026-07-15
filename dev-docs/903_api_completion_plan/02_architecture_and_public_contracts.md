# 02 · 架构与公共契约

## 1. 固定调用链

```text
third-party signaling server
  -> native HTTP adapter / in-process Rust SDK
  -> MediaServices + EngineContext
  -> domain provider
  -> protocol module / system module
  -> driver/runtime adapter
  -> codec + engine media plane
```

兼容 HTTP 只作为另一 adapter 接入同一 providers。adapter 不持有 RTP、回放、代理或录制业务状态。

## 2. 公共服务变更

在 `cheetah-media-api` 增加以下 framework-neutral traits，并由 `MediaServices` 提供独立 slot、register/unregister/get：

| Trait | 必需方法 | 责任 |
| --- | --- | --- |
| `PlaybackApi` | `open/get/list/control/stop` | 文件回放会话 |
| `ImageEncodeApi` | `encode` | 帧到 JPEG/PNG 等图片 |
| `MediaAdmissionApi` | `authorize` | 发布、播放、代理、RTP 前置准入 |
| `WebhookAdminApi` | `create/get/list/update/delete/test` | 外部投递配置管理 |
| `MediaOutputRegistryApi` | `register/unregister/snapshot` | 活跃输出 schema 与端点 |

`ProviderRegistration` 延续 generation 防止旧 module 注销新 provider。新 slot 缺失时 facade 返回 `Unavailable`；provider 存在但 operation 不支持时返回 `Unsupported`。

`MediaFacade` 不再作为必须实现全部能力的扩张点；新增能力通过独立 slot 获取。旧 facade 方法保留一个发布周期，仅做委托，不维护状态。

## 3. 通用类型

- 所有 ID 使用 newtype；外部字符串只在 adapter 构造时校验。
- 所有 list 使用 `Page<T>`，固定 `limit` 范围 1–200，默认 50；稳定排序后分页。
- 异步资源统一包含 `state`、`created_at`、`updated_at`、`generation`、`last_error`。
- 状态更新使用 expected generation；不匹配返回 `Conflict`，避免覆盖并发变更。
- `deadline` 为 Unix epoch 毫秒；进入 provider 先判断过期，长任务接收 runtime-neutral cancellation。
- 创建操作读取 `idempotency_key`；状态查询、删除和纯控制不要求该键。

错误固定为 `InvalidArgument`、`Unauthenticated`、`PermissionDenied`、`NotFound`、`Conflict`、`Unsupported`、`Unavailable`、`DeadlineExceeded`、`ResourceExhausted`、`Internal`。adapter 只映射状态码和 wire error，不改变分类。

## 4. 依赖与迁移顺序

1. 先增加类型、traits 和默认 Unsupported 实现，保持 workspace 可编译。
2. 扩展 `MediaServices`、EngineContext/facade，并补 registry 生命周期测试。
3. 逐个迁移 production provider；同一能力只保留一个状态源。
4. 最后接 native/兼容 adapters，删除其私有状态和静态能力判断。
5. 兼容 shim 标注废弃版本和删除版本，至少保留一个小版本周期。

## 5. 架构验收

- [ ] SDK 公共签名无 `tokio::*`、`tokio_util::*`、Axum 或 FFmpeg 类型。
- [ ] protocol-core 不新增 async、socket、系统时钟和 EngineContext 依赖。
- [ ] module 只通过 RuntimeApi/SDK 使用任务、取消和完成通知。
- [ ] provider 重启后旧 registration 无法注销新实例。
- [ ] 缺 provider、缺 operation、provider 故障返回三种可区分结果。

