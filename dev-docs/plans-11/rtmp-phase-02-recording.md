# RTMP Phase 02 — FLV 文件录制

- **状态**: 未开始
- **范围**: FLV 文件录制引擎、文件写入、生命周期管理、API 与配置
- **完成标准**: 可通过 API 或配置启动/停止录制，生成合法 FLV 文件，ffprobe 验证通过

---

## 目标

实现 FLV 文件录制能力，支持：

1. 对任意发布流进行 FLV 文件录制
2. 按时长/大小自动分片
3. 通过 HTTP API 动态启停录制
4. 通过配置自动录制指定流

---

## 设计约束

- 录制作为独立模块 `cheetah-record-module`，不嵌入 RTMP module
- 通过 `EngineContext` 订阅流，与协议模块解耦
- FLV tag 生成复用 `cheetah-rtmp-core` 的 `flv.rs`
- 文件 I/O 使用 `spawn_blocking` 避免阻塞事件循环
- 所有缓冲区有上界，写入失败不影响直播流

---

## 任务分解

### 2.1 FLV 录制引擎设计

**目标**: 定义录制引擎的核心抽象和数据流。

**实现**:

1. 录制会话抽象：

```rust
/// 单个录制会话
pub struct RecordSession {
    stream_key: StreamKey,
    config: RecordConfig,
    state: RecordState,
    writer: FlvFileWriter,
    stats: RecordStats,
}

pub enum RecordState {
    /// 等待第一个关键帧
    WaitingKeyframe,
    /// 正在录制
    Recording { start_time: Instant, bytes_written: u64 },
    /// 已停止
    Stopped { reason: StopReason },
}

pub enum StopReason {
    Manual,
    StreamEnded,
    SizeLimitReached,
    DurationLimitReached,
    Error(String),
}
```

2. 录制配置：

```rust
pub struct RecordConfig {
    /// 录制文件存储目录
    pub output_dir: PathBuf,
    /// 文件名模板 (支持 {app}, {stream}, {timestamp} 变量)
    pub filename_template: String,
    /// 最大单文件时长 (秒), 0 = 不限制
    pub max_duration_secs: u64,
    /// 最大单文件大小 (字节), 0 = 不限制
    pub max_size_bytes: u64,
    /// 是否在分片时自动开始新文件
    pub auto_split: bool,
    /// 写入缓冲区大小
    pub write_buffer_size: usize,
}
```

**测试**:
- 单元测试：状态机转换正确性

---

### 2.2 FLV 文件写入实现

**目标**: 实现 FLV 文件的完整写入，包括 header、metadata、config frames、media tags。

**实现**:

1. FLV 文件写入器：

```rust
pub struct FlvFileWriter {
    file: BufWriter<File>,
    has_video: bool,
    has_audio: bool,
    bytes_written: u64,
    duration_ms: u64,
    first_timestamp: Option<u32>,
}

impl FlvFileWriter {
    /// 创建新文件，写入 FLV header
    pub fn create(path: &Path, has_video: bool, has_audio: bool) -> io::Result<Self>;

    /// 写入 metadata tag (onMetaData)
    pub fn write_metadata(&mut self, metadata: &FlvMetadata) -> io::Result<()>;

    /// 写入视频 config (sequence header)
    pub fn write_video_config(&mut self, config: &[u8], timestamp: u32) -> io::Result<()>;

    /// 写入音频 config (sequence header)
    pub fn write_audio_config(&mut self, config: &[u8], timestamp: u32) -> io::Result<()>;

    /// 写入媒体 tag
    pub fn write_tag(&mut self, tag_type: FlvTagType, data: &[u8], timestamp: u32) -> io::Result<()>;

    /// 刷新缓冲区
    pub fn flush(&mut self) -> io::Result<()>;

    /// 关闭文件（更新 metadata 中的 duration）
    pub fn finalize(self) -> io::Result<()>;
}
```

2. 文件名生成：

```rust
fn generate_filename(template: &str, stream_key: &StreamKey, timestamp: &DateTime) -> String {
    template
        .replace("{app}", &stream_key.app)
        .replace("{stream}", &stream_key.stream)
        .replace("{timestamp}", &timestamp.format("%Y%m%d_%H%M%S"))
        .replace("{date}", &timestamp.format("%Y-%m-%d"))
}
```

**测试**:
- 单元测试：FLV header 写入正确性
- 单元测试：FLV tag 写入格式验证
- 集成测试：写入完整 FLV 文件 → ffprobe 验证

---

### 2.3 录制生命周期管理

**目标**: 管理录制会话的完整生命周期，包括自动分片和错误恢复。

**实现**:

1. 录制管理器：

```rust
pub struct RecordManager {
    sessions: HashMap<StreamKey, RecordSession>,
    config: RecordModuleConfig,
}

impl RecordManager {
    /// 开始录制指定流
    pub fn start_recording(&mut self, stream_key: StreamKey, config: RecordConfig) -> Result<()>;

    /// 停止录制指定流
    pub fn stop_recording(&mut self, stream_key: &StreamKey) -> Result<()>;

    /// 处理流媒体帧
    pub fn on_frame(&mut self, stream_key: &StreamKey, frame: &AVFrame) -> Result<()>;

    /// 处理流结束事件
    pub fn on_stream_ended(&mut self, stream_key: &StreamKey);

    /// 检查分片条件
    fn check_split_condition(&self, session: &RecordSession) -> bool;

    /// 执行分片（关闭当前文件，打开新文件）
    fn split_file(&mut self, stream_key: &StreamKey) -> Result<()>;
}
```

2. 分片逻辑：

```
on_frame:
  if check_split_condition(session):
    if frame.is_keyframe():  // 只在关键帧处分片
      split_file()
```

3. 错误处理：
   - 写入失败：记录错误日志，停止该流录制，不影响直播
   - 磁盘满：检测 `ErrorKind::StorageFull`，停止所有录制，发出告警

**测试**:
- 单元测试：分片条件判断
- 单元测试：关键帧对齐分片
- 集成测试：录制 10 秒 → 验证文件完整性

---

### 2.4 录制 API 与配置

**目标**: 提供 HTTP API 和配置文件两种方式控制录制。

**实现**:

1. 配置模型：

```yaml
modules:
  record:
    enabled: true
    output_dir: /data/recordings
    filename_template: "{app}/{stream}/{date}/{timestamp}.flv"
    max_duration_secs: 3600
    max_size_bytes: 0
    auto_split: true
    write_buffer_size: 65536
    auto_record:
      - stream_pattern: "live/*"
        enabled: true
      - stream_pattern: "event/*"
        enabled: true
        max_duration_secs: 7200
```

2. HTTP API：

```
POST   /api/record/start    { "stream_key": "live/test", "config": {...} }
POST   /api/record/stop     { "stream_key": "live/test" }
GET    /api/record/status    → 所有录制会话状态
GET    /api/record/status/{app}/{stream}  → 单个流录制状态
GET    /api/record/files     → 录制文件列表
```

3. 自动录制：
   - 监听引擎的 `StreamPublished` 事件
   - 匹配 `auto_record` 规则
   - 自动启动录制会话

**测试**:
- 集成测试：API 启停录制
- 集成测试：自动录制规则匹配
- 集成测试：配置热更新
