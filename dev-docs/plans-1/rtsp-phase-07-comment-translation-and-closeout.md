# Phase 07: 注释中文化与收口

- 状态：已完成（任务 1-4 已完成）
- 范围：完成剩余日文注释翻译、状态收口、文档同步与最终回归矩阵整理。
- 对应用例：所有已迁移用例与所有已移植实现。
- 完成标准：所有阶段状态闭环，所有新增或迁入注释均为中文，测试矩阵可直接用于后续执行跟踪。

## 目标

- 把迁移过程中引入的日文注释、测试描述、fuzz 说明全部翻译为中文。
- 清理临时迁移标记、保留来源说明和必要兼容说明。
- 让 `dev-docs/plans` 目录能够直接作为执行面板使用。

## 最新进展

- 2026-04-20：已完成任务 4（最终回归清单）：依次执行 `cargo fmt`、`cargo clippy -p cheetah-rtsp-core --tests`、`cargo clippy -p cheetah-rtsp-driver-tokio --tests`、`cargo clippy -p cheetah-rtsp-module --tests`、`cargo test -p cheetah-rtsp-core`、`cargo test -p cheetah-rtsp-driver-tokio`、`cargo test -p cheetah-rtsp-module`、`cargo test -p cheetah-rtsp-pbt`、`cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`，结果全部通过；Phase 07 收口完成。
- 2026-04-20：已完成任务 3（文档同步）：核对 `SystemArchitecture.md`、`AGENTS.md` 与 RTSP 相关 README/示例说明，确认本轮迁移未改变 `core + driver + module` 分层边界、依赖方向与 runtime 抽象约束，因此不扩散修改架构文档，仅在 RTSP 计划文档记录同步结论；下一步进入任务 4（最终回归清单）。
- 2026-04-20：已完成任务 2（计划状态收口）：同步核对 `rtsp-phase-01` 至 `rtsp-phase-07` 文件头部状态，确认 Phase 01-06 已闭环并更新总索引 `计划文件清单` 的 Phase 07 状态备注；同时在总索引补充迁移落地 crate、测试文件归档、当前遗留问题与后续建议，便于后续执行追踪；下一步进入任务 3（文档同步）。
- 2026-04-20：已完成任务 1（注释与文案统一）：对 `crates/cheetah-rtsp-core`、`crates/cheetah-rtsp-driver-tokio`、`crates/cheetah-rtsp-module`、`crates/cheetah-rtsp-pbt`、`crates/cheetah-rtsp-fuzz` 执行日文残留扫描，确认迁移后注释与文案已无日文假名残留；下一步进入任务 2（计划状态收口）。

## 具体任务

### 1. 注释与文案统一

- 检查以下位置是否仍有日文：
  - `crates/cheetah-rtsp-core`
  - `crates/cheetah-rtsp-driver-tokio`
  - `crates/cheetah-rtsp-module`
  - `crates/cheetah-rtsp-pbt`
  - `crates/cheetah-rtsp-fuzz`
- 保留必要来源说明，但统一改成中文表达。
- 不翻译协议字面量、标准头字段、RFC 术语值。

### 2. 计划状态收口（已完成）

- 每个阶段完成后，把对应文件头部状态从“未完成”改成“已完成”。
- 更新总索引中的阶段状态与备注。
- 在总索引中补充实际落地的 crate、测试文件、遗留问题与后续建议。

### 3. 文档同步（已完成）

- 若迁移结果改变了以下内容，需要同步更新：
  - `SystemArchitecture.md`
  - `AGENTS.md`
  - RTSP 相关 README 或示例说明
- 若未改变架构边界，只更新 RTSP 计划与必要说明，不扩散修改面。
- 本轮核对结论：未发生架构边界变化，因此仅更新 RTSP 计划文档状态与说明。

### 4. 最终回归清单（已完成）

- `cargo fmt`
- `cargo clippy -p cheetah-rtsp-core --tests`
- `cargo clippy -p cheetah-rtsp-driver-tokio --tests`
- `cargo clippy -p cheetah-rtsp-module --tests`
- `cargo test -p cheetah-rtsp-core`
- `cargo test -p cheetah-rtsp-driver-tokio`
- `cargo test -p cheetah-rtsp-module`
- `cargo test -p cheetah-rtsp-pbt`
- `cargo check --manifest-path crates/cheetah-rtsp-fuzz/Cargo.toml`

## 完成判定

- 所有计划文件状态均为“已完成”。
- 所有迁入代码与测试中的日文注释已清零。
- 总索引可用于追踪来源用例、本地落点与完成情况。
- 文档同步已按影响范围完成。
