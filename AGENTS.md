# AGENTS.md

本文件用于指导本仓库中的编码工作。它只定义高优先级的工程约束、命名规则、边界和提交前检查；不替代详细设计文档。

## 1. 项目范围

- 本项目是 `cheetah` 流媒体服务器。
- crate 命名统一使用 `cheetah-` 前缀。
- 文档、注释、类型名、目录名统一使用 **module（模块）**，不要再引入 `plugin` 命名。
- 详细架构以 `SystemArchitecture.md` 为准；若实现与该文档冲突，优先修正实现，必要时同步更新文档。

## 1.1 crate 命名规则

- 协议主路径 crate 统一命名为 `cheetah-<proto>-core`、`cheetah-<proto>-driver-<runtime>`、`cheetah-<proto>-module`。
- runtime 抽象和实现统一命名为 `cheetah-runtime-api`、`cheetah-runtime-<runtime>`。
- SDK、宏、配置、控制面、引擎、媒体基础层等共享 crate 使用清晰职责后缀，如 `cheetah-sdk`、`cheetah-sdk-macros`、`cheetah-config`、`cheetah-control`、`cheetah-engine`、`cheetah-codec`。
- 非主路径测试/工具 crate 也必须使用可读完整名称；属性测试使用 `cheetah-<proto>-property-tests`，不要使用 `pbt` 这类不透明缩写。
- 对外绑定或目标平台桥接 crate 使用明确目标后缀，如 `cheetah-<proto>-c-api`、`cheetah-<proto>-wasm`。
- 新增 crate 前先确认它属于协议三段式、共享基础设施、测试工具还是平台绑定；不要用缩写掩盖职责边界。

## 1.2 crate 目录组织

- 顶层 `crates/` 按职责分组，不再把所有 crate 扁平展开。
- 共享基础设施放在 `crates/foundation/`、`crates/runtime/`、`crates/sdk/`、`crates/system/`。
- 协议相关 crate 放在 `crates/protocols/<proto>/` 下。
- 协议主路径目录固定为 `core/`、`driver-<runtime>/`、`module/`，但 Cargo package name 仍必须是 `cheetah-<proto>-core`、`cheetah-<proto>-driver-<runtime>`、`cheetah-<proto>-module`。
- 协议绑定放在 `crates/protocols/<proto>/bindings/<target>/`，如 `bindings/c-api`、`bindings/wasm`。
- 协议测试工具放在 `crates/protocols/<proto>/testing/<kind>/`，如 `testing/property-tests`。
- fuzz harness 放在 `crates/protocols/<proto>/fuzz/`，作为独立 cargo-fuzz workspace 管理，默认不加入根 workspace members。

## 2. 分层与依赖方向

- 严格遵守六层架构，依赖方向只能单向向下。
- `cheetah-codec` 是 Foundation 层；不得依赖引擎、模块、HTTP、数据库或具体 runtime。
- `cheetah-sdk` 定义模块契约和引擎注入能力；不得反向依赖具体协议模块。
- `cheetah-sdk` 的 HTTP module 契约必须保持框架无关；不要在 SDK 公共接口中绑定 Axum/Tide/Actix 等具体 Web 框架。
- Feature modules 只能通过 `cheetah-sdk` 和 `cheetah-codec` 与系统交互。
- 不允许跨层偷依赖；如果需要跨层能力，先补 trait / API，再注入。

## 3. 所有协议统一采用 `core + driver + module`

每个协议都必须拆成：

- `cheetah-<proto>-core`
- `cheetah-<proto>-driver-tokio`
- `cheetah-<proto>-module`

职责边界固定：

- `core`：纯 Sans-I/O 协议状态机。
- `driver`：runtime / socket / timer / task 驱动层。
- `module`：引擎接入、资源分配、业务编排、权限控制。

不要把这三层混写在一个 crate 或一个大模块里。

## 4. Sans-I/O 硬约束（适用于所有 protocol-core）

`protocol-core` 必须满足以下约束：

- 不依赖 Tokio 或任何具体 runtime。
- 不持有 socket / listener / stream。
- 不启动线程，不创建异步任务，不写 `async fn` 作为核心状态机接口。
- 不直接调用 `Instant::now()` 或系统时间 API。
- 不直接访问数据库、HTTP、`EngineContext`、`StreamManager`、`RoomService`。
- 不写业务编排逻辑。
- 输入输出必须是显式的 `Input / Output / Event / Timer` 模型。

判断标准：`core` 只回答“输入来了以后，协议状态如何推进、应该输出什么动作”。

## 5. Driver 约束

- 多线程高性能并发是主目标；第一阶段 runtime 只实现 Tokio。
- runtime 抽象放在 driver 层，不放在 core 层。
- runtime 抽象统一通过 `RuntimeApi` 注入，公共接口使用 runtime-neutral 类型。
- `RuntimeApi` 采用双通道任务模型：`spawn`（`Send` 主路径）+ `spawn_local`（browser/WASI 附加路径）；不得为了 wasm 目标削弱多线程主路径。
- 在 `cheetah-runtime-api` / `cheetah-sdk` / `cheetah-engine` / `*-module` 的公共接口中，禁止直接暴露 `tokio::*` 或 `tokio_util::*` 类型。
- `tokio`/`tokio-util` 仅允许留在 `cheetah-runtime-tokio`、`*-driver-tokio`、应用层 crate 以及 `cheetah-engine` 的内部实现；`cheetah-engine` 公共接口仍必须保持 runtime-neutral。若其他 runtime 缺少对应原语，只能在该 runtime adapter crate 内封装后再暴露抽象。
- 收包、发包、分帧、组帧、timer、spawn、channel、backpressure 都在 driver。
- TCP framing / UDP 收发属于 driver，不属于 core。
- driver 不应持有业务状态；业务决策应回到 module。

## 6. Module 约束

- module 负责接入引擎，不负责重写协议状态机。
- module 通过 `EngineContext` 与系统交互。
- module 不得直接依赖 `tokio::net`、`tokio::time`、`tokio::sync`、`tokio_util::sync`；需要的取消、任务句柄、完成通知统一走 `RuntimeApi` / SDK 抽象。
- module 不得使用 `tokio::select!`；多路等待统一使用 runtime-neutral 原语（如 `CancellationToken` + futures 组合子）。
- 资源分配、会话绑定、鉴权、API 路由、业务映射写在 module。
- publish、play、proxy 创建（pull / push / ffmpeg）、RTP open 等会分配租约/端口/会话的资源操作，必须在分配前调用 `MediaAdmissionApi::authorize`；`Deny` 时不得留下任何租约、端口、任务或幂等成功记录。
- 当配置应用结果为 `ModuleRestartRequired` 时，由基础层执行模块重建重启（`create -> init -> start`）；module 不应自行绕过该语义维护私有重启流程。
- `ModuleManagerApi::restart_module / restart_modules` 只接受 `Running` 模块；非 `Running` 状态必须返回 `Conflict`，避免绕过生命周期约束。
- 同一 `StreamKey` 默认采用单发布者独占语义；不要在 module 侧绕过发布租约模型实现多发布者并写。
- 不要在 module 中复制一套媒体时间戳修正、NALU 处理或参数集缓存逻辑；这些应回到 `cheetah-codec`。

## 7. `cheetah-codec` 规则

`cheetah-codec` 是系统共享的媒体内核，不是“协议工具箱”。

必须遵守：

- 所有协议进入引擎前，统一收敛为 `AVFrame + TrackInfo`。
- 所有协议输出前，优先通过 `cheetah-codec` 导出目标封装视图。
- 时间戳归一化、timebase 转换、DTS 生成、回绕处理、断流标记、Access Unit 拼装、参数集缓存/补发，统一放在 `cheetah-codec`。
- 不要让每个协议各自维护一套私有 frame 模型、时间戳修正器或参数集缓存。
- `cheetah-codec` 不实现 RTMP/RTSP/WebRTC/SIP 等协议状态机。
- 不要把 FFmpeg 类型泄漏进 `cheetah-codec` 的公共接口。

## 8. 兼容优先原则

本项目不是“只实现协议文档”的教学项目。实现时必须优先考虑真实互操作性。

- 行为目标应尽量对齐成熟 C++ 流媒体服务器simple-media-server[vendor-ref/simple-media-server]的工程实践，而不是只覆盖 RFC 的理想路径。
- 入口允许兼容脏数据、历史包袱和厂商 quirks；内部必须规范化；出口必须稳定可预测。
- 厂商兼容逻辑应集中管理、显式命名，不要把特殊分支打散到各处。
- 新增兼容处理时，优先补到 `cheetah-codec` 或明确的 compat 层；不要在协议热路径里到处临时修补。

## 9. 性能与并发约束

- 热路径优先单线程分片和所有权局部化。
- 热路径禁止阻塞；避免 contended mutex。
- 冷路径允许上锁，但不能把锁带入每包必经路径。
- 使用 `Arc<AVFrame>`、`Bytes`、原地处理和有界缓冲；避免不必要的 clone / memcpy / 动态分配。
- Dispatcher / RingBuffer / subscriber queue 必须保持“慢订阅者不拖累其他订阅者”的原则。
- 所有缓存、队列、重传窗口、jitter buffer 都必须有上界。

## 10. Rust 编码约定

- 优先写小模块；新增逻辑时优先新建模块，不要持续膨胀中心文件。
- 目标：单个 Rust 模块尽量控制在约 500 行以内；若明显超过 800 行，应优先拆分。
- 不要创建只被调用一次的小 helper 函数，除非它显著提升可读性或复用边界。
- `format!` 能直接内联变量时，直接写到 `{}` 中。
- 避免布尔位置参数和含义不清的 `Option` 参数；优先使用 enum / newtype / builder / 具名方法。
- 能写穷尽 `match` 时，不要随手加 `_ => {}` 吞掉未来分支。
- 新类型名、新模块名统一使用 `module`，不要重新引入 `plugin`。

## 11. 测试要求

- `core`：优先做纯单元测试、属性测试、fuzz；测试不应依赖真实网络 I/O。
- `driver`：做运行时与 I/O 集成测试。
- `module`：做互操作测试、端到端流程测试。
- 涉及时间戳、重排、参数集补发、协议兼容修复时，必须补测试。
- 修复真实设备或真实客户端兼容问题时，优先补可复现样例或回归测试。

## 12. 提交前最低检查

完成 Rust 代码改动后，至少执行：

1. `cargo fmt`
2. `cargo clippy -p <changed-crate>`
3. `cargo test -p <changed-crate>`

如果改动影响共享基础层、协议公共层或 `cheetah-codec` / `cheetah-sdk`：

4. 继续运行相关工作区测试或受影响模块测试。

不要例行使用 `--all-features`；只有在确实需要验证完整特性组合时才使用。

## 13. 文档同步规则

出现以下情况时，必须同步更新文档：

- 改变分层边界、crate 命名或依赖方向。
- 改变 `AVFrame` / `TrackInfo` / 时间戳模型。
- 改变协议三段式约束或 runtime 抽象。
- 改变模块对外 API、配置模型或 feature flag。

优先更新：

- `SystemArchitecture.md`
- 本文件 `AGENTS.md`
- 相关 README / 示例 / 配置说明

## 14. 一句话总纲

- 协议统一：`core + driver + module`
- 媒体统一：`AVFrame + TrackInfo`
- 时间统一：显式注入、统一归一化
- 实现统一：兼容优先、性能优先、边界清晰
