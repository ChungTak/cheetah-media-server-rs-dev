# Phase 03 — MP4 VOD core / driver / module

- **状态**: 已完成
- **范围**: 新增 `cheetah-mp4-core`、`cheetah-mp4-driver-tokio`、`cheetah-mp4-module`，实现 MP4 文件点播、seek、pause、speed、stop、repeat 和 session lifecycle
- **完成标准**: 给定本地 MP4 文件、MP4 文件列表或 MP4 目录，系统可建立 VOD session，把文件稳定输出为统一媒体帧，并支持协议侧控制

## 实现概览

- 复用 `plans-26-mp4-sms` Phase 03 已落地的三段式 crate：
  - `cheetah-mp4-core::VodSession` Sans-I/O 状态机（输入 `Control / ReadAt / Tick`，输出 `ReadAt / EmitTrackInfo / EmitFrame / ScheduleTick / CloseSession`）。
  - `cheetah-mp4-driver-tokio::open_file` 把状态机粘到 `tokio::fs::File` 与 mpsc 控制通道。
  - `cheetah-mp4-module::Mp4Module` 通过 `cheetah-sdk::Module` 接入 engine。
- 新增 `zlm_compat` 子模块对接 ZLM 行为：
  - `ZlmLoadMp4 / ZlmSeekRecord / ZlmSetSpeed` 请求模型；`speed` 严格限制 `[0.1, 20.0]`。
  - `normalize_rtmp_mp4_uri`：把 `mp4:0` / `mp4:0.mp4` / `flv:0` 还原为 `0.mp4`，对接 RTMP 客户端的 `mp4:` 前缀习惯。
  - `expand_uri_list`：支持 `;` 分隔的多文件 URI；驱动层 `cheetah-mp4-driver-tokio::open_files` 按序读取并合并为单一 timeline，仅在第一文件时发出 `Tracks` 事件，后续文件复用同一 schema。
  - `ZlmVodCompat::seek_record / set_speed` 直接复用 `VodApi::registry().handle().send_control()` 派发 `Seek / Scale` 命令到 core 状态机。
- HTTP 路由扩展：`/zlm/loadMP4File`、`/zlm/seekRecordStamp`、`/zlm/setRecordSpeed` 与原有 `/start / control / stop` 共存。
- `cargo test -p cheetah-mp4-core / -driver-tokio / -module / -property-tests` 全部通过（4 + 1 + 7 + 6 = 18 用例）。

## 3.1 crate 边界

新增目录与 package：

```text
crates/protocols/mp4/
  core/                    # cheetah-mp4-core
  driver-tokio/            # cheetah-mp4-driver-tokio
  module/                  # cheetah-mp4-module
  testing/property-tests/
  fuzz/
```

职责：

- `core`：VOD session 状态机、request parser、control command、playback pacing 输入输出
- `driver-tokio`：文件 read-at、prefetch、timer、bounded work queue
- `module`：会话管理、API、鉴权、source path 解析、EngineContext 桥接

## 3.2 Sans-I/O VOD session 模型

`core` 只处理：

- `Start`
- `ReadReady`
- `Tick`
- `Seek`
- `Pause`
- `Speed`
- `Stop`
- `Eof`

输出只包含：

- `ReadAt`
- `EmitTrackInfo`
- `EmitFrame`
- `FlushPlayback`
- `ScheduleTick`
- `CloseSession`
- `Diagnostic`

要求：

- `core` 不直接读文件
- `core` 不直接拿系统时间
- `core` 不直接操作 engine 或协议连接
- pacing 使用显式 tick 输入和 speed 参数计算

## 3.3 ZLM 风格 loadMP4File API

新增兼容路由：

- `POST /index/api/loadMP4File`
- `POST /index/api/seekRecordStamp`
- `POST /index/api/setRecordSpeed`

行为：

- `loadMP4File` 接收 `vhost`、`app`、`stream`、`file_path`
- 可选 `seek_ms`、`speed`、`file_repeat`
- 创建 MP4 VOD session 并发布为 engine stream
- 返回 `duration_ms`

## 3.4 Cheetah VOD API

新增路由：

- `POST /api/v1/vod/start`
- `POST /api/v1/vod/control`
- `POST /api/v1/vod/stop`

行为：

- `start` 接收 `uri`、`format`、`startTime`、`endTime`、`loopCount`
- `control` 接收 `seek`、`pause`、`speed`
- `stop` 终止 session 并释放 reader、queue、subscription

`cheetah-sdk` / `cheetah-engine` 新增：

- `VodControlApi`
- `VodSessionSnapshot`
- `VodControlCommand`

## 3.5 多文件与目录回放

对齐 ZLM `MultiMP4Demuxer`：

- 文件路径是目录时，扫描目录下 `.mp4` 并按文件名排序
- 文件路径包含 `;` 时，按分号分隔顺序串联
- 总 duration 是各文件 duration 累加
- seek 按总 timeline 定位到对应文件，再在文件内 seek
- 切换文件时 track 信息以第一个文件为基准，不兼容轨道变化返回 diagnostic

## 3.6 Phase 03 测试要求

- `start/seek/pause/speed/stop/repeat` 状态机单元测试
- `loadMP4File` API 回归
- `moov` 在尾部和多轨 MP4 的 VOD 回归
- 多文件串联 duration、seek、EOF 切换回归
- seek 后首帧同步点和 timeline 单调性回归
- EOF、repeat、stop、非法路径、损坏文件、空轨道回归
- property tests 覆盖 seek 不回退、pause/speed 状态转换
