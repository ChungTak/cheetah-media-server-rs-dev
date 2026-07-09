# Phase 02: 统一录制模块与多格式文件管理

- **状态**: 已完成
- **目标**: 建立系统级 `cheetah-record-module`，统一调度 FLV/HLS/MP4/PS 录制并维护文件目录
- **完成标准**: 录制控制、文件落盘、索引、过期清理和事件接口成型

## 实现概览

- 复用 `plans-26-mp4-sms` Phase 02 的 `cheetah-record-module`：`config / metadata / registry / task / api / module` 子模块。
- 文件 catalog 由 `RecordRegistry` 内存维护，并通过 `FileQueryRequest` 支持 ABL 式按 app/stream/format/时间窗过滤。
- ZLM 风格 `/index/api/*` 路由复用同一 `RecordApi`，所以 ABL 客户端发出的 SMS / ZLM 数字 type / customized_path 都能落到同一存储。
- 录制 writer 注册表通过 `cheetah_codec::record::RecordContainerWriter` 完成，预留 `Fmp4 / Ts` 扩展位。
- ABL `RecordFileSource` 风格的 retention 策略由 `RecordModuleConfig::cleanup_on_start` 与 `metadata_flush_interval_ms` 提供入口；具体的 m3u8 缓存与目录扫描属于运行时层（host）后续增量。
- 测试覆盖：`cargo test -p cheetah-record-module --lib` 16 用例通过（含 `start/stop/list/file-query/file-delete` API、注册表唯一性与容量、ZLM type/period 解析等）。

## 交付项

1. `RecordFormat`、`RecordPolicy`、`RecordTarget`、`RecordCatalogEntry`
2. `RecordControlApi`，支持 `start`、`stop`、`status`、`list`、`delete`
3. writer registry，把 `RecordFormat` 映射到 `cheetah-codec` writer
4. `RecordCatalog`，记录 app、stream、格式、时间范围、文件大小、持续时长、切片目录
5. 录制事件：开始、切片完成、录制完成、录制失败、目录清理

## ABL 对齐要求

1. 文件 catalog 不能只依赖目录扫描，要有内存索引和过期刷新能力，对齐 `RecordFileSource`
2. HLS 切片与 VOD 回放元数据共享同一时间范围视图，避免拼 m3u8
3. 录制结束后才暴露正式文件，避免外部读取未 finalize 的 MP4
4. 录制文件路径、app、stream、时间戳需要能反查成回放源
5. 预留多文件连续回放所需的文件排序和范围查询接口

## 控制面约束

1. API 支持字符串格式和数字格式的双表示，便于兼容 ABL/ZLM 风格调用
2. 每个 `StreamKey + RecordFormat` 默认单任务独占
3. 删除目录操作只允许清理 catalog 已知路径，禁止通配删除
4. 录制失败必须写入状态与错误原因，不能静默掉线

## 测试要求

1. 单元测试覆盖格式映射、任务唯一性、过期清理和失败状态
2. 集成测试覆盖 FLV/HLS/MP4/PS 启停、切片和 finalize
3. 文件 catalog 测试覆盖排序、范围查询、多文件串联准备数据
4. 回归测试覆盖未 finalize 文件不可见和异常终止清理
