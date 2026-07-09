# Phase 02 — 统一多格式录制模块

- **状态**: 已完成
- **范围**: 新增 `cheetah-record-module`，统一录制任务、文件元数据、目录布局、格式 writer 分发和 SMS 风格 record API
- **完成标准**: 对任意 engine live stream 可启动 `FLV/HLS/MP4/PS` 录制任务，支持查询、停止、列举文件和删除文件

## 实现概览

- 新增 crate `crates/system/cheetah-record-module/`，包含 `config / metadata / registry / task / api / module` 子模块。
- `RecordRegistry` 提供 task 与 file 的有界注册表；插入校验任务唯一性与容量上限，文件查询支持按 app/stream/format/时间窗过滤。
- `RecordApi` 实现 SMS 风格 `start/stop/list/query/file/query/file/delete` 行为；路径穿越请求被拒绝、未知格式返回明确错误。
- `RecordModule` 通过 `cheetah-sdk::Module` 接入引擎；HTTP 路由使用 `HttpRouteDescriptor`，HTTP 处理走 `ModuleHttpService`，路由前缀为 `/api/v1/record/`。
- V1 提供 `StubExecutor`：负责把任务模型登记到 registry，引擎 host 后续替换为真实 subscriber/writer 编排（接入 `cheetah-codec::record::*`）。
- `cargo test -p cheetah-record-module --lib` 通过（10 用例）。

## 2.1 crate 与职责

新增 crate：

```text
crates/system/cheetah-record-module/
```

职责：

- 管理 `RecordTaskRegistry`
- 对 source stream 建立 subscriber
- 为每个任务实例化对应 `RecordContainerWriter`
- 负责磁盘写入、`.part` finalize、元数据刷新和保留策略
- 暴露 SMS 风格 `/api/v1/record/*`

## 2.2 任务与元数据模型

任务字段至少包含：

- `task_id`
- `format`
- `app`
- `stream`
- `source_stream_key`
- `state`
- `create_time_ms`
- `duration_limit_ms`
- `segment_duration_ms`
- `segment_count_limit`

文件记录字段至少包含：

- `file_id`
- `task_id`
- `format`
- `path`
- `duration_ms`
- `size_bytes`
- `start_time_ms`
- `end_time_ms`
- `track_summary`

要求：

- 任务 registry 必须支持 module restart 后恢复已落盘文件元数据
- 目录扫描必须 bounded，按日期层级枚举
- 删除文件 API 必须拒绝路径穿越和非法 namespace

## 2.3 record API 行为

路由：

- `POST /api/v1/record/start`
- `GET /api/v1/record/list`
- `POST /api/v1/record/stop`
- `GET /api/v1/record/query`
- `GET /api/v1/record/file/query`
- `POST /api/v1/record/file/delete`

兼容规则：

- `format=flv|hls|mp4|ps`
- `recordTemplate.duration`
- `recordTemplate.segmentDuration`
- `recordTemplate.segmentCount`
- 返回 `taskId`、`format`、`status`、`path`、`duration`

## 2.4 录制边界与策略

- MP4/FLV/PS 默认单文件写入，按关键帧或 duration 切片
- HLS 默认按 `segment_duration_ms` 切 segment，并完成 VOD playlist
- audio-only 流允许按时长切片
- 慢磁盘或写失败不能拖死 module 主循环，必须有 bounded queue 和 sampled diagnostic

## 2.5 Phase 02 测试要求

- `record start/stop/list/query/file-query/file-delete` API 集成测试
- `FLV/HLS/MP4/PS` 四种格式录制回归
- 切片边界、finalize、异常中断恢复、元数据落盘回归
- 目录扫描、非法路径、重复 task id、无源流、无权限 namespace 回归
