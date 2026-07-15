# 13 · 工具链、CI 与发布门禁

## 1. 工具链基线

保留仓库精确 Rust `1.94.1`。CI 镜像或内部镜像必须预装/提供该版本，所有正式证据使用普通 `cargo`，不能用 `cargo +stable` 替代。流水线第一步输出 `rustc --version --verbose`、`cargo --version` 和 lockfile hash；取不到精确工具链即 S0 失败，不通过改版本绕过。

## 2. 测试等级

| 等级 | 内容 | 典型失败 |
| --- | --- | --- |
| L0 | Domain/core 单元、属性、golden | 模型、状态机、wire 漂移 |
| L1 | driver/module + production provider | I/O、生命周期、真实副作用 |
| L2 | native HTTP 完整服务器 | adapter、auth、deadline、幂等 |
| L3 | 兼容 HTTP 黑盒 | 字段与场景互操作 |
| L4 | 四类信令、故障、长稳、泄漏 | 系统交付失败 |

fake provider 测试最高只计 L0。真实媒体必须由独立 parser/decoder 验证。

## 3. 每个任务的最低命令

Rust 改动至少运行：

```text
cargo fmt --check
cargo clippy -p <changed-crate> -- -D warnings
cargo test -p <changed-crate>
```

改动 `cheetah-media-api`、`cheetah-sdk`、engine、codec 或公共 protocol-core 时，再运行所有直接反向依赖 crate 的 test/check 和信令 contract。无需例行 `--all-features`；服务器发布 profile 和任务涉及的 feature 组合必须单独检查。

## 4. CI 分组

1. S0：toolchain、fmt、依赖边界检查。
2. S1：media-api/sdk/core 单元、属性和 serde compatibility。
3. S2：RTP、MP4、snapshot、record、proxy、Webhook module tests。
4. S3：native HTTP、兼容 HTTP golden/黑盒。
5. S4：signal contracts、并发取消、资源泄漏。
6. S5：发布 profile build、制品 smoke 和证据归档。

测试不得共享固定端口或仓库目录。超时必须终止子进程并打印受限日志。flaky 用例第一次失败即保留证据，可自动重跑一次用于诊断，但重跑通过不使门禁变绿，需修复后重新运行整组。

## 5. 发布阻断项

以下任一存在即禁止发布：能力/URL 虚报；非图片字节伪装 JPEG；回放只变内存状态；deadline 后遗留任务；FFmpeg 未实际执行却返回运行；准入未接主路径；跨资源授权泄漏；信令合同使用 fake/伪媒体；兼容接口证据等级虚报。

`REL-01` 修复精确工具链供应；`REL-02` 建立 S0–S5；`REL-03` 资源泄漏观测；`REL-04` 发布证据自动归档。

