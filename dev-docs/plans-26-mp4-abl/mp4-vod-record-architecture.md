# 点播与多格式录制总体架构（对标 ABLMediaServer）

- **状态**: 已完成
- **范围**: 定义本地 MP4 点播、统一录制和跨协议回放的分层边界、核心数据流与控制面

## 目标架构

整体沿用本仓库既有分层约束：

1. `cheetah-codec` 负责容器读写、时间戳归一化、参数集缓存、sample 索引与 compat 逻辑
2. `cheetah-record-module` 负责录制任务、文件生命周期、索引和控制面，不直接处理协议状态机
3. `cheetah-mp4-core` 负责 MP4 文件点播状态机，保持 Sans-I/O
4. `cheetah-mp4-driver-tokio` 负责文件读取、调度、计时、背压与任务运行
5. `cheetah-mp4-module` 负责引擎接入、VOD source 发布和跨协议控制绑定

## 数据流

### 录制路径

1. 各协议 module 将输入统一为 `AVFrame + TrackInfo`
2. `cheetah-record-module` 按 stream key 和格式策略创建 writer
3. writer 使用 `cheetah-codec` 输出 `FLV/HLS/MP4/PS`，并预留 `FMP4/TS`
4. 文件 finalize 后更新索引、触发事件、生成回放元数据

### 点播路径

1. 控制面加载本地 MP4 文件或多文件回放清单
2. `cheetah-mp4-driver-tokio` 读取文件 sample，交给 `cheetah-mp4-core` 推进时间线
3. `cheetah-mp4-module` 将输出注册为 VOD media source
4. `RTSP/RTMP/HTTP-FLV/WS-FLV` 从统一 media source 拉取帧并映射协议控制

## 关键模块边界

### `cheetah-codec`

- classic MP4 reader/writer、sample table、seek index、track metadata
- FLV/PS/TS/FMP4/HLS writer 的统一抽象
- ABL 兼容逻辑：AAC ADTS 补齐、G711 时间戳、H264/H265 参数集、真实帧率估算、损坏时间戳修正

### `cheetah-record-module`

- `RecordFormat`、`RecordPolicy`、`RecordSessionId`、`RecordCatalog`
- 按 stream key 管理并发录制任务
- 临时文件、正式文件、切片目录和过期清理
- 录制开始、切片完成、录制完成、录制失败事件

### `cheetah-mp4-core`

- `Open`、`ReadNext`、`SeekTo`、`Pause`、`Resume`、`SetSpeed`、`Stop`
- `read_count`、EOF loop、关键帧 seek、音视频时间线对齐
- 高倍速关键帧输出策略

### `cheetah-mp4-driver-tokio`

- 文件 I/O、定时调度、回放任务池、背压、空闲关闭
- 多文件串联回放和文件头重开
- 控制命令串行化，避免 seek/pause/range 冲突

### `cheetah-mp4-module`

- 暴露 VOD control API
- 将 RTSP `Range/Scale`、RTMP `seek/pause/onPlayCtrl`、HTTP 查询参数映射到统一控制语义
- 维护回放事件和审计字段，兼容 ABL `on_rtsp_replay` 风格信息

## 对外接口新增

1. `RecordFormat` 至少覆盖 `Flv`、`Hls`、`Mp4`、`Ps`，保留 `Fmp4`、`Ts`
2. `RecordControlApi` 支持 `start`、`stop`、`status`、`list`、`delete`
3. `VodControlApi` 支持 `load_mp4`、`seek`、`pause`、`resume`、`set_speed`、`stop`
4. `VodLoadOptions` 包含 `path`、`stream_key`、`read_count`、`start_position_ms`、`compat_profile`
5. `VodCompatProfile` 至少包含 `StrictSpec` 和 `AblCompat`

## ABL 对齐策略

1. 默认回放一次；`read_count = -1` 表示无限循环
2. seek 超出 duration 返回明确错误，不做静默截断
3. 高倍速回放切换为关键帧优先，避免输出无意义的中间帧
4. 文件回放结束时根据循环配置重新回到文件头，而不是重建整条业务链
5. 真正视频帧率在录制和回放阶段都使用统一估算器，避免固定 25fps 假设

## 非目标

1. 不在首版引入转码
2. 不在 `core` 层感知 RTSP/RTMP/HTTP 细节
3. 不将 FFmpeg 类型泄漏到公共 API
