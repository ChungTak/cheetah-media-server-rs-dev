# 02 · Capability Registry 与领域契约收敛

## 1. 目标

消除 `EngineMediaFacade` 静态 capability、`MediaServices` 动态 provider 与默认 stub 三套状态，使第三方调用者能够可靠区分：

- provider 未编译或未注册：Unavailable。
- provider 正在初始化/重启：Unavailable + retryable。
- provider 已注册但特定操作不支持：Unsupported。
- provider 声明支持：调用必须存在成功路径和生产测试。

## 2. 固定设计

### 2.1 唯一事实来源

`MediaServices` 内部 registry 是唯一 provider 和 capability 来源。新增 registry-backed `MediaFacade`，所有方法在调用时解析当前 provider，不缓存 module provider 的旧 `Arc`。

禁止：

- Engine 启动时为 record/snapshot/proxy/RTP 注册 stub。
- `with_record/with_snapshot/with_proxy/with_rtp` 维护另一份 capability。
- provider 未注册但 capability 仍返回 true。

Engine 启动只注册已经实现的基础 control；publish/subscribe 在 S2 完成前不宣告可用。record、RTP、snapshot、proxy 由对应 module 在 init/start 后注册。

### 2.2 注册租约

provider 注册返回 `ProviderRegistration`：

```rust
pub struct ProviderRegistration {
    pub capability: MediaCapability,
    pub provider_id: String,
    pub generation: u64,
}
```

module stop/restart 时按 registration 注销；旧 generation 不能注销新 provider。调用中拿到旧 provider 后若 module 已停止，返回 Unavailable，不能 panic。

### 2.3 Capability 描述

扩展 capability 描述为：

```rust
pub struct MediaCapabilityDescriptor {
    pub capability: MediaCapability,
    pub version: u32,
    pub provider_id: String,
    pub state: CapabilityState,
    pub operations: Vec<String>,
}
```

`CapabilityState` 固定为 `Starting/Available/Degraded/Stopping/Unavailable`。HTTP capability route 只输出安全字段，不暴露内部对象地址。

## 3. 公共 API 修正

- 为 `RtpApi::get_rtp_session` 提供所有实现，包括 test double。
- 所有 `RtpSenderRequest` 构造补齐 `codec_hint`；新字段有 serde default，避免旧 HTTP body 立即破坏。
- `MediaCapabilitySet::add` 对同 capability 去重并更新版本。
- `MediaRequestContext` 由 adapter 填充 request ID、principal、source、deadline 和 correlation ID。
- 为所有 query 的 page/page_size 在 domain 构造或 provider 入口统一 clamp。
- Unknown/未来枚举使用向后兼容策略；不能依赖穷尽反序列化导致旧客户端拒绝新值。

## 4. Engine 与 module 生命周期

1. Engine 创建 registry，不注册虚假能力。
2. module init 创建 provider，但状态为 Starting。
3. module start 完成资源绑定后切换 Available。
4. module stop 先切换 Stopping，拒绝新命令，完成有界 drain 后注销。
5. `ModuleRestartRequired` 由 module manager 重建；新 generation Available 后旧 registration 失效。
6. adapter 每次请求从 registry 获取 provider，不能持有跨 restart 的永久 provider。

## 5. 任务清单

| ID | 任务 | DoD |
| --- | --- | --- |
| S1-T1 | 修复 SDK contract 编译 | `cargo test -p cheetah-sdk` 可运行 |
| S1-T2 | registry-backed facade | 无静态 stub provider |
| S1-T3 | registration/generation | restart 不留下 stale provider |
| S1-T4 | capability descriptor | 状态、provider、operation 诚实 |
| S1-T5 | capability HTTP/SDK 查询 | native 可查询，Rust 可调用 |
| S1-T6 | 并发注册测试 | 同 capability 替换规则确定且可测 |

## 6. 验收

```bash
cargo test -p cheetah-media-api
cargo test -p cheetah-sdk signal_contracts
cargo test -p cheetah-engine media_provider
cargo clippy -p cheetah-media-api
cargo clippy -p cheetah-sdk
cargo clippy -p cheetah-engine
```

- [ ] 无 provider 时 `MediaServices::<capability>()` 返回 None/Unavailable。
- [ ] provider 注册后 capability 立即一致。
- [ ] stop/restart 后旧 provider 不再被 adapter 调用。
- [ ] capability 宣告支持的每个 operation 都有生产成功测试。

