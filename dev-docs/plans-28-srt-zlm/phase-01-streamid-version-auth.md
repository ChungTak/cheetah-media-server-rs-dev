# Phase 01 — Stream ID / 版本类型 / 鉴权参数（可执行）

- **状态**: 完成  
- **依赖**: 无  
- **兼容规范**: [reference-behavior-zlm-compat.md](reference-behavior-zlm-compat.md) §2、§5.3、§8  
- **架构**: [srt-zlm-architecture.md](srt-zlm-architecture.md) §3–§5、§8  

## 完成标准（DoD）

- [ ] `parse` 行为满足参考规范 §2.3 / §2.4 全表  
- [ ] `ingress.default_mode` 默认 `"request"`  
- [ ] `auth_params` 含 `m` 及全部非 h/r 键  
- [ ] 版本 parse/compare 纯函数 + 单测  
- [ ] module classify 使用新模型；旧配置可回退  
- [ ] `cargo test -p cheetah-srt-core` / `cheetah-srt-module` 通过  
- [ ] 无 `vendor-ref` 引用  

---

## 任务 1.1 — 扩展 core 解析 API

### 文件

- 修改：`crates/protocols/srt/core/src/stream_id.rs`  
- 修改：`crates/protocols/srt/core/src/lib.rs`（re-export）  
- 修改：`crates/protocols/srt/core/src/error.rs`（如需新错误文案）  
- 修改：`crates/protocols/srt/core/tests/parser.rs`  

### 实现步骤

1. 定义 `StreamIdParseOptions`（见 architecture §3.1）。  
2. 增加：

```rust
pub fn parse_srt_stream_id_with_options(
    input: &str,
    opts: &StreamIdParseOptions,
) -> SrtCoreResult<ParsedSrtStreamId>
```

3. 保留 `parse_srt_stream_id(input)` 作为便捷包装：使用 **严格 ZLM 默认 options**（`strict_prefix=true, strict_resource=true, allow_bare_key=false, default_vhost="__defaultVhost__"`），**或** 明确文档写明旧签名兼容策略。  
   - 推荐：旧函数改为调用严格 options，避免双语义；用 `allow_bare_key` 仅在 module 配置打开时走 with_options。  

4. `ParsedSrtStreamId` 字段改为 architecture §3.1；若需避免破坏过多，可保留 `stream_key` 作为 `format!("{app}/{stream}")` 派生字段。  

5. **解析算法严格按参考规范 §2.3**：  
   - `m` **先**写入 `auth_params`，再根据 `auth_params.get("m")` 设置 `mode`  
   - 不要 `fields.remove("m")` 后丢失鉴权侧的 m  

6. `strict_resource`：`r` 按 `/` 分割，`parts.len() < 2` 或空段 → `Err`。  
7. `strict_prefix`：无 `#!::` → `Err`；若 `allow_bare_key` 则走旧 bare 逻辑（mode=None，app/stream 用 `stream_key_from_string` 规则填充或仅填 resource）。  

### 测试（`parser.rs`）— 必须全部添加

| 测试名建议 | 输入 | 断言 |
|------------|------|------|
| `zlm_publish_with_vhost` | `#!::h=zlmediakit.com,r=live/test,m=publish` | vhost/app/stream/mode；auth_params["m"]=="publish" |
| `zlm_play_default_no_m` | `#!::r=live/test` | mode=None；app=live stream=test |
| `zlm_request` | `#!::r=live/test,m=request` | mode=Request；auth 含 m |
| `zlm_token_and_custom` | `#!::r=live/test,m=publish,token=t,foo=bar` | auth 含 m,token,foo；无 h/r |
| `missing_r_fails` | `#!::m=publish` | Err |
| `single_segment_r_fails_strict` | `#!::r=live` | Err |
| `bare_rejected_when_strict` | `live/test` | Err |
| `bare_ok_when_allowed` | opts.allow_bare_key | Ok 兼容 |
| `percent_encoded_r` | `#!::r=live%2Ftest,m=play` | app/stream 正确 |
| `plus_is_literal` | 现有 + 用例保留 | |

运行：

```bash
cargo test -p cheetah-srt-core
```

---

## 任务 1.2 — 版本纯函数

### 文件

- 新建：`crates/protocols/srt/core/src/version.rs`  
- 修改：`lib.rs` re-export  

### 实现

```rust
/// "1.3.0" -> 0x00010300
pub fn parse_srt_version(s: &str) -> SrtCoreResult<u32>;
pub fn format_srt_version(v: u32) -> String;
pub fn version_at_least(peer: u32, min: u32) -> bool;
```

常量：

```rust
pub const SRT_VERSION_1_3_0: u32 = 0x0001_0300;
pub const SRT_VERSION_1_5_0: u32 = 0x0001_0500;
```

单测：边界 `1.2.9 < 1.3.0`、`1.3.0 ==`、`1.5.0 >`、非法字符串。

---

## 任务 1.3 — Module 配置扩展

### 文件

- `crates/protocols/srt/module/src/config.rs`  

### 改动

1. `SrtIngressConfig::default().default_mode = "request".into()`  
2. 增加字段（serde default）：

```rust
// SrtModuleConfig
default_vhost: String,              // "__defaultVhost__"
min_peer_srt_version: String,       // "1.3.0"
local_srt_version: String,          // "1.5.0"
require_peer_version_extension: bool, // false
stream_id: SrtStreamIdModuleConfig,
```

3. `SrtStreamIdModuleConfig` 默认：`strict_prefix=true, strict_resource=true, allow_bare_key=false, stream_key_vhost_mode="app_only"`  
4. `from_value` / schema 自动通过 serde；确认 `default_json()` 含新字段。  

---

## 任务 1.4 — classify + auth

### 文件

- 优先 **抽出** 到：  
  - `module/src/stream_classify.rs`  
  - `module/src/auth.rs`  
- 从 `module.rs` 移动 `classify_stream` / `authorize_stream` / `stream_key_from_string`  

### `classify_stream` 目标签名

```rust
pub fn classify_stream(
    config: &SrtModuleConfig,
    stream_id: Option<&str>,
) -> Result<SrtClassifiedStream, String>

pub struct SrtClassifiedStream {
    pub mode: SrtStreamMode,
    pub stream_key: StreamKey,
    pub auth: SrtAuthContext,
}
```

算法：

1. 构造 `StreamIdParseOptions` from config  
2. parse  
3. mode = parsed.mode.unwrap_or(default_mode_enum)  
4. StreamKey from vhost mode（architecture §3.2）  
5. 填 `SrtAuthContext`  
6. `authorize_stream(&config.auth, &auth_ctx)?`  
7. Ok  

### `authorize_stream`

- 使用 `auth.auth_params.get("token")`  
- 保留全局 token + users 逻辑  
- 额外：可提供 `auth_params_as_query(&auth) -> String` 把 params 编成 `k=v&k2=v2`（供 webhook 后续）  

### 更新 `handle_driver_event(Connected)`

使用 `SrtClassifiedStream`；失败 Close reason 用稳定字符串：`invalid_stream_id: ...` / `auth_rejected`。

---

## 任务 1.5 — 更新 property / fuzz

- `testing/property-tests`：字段顺序交换（`m` 与 `r` 对调）结果稳定  
- fuzz `fuzz_stream_id`：确保新解析不 panic（Err 可接受）  

```bash
cargo test -p cheetah-srt-property-tests
```

---

## 任务 1.6 — 破坏性变更说明

在 `config.rs` 的 `default_mode` 字段旁注释：

```rust
/// Default when streamid omits `m`. ZLM-compatible default is `request` (play).
/// Set to `publish` to restore pre-compat behavior.
```

---

## 验收命令

```bash
cargo fmt
cargo clippy -p cheetah-srt-core
cargo clippy -p cheetah-srt-module
cargo test -p cheetah-srt-core
cargo test -p cheetah-srt-module
cargo test -p cheetah-srt-property-tests
```

## 本阶段不做

- driver 握手拒绝旧版本（Phase 02）  
- FEC  
- 完整 webhook HTTP  
- 弱网 netem  
