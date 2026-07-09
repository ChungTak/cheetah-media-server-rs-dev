# MP4 点播与多格式录制总体架构设计

- **状态**: 已完成
- **范围**: 固定 MP4 VOD 与统一录制的 crate 边界、共享媒体 API、REST 控制模型、跨协议桥接方式和兼容策略
- **完成标准**: 实现者能够按本文拆出 `cheetah-mp4-*` 与 `cheetah-record-module`，并把容器/时间戳/seek 逻辑收敛到正确层级

## 架构目标

本次能力分成两条主线：

1. **统一录制主线**：对 engine live stream 建立统一录制任务，按格式写出 `FLV/HLS/MP4/PS` 文件，并保存文件元数据和任务状态。
2. **MP4 VOD 主线**：把本地 MP4 文件读取成统一 `AVFrame + TrackInfo`，通过 engine 和现有协议模块对外点播，并支持 seek、pause、scale、stop。

首版实现策略：

- 录制与点播都复用 `AVFrame + TrackInfo`
- 容器读写和 sample/index/compat 尽量下沉到 `cheetah-codec`
- 点播会话与协议桥接放在 `cheetah-mp4-module`
- 统一录制任务和文件索引放在 `cheetah-record-module`

## Crate 与依赖方向

新增目录与 package：

```text
crates/protocols/mp4/
  core/                    # cheetah-mp4-core
  driver-tokio/            # cheetah-mp4-driver-tokio
  module/                  # cheetah-mp4-module
  testing/property-tests/  # cheetah-mp4-property-tests
  fuzz/                    # standalone cargo-fuzz workspace

crates/system/
  cheetah-record-module/   # 统一录制模块
```

依赖方向固定为：

```text
cheetah-mp4-module
  -> cheetah-mp4-driver-tokio
  -> cheetah-mp4-core
  -> cheetah-sdk
  -> cheetah-codec

cheetah-record-module
  -> cheetah-sdk
  -> cheetah-codec
```

约束：

- `cheetah-mp4-core` 不依赖 Tokio、socket、HTTP、database、engine
- `cheetah-record-module` 不直接依赖 `tokio::net`、`tokio::time`、`tokio::sync`
- `cheetah-record-module` 不复制协议模块的打包/去打包逻辑
- `cheetah-codec` 负责 classic MP4、FLV、PS、segment 视图和时间戳模型，不负责 engine 任务编排

## 共享媒体 API

`cheetah-codec` 需要增强：

```rust
pub enum RecordFormat {
    Flv,
    Hls,
    Mp4,
    Ps,
}

pub enum RecordWriteEvent {
    Bytes(Bytes),
    Segment { path_hint: String, bytes: Bytes },
    InitSegment { path_hint: String, bytes: Bytes },
    Playlist { path_hint: String, body: Bytes },
    Diagnostic(RecordDiagnostic),
}

pub trait RecordContainerWriter {
    fn update_tracks(&mut self, tracks: &[TrackInfo]) -> Result<(), RecordError>;
    fn push_frame(&mut self, frame: &AVFrame) -> Result<Vec<RecordWriteEvent>, RecordError>;
    fn finalize(&mut self) -> Result<Vec<RecordWriteEvent>, RecordError>;
}
```

设计要求：

- MP4/FLV/PS 录制 writer 都基于统一 canonical 时间线
- HLS writer 输出 segment 和 playlist 事件，不直接做磁盘 I/O
- H264/H265 参数集缓存、AAC ADTS/ASC、G711/MP3/Opus、VP8/VP9/AV1 视图复用现有 codec helper
- classic MP4 reader 提供 sample index、track map、seek、duration 和多轨 frame 输出

## 录制数据流

录制主路径：

```text
Engine StreamManager
  -> cheetah-record-module subscriber
  -> RecordContainerWriter(format-specific)
  -> file writer / metadata store
  -> record registry
```

规则：

- 一个录制任务绑定一个 source `StreamKey`
- 一个 source 可以并行挂多个任务，但每个任务有独立 bounded 队列
- MP4/FLV/PS 默认写单文件，可配置关键帧切片
- HLS 默认写 fMP4 segment + VOD playlist，可配置 TS legacy 模式
- 所有文件先写 `.part`，finalize 后原子 rename

## MP4 VOD 数据流

MP4 文件点播：

```text
RTSP/RTMP/HTTP-FLV/WS-FLV request
  -> protocol module route/parser
  -> VodControlApi
  -> cheetah-mp4-module session manager
  -> cheetah-mp4-driver-tokio reader
  -> cheetah-codec mp4 demux/index
  -> TrackInfo + AVFrame
  -> Engine StreamManager / direct protocol bridge
```

规则：

- VOD session 以 `session_id` 标识，并绑定 source file、target stream key、loop_count
- seek 后必须清空旧缓冲并从新位置的可用同步点恢复
- pause 只停止 pacing，不销毁 reader
- scale 首版仅支持正值，默认 `1.0`
- RTSP `Range`、RTMP `seek`、HTTP query `seek` 最终统一映射到 `VodControlApi`

## REST API 设计

VOD 路由保持 SMS 兼容：

```text
POST /api/v1/vod/start
POST /api/v1/vod/control
POST /api/v1/vod/stop
```

Record 路由保持 SMS 兼容：

```text
POST /api/v1/record/start
GET  /api/v1/record/list
POST /api/v1/record/stop
GET  /api/v1/record/query
GET  /api/v1/record/file/query
POST /api/v1/record/file/delete
```

输入兼容规则：

- `format` 支持 `flv`、`hls`、`mp4`、`ps`
- `pause` 支持 `true/false` 和 `0/1`
- `scale` 支持字符串或数字
- `seek` 使用毫秒值
- `recordTemplate` 支持 `duration`、`segmentDuration`、`segmentCount`

## 配置草案

```yaml
modules:
  record:
    enabled: true
    root_path: "./record"
    max_tasks: 256
    queue_capacity: 1024
    metadata_flush_interval_ms: 1000
    cleanup_on_start: false
    formats:
      hls:
        default_container: fmp4
        segment_duration_ms: 5000
      mp4:
        faststart_on_close: true
      flv:
        compat_mode: auto
      ps:
        max_tracks: 16

  mp4:
    enabled: true
    root_path: "./record/mp4"
    max_sessions: 256
    read_chunk_bytes: 262144
    prefetch_samples: 64
    max_box_bytes: 8388608
    idle_timeout_ms: 15000
```

## 文件与元数据模型

录制文件布局：

```text
./record/{format}/{app}/{stream}/{yyyy}/{mm}/{dd}/{timestamp}.{ext}
```

元数据至少包含：

- `task_id`
- `format`
- `app`
- `stream`
- `path`
- `duration_ms`
- `size_bytes`
- `start_time_ms`
- `end_time_ms`
- `track_summary`
- `state`

VOD session 至少包含：

- `session_id`
- `source_uri`
- `stream_key`
- `duration_ms`
- `position_ms`
- `paused`
- `scale`
- `loop_count`
- `state`

## 关键兼容策略

- MP4 reader 容忍 `moov` 在尾部，并允许 bounded 扫描定位
- 缺失 `stss` 时按 sample flags 或视频随机访问判断 seek 回退点
- HLS 录制默认用 fMP4，提高 `OPUS/VP8/VP9/AV1` 覆盖面
- FLV 录制优先 Enhanced FLV / domestic compat，不做转码
- PS 录制优先服务 GB28181 互操作，限制在明确映射支持的 codec 组合
