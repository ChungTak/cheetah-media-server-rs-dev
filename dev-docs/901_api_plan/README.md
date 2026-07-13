# Cheetah 流媒体系统接口层开发计划

## 1. 文档定位

本文档定义 Cheetah 流媒体服务器的系统接口层，不定义 GB28181、ONVIF、Apple HomeKit、Matter 等信令协议的实现。上述协议由独立的第三方项目实现；第三方项目通过本文档定义的媒体端口完成发布、播放、录制、快照、RTP 会话和媒体查询等操作。

本计划面向外部编程执行体。执行体不能依赖仓库中的参考实现目录；本目录已经把实现所需的接口语义、字段、错误规则、兼容路径和验收条件复制为独立说明。实现时必须同时遵守仓库根目录 `AGENTS.md` 与 `SystemArchitecture.md`。

## 2. 核心结论

1. 新增独立的 `cheetah-media-api` crate，承载媒体领域模型、端口 trait、内部事件和协议无关错误。
2. `cheetah-media-api` 不依赖 `cheetah-sdk`，避免领域层反向依赖系统层；`cheetah-sdk`、`cheetah-engine` 和各 module 通过适配器注入该端口。
3. 领域端口采用异步、runtime-neutral 的接口，公共接口不得暴露 Tokio、Axum、数据库连接或协议 wire DTO。
4. 媒体身份使用新的 `MediaKey { vhost, app, stream, schema }`。与现有 `StreamKey` 的转换必须显式、可逆，并集中在 bridge 中处理。
5. 提供两类 HTTP 表面：
   - native API：面向 Cheetah 自有控制面，使用稳定的 `/api/v1/media/...` 资源模型和统一 HTTP 错误。
   - ZLMediaKit compatibility API：独立 adapter，默认保留 `/index/api/*` 与 `/index/hook/*` 的路径、参数、返回字段及状态语义，支持旧管理台、SDK 和 exporter。
6. 兼容接口与 native API 必须调用同一组领域端口；禁止在兼容路由中复制业务逻辑。
7. 录制模块现有的 `/zlm/*` 路由应迁移到兼容 adapter。迁移期间可保留别名，但别名只能转发到 adapter 的同一实现。
8. 第一阶段覆盖完整接口目录和可用的高价值子集；未实现能力必须返回明确的 `TODO/Unsupported`，不得伪装成功。

## 3. 文档索引

| 文档 | 内容 |
| --- | --- |
| [01_baseline_and_constraints.md](01_baseline_and_constraints.md) | 现有工程基线、分层和不可违反的约束 |
| [02_media_domain_contract.md](02_media_domain_contract.md) | `cheetah-media-api` 领域模型、trait、错误和事件 |
| [03_engine_provider_integration.md](03_engine_provider_integration.md) | EngineContext、Provider、模块注入和生命周期 |
| [04_native_http_adapter.md](04_native_http_adapter.md) | native HTTP 资源模型和请求/响应规范 |
| [05_zlm_http_adapter.md](05_zlm_http_adapter.md) | ZLMediaKit 兼容 HTTP 全目录、字段和迁移策略 |
| [06_event_bus_and_webhook.md](06_event_bus_and_webhook.md) | 内部事件总线、可靠性和兼容 webhook |
| [07_media_operations.md](07_media_operations.md) | 录制、快照、代理、RTP、发送和播放控制 |
| [08_signal_integration_contracts.md](08_signal_integration_contracts.md) | GB28181/ONVIF/HomeKit/Matter 的媒体调用契约 |
| [09_legacy_reference_catalog.md](09_legacy_reference_catalog.md) | 已整理的参考接口、字段和行为目录 |
| [10_roadmap_migration.md](10_roadmap_migration.md) | 分阶段实施、兼容迁移和 TODO 管理 |
| [11_test_and_acceptance.md](11_test_and_acceptance.md) | 单测、集成测试、互操作和验收标准 |

## 4. 执行顺序

先完成 01、02、03 建立端口和依赖边界，再完成 04 的 native API。之后并行实现 05、06、07；08 只实现媒体调用契约，不实现信令协议；最后按 10 的迁移顺序收敛旧路由并执行 11 的验收。

