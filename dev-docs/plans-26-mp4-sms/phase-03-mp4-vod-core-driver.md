# Phase 03 — MP4 VOD core / driver / module

- **状态**: 已完成
- **范围**: 新增 `cheetah-mp4-core`、`cheetah-mp4-driver-tokio`、`cheetah-mp4-module`，实现 MP4 文件点播、seek、pause、scale、stop 和 session lifecycle
- **完成标准**: 给定本地 MP4 文件，系统可建立 VOD session，把文件稳定输出为统一媒体帧，并支持协议侧控制

## 实现概览

- `cheetah-mp4-core::VodSession` 是 Sans-I/O 状态机，输入为 `Control/ReadAt/Tick`，输出为 `ReadAt/EmitTrackInfo/EmitFrame/ScheduleTick/CloseSession`，不直接持有 socket 或文件。
- `cheetah-mp4-driver-tokio::open_file` 通过 `tokio::fs::File`（启用 `fs` feature）+ unbounded mpsc 把 core 状态机粘到真实磁盘 I/O；提供 `VodDriverHandle::send_control / take_events`。
- `cheetah-mp4-module::Mp4Module` 通过 `cheetah-sdk::Module` 接入 engine，HTTP 路由前缀 `/api/v1/vod`，路由 `start/control/stop` 与 SMS `VodApi` 对齐。
- `VodSessionRegistry` 提供有界会话管理，`uri` 解析拒绝路径穿越；`VodApi::start` 接受 `file/...` 与 `record/...` namespace。
- 新增 crate 已加入根 workspace；`cargo test -p cheetah-mp4-core/-driver-tokio/-module` 全部通过（共 8 用例）。

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
- `Scale`
- `Stop`

输出只包含：

- `ReadAt`
- `EmitTrackInfo`
- `EmitFrame`
- `FlushPlayback`
- `ScheduleTick`
- `CloseSession`

要求：

- `core` 不直接读文件
- `core` 不直接拿系统时间
- `core` 不直接操作 engine 或协议连接

## 3.3 module 与控制 API

新增路由：

- `POST /api/v1/vod/start`
- `POST /api/v1/vod/control`
- `POST /api/v1/vod/stop`

行为：

- `start` 接收 `uri`、`format`、`startTime`、`endTime`、`loopCount`
- `control` 接收 `seek`、`pause`、`scale`
- `stop` 终止 session 并释放 reader、queue、subscription

`cheetah-sdk` / `cheetah-engine` 新增：

- `VodControlApi`
- `VodSessionSnapshot`
- `VodControlCommand`

## 3.4 文件与 session 管理

- session 通过 `session_id` 唯一标识
- `file/` namespace 代表本地文件 VOD
- `record/` namespace 代表录像文件 VOD
- 默认 target stream key 采用确定性规则，避免多个点播请求互相污染
- session idle/EOF/stop 都必须正确 cleanup

## 3.5 Phase 03 测试要求

- `start/seek/pause/scale/stop` 状态机单元测试
- `moov` 在尾部和多轨 MP4 的 VOD 回归
- seek 后首帧同步点和 timeline 单调性回归
- EOF、loop、stop、非法路径、损坏文件、空轨道回归
- property tests 覆盖 seek 不回退、pause/scale 状态转换
