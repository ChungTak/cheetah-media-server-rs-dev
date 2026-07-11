# Phase 01 — Stream ID、版本策略与鉴权参数

- **状态**: 待执行
- **范围**: 锁定 ZLM 对齐的 streamid 语义、`h/r/m` 映射、默认拉流、鉴权参数模型、peer 版本下限策略；纯 core + module 分类/鉴权逻辑，不改媒体管线。
- **完成标准**: core 单测覆盖用户文档全部 streamid 样例；无 `m` 默认为拉流；`auth_params` 可序列化用于鉴权；版本配置类型与校验落地；module classify 使用新模型且旧配置可回退。

---

## 实现概览

本阶段固定 **业务语义边界**，避免后续 driver/module 在 streamid 与鉴权上反复修改。

参考：

| 来源 | 路径 |
|------|------|
| ZLM 解析 | `vendor-ref/ZLMediaKit/srt/SrtTransportImp.cpp` `parseStreamid` / `onHandShakeFinished` |
| ZLM 文档 | `vendor-ref/ZLMediaKit/srt/srt.md` |
| 本地解析 | `crates/protocols/srt/core/src/stream_id.rs` |
| 本地分类 | `crates/protocols/srt/module/src/module.rs` `classify_stream` / `authorize_stream` |
| 本地配置 | `crates/protocols/srt/module/src/config.rs` |
| 架构 | [srt-zlm-architecture.md](srt-zlm-architecture.md) |

---

## 1.1 扩展 `ParsedSrtStreamId`

**文件**: `crates/protocols/srt/core/src/stream_id.rs`

建议字段：

```text
vhost: String                 # 来自 h 或调用方注入的 default_vhost
app: String                   # r 第一段
stream: String                # r 第二段（严格模式）
resource_raw: String          # r 原始值
mode: Option<SrtStreamMode>   # publish / request / play；None=未声明
user: Option<String>          # u
session: Option<String>       # s
auth_params: BTreeMap<String, String>  # 除 h/r 外全部 key（含 m）
extras: BTreeMap              # 可与 auth_params 合并或保留兼容别名
```

解析规则：

1. 必须以 `#!::` 开头（**严格 ZLM 模式**）；或当 `allow_bare_key` 时允许 bare / 无前缀 resource。
2. `r` 必填（严格模式）；按第一个 `/` 拆 `app`/`stream`；不足两段时：
   - `strict_resource=true` → `Err`
   - 否则兼容旧逻辑整串进 stream_key。
3. `h` 可选；解析阶段可保留 `Option`，由 module 填 default_vhost。
4. `m=publish` → `Some(Publish)`；`m=request|play` → 对应枚举；未知 `m` → `Err`（与现逻辑一致）；**缺失 `m` → `None`**（默认策略留给 module）。
5. 所有 **非 `h`/`r`** 的 key 进入 `auth_params`（**包括 `m`**），对齐 ZLM。
6. percent-decoding 保持现状；`+` 仍为字面量。

兼容：

- 保留 `stream_key` 派生：`format!("{app}/{stream}")` 或配置策略。
- 旧测试样例继续通过；新增 ZLM 样例。

---

## 1.2 `StreamKey` 与 vhost 映射

**文件**: `module` 分类逻辑（建议新文件 `stream_classify.rs`）

配置：

```text
default_vhost: "__defaultVhost__"
stream_id.strict_resource: true
stream_id.allow_bare_key: false
stream_id.stream_key_vhost_mode: "app_only" | "vhost_prefix"
```

映射：

| 模式 | namespace | path |
|------|-----------|------|
| `app_only`（默认） | `app` | `stream` |
| `vhost_prefix` | `{vhost}/{app}` 或规范化串 | `stream` |

`vhost` 始终写入会话上下文与 metrics labels，即使不进 `StreamKey`。

---

## 1.3 默认模式对齐 ZLM

**文件**: `module/src/config.rs`、`classify_stream`

| 条件 | 结果 |
|------|------|
| `m=publish` | Publish |
| `m=request` / `m=play` | Request / Play（拉流） |
| `m` 缺失 | `ingress.default_mode`，**新默认 `request`** |

迁移：

- 默认值从 `publish` 改为 `request`。
- 配置注释与 index 风险节说明；旧部署可设 `default_mode=publish`。
- 单测覆盖：无 `m` + 默认 request；无 `m` + 配置 publish。

---

## 1.4 鉴权参数模型

**文件**: 建议 `module/src/auth.rs`

```text
SrtAuthContext {
  mode,
  vhost, app, stream,
  stream_key,
  user,
  auth_params,   // 含 m, token, 自定义 key
  peer_addr,
}
```

行为：

1. `auth.enabled=false` → 放行（与现一致）。
2. 全局 token：`publish_token` / `request_token` 与 `auth_params["token"]` 比较。
3. 用户表：`u` + `token`。
4. 预留：`webhook_enabled` 时把 `auth_params` + 资源三元组交给 control/webhook（若基建未就绪，以 trait/ hook + 单元测试固定参数形状，HTTP 实现可 TODO）。

**禁止** 在鉴权路径丢弃除 h/r 外的自定义 key。

---

## 1.5 版本策略类型

**文件**: `core/src/config.rs`（或新 `version.rs`）、module/driver 配置透传

```text
min_peer_srt_version: "1.3.0"     # 解析为 0x010300
local_srt_version: "1.5.0"        # 可选覆盖库默认宣告
require_peer_version_extension: false
```

本阶段至少完成：

- 版本字符串 ↔ `u32`（`major<<16 | minor<<8 | patch`）纯函数。
- 比较与 `is_supported(peer, min)`。
- 配置校验：`min >= 1.3.0` 的推荐告警（或硬校验 min 不得高于 local）。

Driver 侧真正在握手后拒绝可在 **Phase 02** 闭环；本阶段提供类型与单测，并在 module 配置 schema 暴露字段。

---

## 1.6 测试清单

### 单元（core）

| 用例 | 期望 |
|------|------|
| `#!::h=zlmediakit.com,r=live/test,m=publish` | vhost/app/stream/mode 正确；auth 含 `m=publish` |
| `#!::r=live/test` | mode=None；app=live stream=test |
| `#!::r=live/test,m=request` | 拉流 |
| 缺 `r` | Err |
| `r=live` 严格模式 | Err |
| bare `live/test` + allow_bare_key | Ok 兼容 |
| percent-encoding `r` | 正确解码 |
| 自定义 `token=xx,foo=bar` | 均在 auth_params |

### 单元（module classify/auth）

| 用例 | 期望 |
|------|------|
| 无 m + default request | Request + 正确 StreamKey |
| 无 m + default publish | Publish |
| token 错误 | 拒绝 |
| auth_params 含 m 与自定义 key | 结构完整 |

### 回归

- 现有 `core/tests/parser.rs` 全部通过；必要时按新字段调整断言。
- property tests 增加字段排列顺序无关性。
- fuzz `fuzz_stream_id` 仍不 panic。

---

## 1.7 验收命令

```bash
cargo fmt
cargo clippy -p cheetah-srt-core
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-core
cargo test -p cheetah-srt-module
cargo test -p cheetah-srt-property-tests
```

---

## 关键文件

| 动作 | 路径 |
|------|------|
| 改 | `crates/protocols/srt/core/src/stream_id.rs` |
| 改 | `crates/protocols/srt/core/src/config.rs` / `lib.rs` |
| 改 | `crates/protocols/srt/core/tests/parser.rs` |
| 改 | `crates/protocols/srt/module/src/config.rs` |
| 改/拆 | `crates/protocols/srt/module/src/module.rs` → `stream_classify.rs` / `auth.rs` |
| 改 | `crates/protocols/srt/testing/property-tests/**` |
| 参考 | `vendor-ref/ZLMediaKit/srt/SrtTransportImp.cpp` |

---

## 本阶段不做

- 不改 TS demux/mux 与 engine 发布逻辑（Phase 03）。
- 不实现 FEC（Phase 04）。
- 不强制完成握手级版本拒绝的 driver 集成（Phase 02 可接）。
- 不实现完整 webhook HTTP 客户端（可留 hook）。
