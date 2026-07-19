# 04 · 架构、crate 与数据流

## 1. ARCH-01：新增 crate

### `cheetah-media-control-plane`

位置：`crates/system/cheetah-media-control-plane/`

职责：

- mutation validation/fencing/capacity orchestration；
- durable idempotency 与 controlled resource index；
- replayable event journal、cursor、reconciliation；
- runtime-neutral facade 和 store traits；
- SQLite provider 的控制面冷路径实现。

公共接口不得暴露 rusqlite connection、Tokio 或 tonic 类型。SQLite I/O 通过
`RuntimeApi::spawn_blocking` 隔离，所有事务和查询有 deadline/bounds。

### `cheetah-media-grpc-adapter`

位置：`crates/system/cheetah-media-grpc-adapter/`

职责：

- generated DTO ↔ domain mapper；
- tonic server 与 gRPC health；
- registry/heartbeat/deregister client；
- SecretExchange client；
- mTLS identity、证书 reload 与 RPC metrics/audit。

该 crate 可以内部依赖 Tokio/tonic，但公开构造参数使用配置、domain ports 和
runtime-neutral handle；不得成为协议 core 或 feature module 的依赖。

## 2. ARCH-02：依赖方向

```text
apps/cheetah-server
  -> cheetah-media-grpc-adapter
       -> cheetah-media-control-plane
            -> cheetah-media-api / cheetah-sdk / cheetah-runtime-api
       -> cheetah-signal-contracts (fixed revision)
       -> tonic/tokio/rustls (internal)

protocol/system modules
  -> cheetah-sdk / cheetah-media-api
  -> no tonic/prost/SQLite/signaling contract
```

`cheetah-engine` 只注册/提供 ports，不依赖 generated contract。feature modules 通过
SDK service slots 获取 fencing、credential、capacity 或 outbound policy 能力，不跨层依赖
两个新 system crate。

## 3. ARCH-03：服务器装配

新增 feature：

```text
signaling-control-plane =
  cheetah-media-control-plane
  + cheetah-media-grpc-adapter
  + cheetah-signal-contracts
  + tonic/prost/rusqlite
```

- 默认 feature 不包含它。
- `media-control-full` 包含它，但媒体处理 feature 仍独立。
- feature-off 时既有 native/ZLM/control server 行为不变。
- feature-on 但配置 disabled 时不 bind、不注册、不创建 SQLite。

启动顺序：

1. 加载/校验 config、TLS、contract metadata 和 store path。
2. 打开 SQLite，执行迁移和完整性检查。
3. 构建 control-plane facade，接入 media ports/capability/event bus。
4. engine/module 全部启动并完成 preflight。
5. bind gRPC listener，health 为 NotServing，mutation gate 关闭。
6. 向 signaling 注册，取得 lease、instance epoch、heartbeat interval、accepted version。
7. 持久化 node runtime state，开启 event journal 与 reconciliation。
8. health 切换 Serving，mutation gate 按 Active/Draining 状态开放。

关闭顺序反向执行：先关闭 create gate，进入 draining，等待有界 RPC，flush store/event，
bounded deregister，停止 gRPC，最后停止 engine。deregister 失败不得无限阻塞 shutdown。

## 4. 请求数据流

```text
mTLS peer
  -> contract/version/auth validation
  -> wire mapper
  -> MediaRequestContext + MediaMutationContext
  -> idempotency prepare/replay
  -> instance/owner/generation/drain/capacity guard
  -> typed media port
  -> controlled resource/result persistence
  -> event append
  -> response persistence
  -> wire response
```

持久化完成后才发送成功响应。若副作用完成但结果持久化状态不确定，返回 `UNKNOWN`，禁止
adapter 猜测 `NOT_APPLIED`。

## 5. 资源所有权

- engine/provider 继续拥有实际 socket、task、publisher lease、file 和媒体状态。
- control plane 拥有跨进程关联、幂等结果、fencing epoch、generation 和 event journal。
- signaling 拥有 Operation/MediaSession/MediaBinding 业务生命周期。
- SQLite 不是媒体状态机，不保存 AVFrame、packet、SDP 原文、secret 或 worker 对象。

进程重启后 control plane 通过所有 typed Get/List 与持久索引对账。找不到的非终态资源标记
Gone；发现同进程 orphan 时先保护观察，再按 typed Stop/Delete 清理。

## 6. 配置边界

配置至少分组：

```text
global.signaling_control_plane.enabled
global.signaling_control_plane.grpc
global.signaling_control_plane.registry
global.signaling_control_plane.contract
global.signaling_control_plane.store
global.signaling_control_plane.events
global.signaling_control_plane.capacity
global.signaling_control_plane.security
global.signaling_control_plane.rollout
```

证书/密钥/credential 只接受受控 handle 或部署 secret provider，不接受日志可见的任意内联
密码。监听地址、advertised endpoint、media addresses 分离，不能从请求 Host 推导。
