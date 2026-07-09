# Phase 02 — 统一多格式录制模块

- **状态**: 已完成
- **范围**: 新增 `cheetah-record-module`，统一录制任务、文件元数据、目录布局、格式 writer 分发和 ZLM 风格 record API
- **完成标准**: 对任意 engine live stream 可启动 `FLV/HLS/MP4/PS` 录制任务，支持查询、停止、列举文件和删除文件

## 实现概览

- 复用 `plans-26-mp4-sms` Phase 02 已落地的 `crates/system/cheetah-record-module/`（`config / metadata / registry / task / api / module`）。
- 新增 `zlm_compat` 子模块，提供 ZLM `WebApi.cpp` 风格的请求/响应模型与处理器：
  - `ZlmStartRecord` / `ZlmStopRecord` / `ZlmIsRecording` / `ZlmGetMp4Files` / `ZlmDeleteDirectory`。
  - `parse_zlm_type` 同时支持数字 `type`（0=mp4 / 1=hls / 2=hls-fmp4 / 3=fmp4）与字符串 `type`。
  - `validate_customized_path` 拒绝 `..`、绝对路径、Windows 反斜杠，符合 §5.3 安全要求。
  - `apply_period` 把 `period=YYYY-MM` 与 `period=YYYY-MM-DD` 转成 `start_time_ms / end_time_ms` 范围。
  - `delete_record_directory` 内部走 `RecordApi::query_files` + 逐项 `delete_file`，集中复用安全检查。
- `RecordModule::http_routes` 新增 `/zlm/{startRecord, stopRecord, isRecording, getMP4RecordFile, deleteRecordDirectory}` 路由，与 `/start /stop /list /query /file/query /file/delete` 共存。
- 标准响应包络 `ZlmResponse { code, msg, result }` 对齐 ZLM 客户端期望的 JSON 形态。
- `cargo test -p cheetah-record-module --lib` 16 用例全部通过（含 6 个 zlm_compat 用例）。

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
- 暴露 ZLM 风格 `/index/api/*` 和 Cheetah `/api/v1/record/*`

## 2.2 任务与元数据模型

任务字段至少包含：

- `task_id`
- `format`
- `vhost`
- `app`
- `stream`
- `source_stream_key`
- `state`
- `create_time_ms`
- `duration_limit_ms`
- `segment_duration_ms`
- `segment_count_limit`
- `customized_path`

文件记录字段至少包含：

- `file_id`
- `task_id`
- `format`
- `path`
- `url`
- `duration_ms`
- `size_bytes`
- `start_time_ms`
- `end_time_ms`
- `track_summary`

要求：

- 任务 registry 必须支持 module restart 后恢复已落盘文件元数据
- 目录扫描必须 bounded，按日期层级枚举
- 删除文件 API 必须拒绝路径穿越和非法 namespace
- 录制中的临时文件不能被 list/query 当作可播放文件返回

## 2.3 ZLM 风格 record API 行为

路由：

- `POST /index/api/startRecord`
- `POST /index/api/startRecordTask`
- `POST /index/api/stopRecord`
- `GET /index/api/isRecording`
- `GET /index/api/getMP4RecordFile`
- `POST /index/api/deleteRecordDirectory`

兼容规则：

- `type` 支持 ZLM 数字值和字符串值
- `customized_path` 允许覆盖默认根目录，但必须做路径归一化和安全检查
- `max_second` 映射 MP4 切片时长
- `period=yyyy-mm` 返回日期目录
- `period=yyyy-mm-dd` 返回文件列表

## 2.4 Cheetah 统一 record API 行为

路由：

- `POST /api/v1/record/start`
- `GET /api/v1/record/list`
- `POST /api/v1/record/stop`
- `GET /api/v1/record/query`
- `GET /api/v1/record/file/query`
- `POST /api/v1/record/file/delete`

兼容规则：

- `format=flv|hls|hls_fmp4|mp4|ps|ts|fmp4`
- 首批验收 `flv|hls|mp4|ps`
- 返回 `taskId`、`format`、`status`、`path`、`url`、`duration`

## 2.5 录制边界与策略

- MP4/FLV/PS 默认单文件写入，按关键帧或 duration 切片
- HLS 默认按 `segment_duration_ms` 切 segment，并完成 VOD playlist
- audio-only 流允许按时长切片
- 慢磁盘或写失败不能拖死 module 主循环，必须有 bounded queue 和 sampled diagnostic
- MP4 finalize 和 faststart 在冷路径异步执行
- 小于最小阈值的坏文件删除并记录 diagnostic

## 2.6 Phase 02 测试要求

- `startRecord/stopRecord/isRecording/getMP4RecordFile/deleteRecordDirectory` API 集成测试
- `/api/v1/record/*` API 集成测试
- `FLV/HLS/MP4/PS` 四种格式录制回归
- 切片边界、finalize、异常中断恢复、元数据落盘回归
- 目录扫描、非法路径、重复 task id、无源流、无权限 namespace 回归
