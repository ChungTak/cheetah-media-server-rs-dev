# Phase 03 — 生产化

- **状态**: 未开始
- **范围**: Player session 超时清理、磁盘录制、配置完善
- **完成标准**: 无人观看时自动释放资源；可录制 VOD HLS 文件；配置热更新生效

---

## 3.1 Player Session UID 超时清理

**问题**: 当前 `ensure_muxer` 在首次请求时创建 muxer + subscriber，但 player 断开后 muxer 仅在流结束时清理。如果流持续推送但无人观看，资源浪费。

**simple-media-server 参考**:
- 每个 player 通过 UID 跟踪（嵌入 playlist URL）
- `_mapPlayer` 记录 `{uid → lastRequestTime}`
- 定时器每 5s 扫描，超过 `playTimeout`(5s) 未请求的 player 淘汰
- 所有 player 消失后触发 `onNoPlayer()` → 清理 muxer

**实现方案**:

```rust
// cheetah-hls-module/src/module.rs
struct PlayerSession {
    last_request_time: Instant,
}

// 在 MuxerMap 旁维护 player 状态
type PlayerMap = Arc<Mutex<HashMap<u64, PlayerSession>>>;
```

**逻辑**:
1. `MediaPlaylistRequested` / `SegmentRequested` 时更新 `last_request_time`
2. 定时任务（每 `session_timeout_secs/2` 秒）扫描过期 session
3. 当某 stream 的所有 session 过期 → 取消 subscriber → 移除 muxer

**改动点**:
- `run_server_loop`: 新增定时清理任务
- `handle_core_event`: 更新 player session 时间戳
- 配置: `session_timeout_secs`（已有）

---

## 3.2 磁盘录制 + VOD Playlist

**问题**: 需要将 HLS 流录制到磁盘，生成可回放的 VOD playlist。

**simple-media-server 参考**: `HlsFileWriter` 将 segment 写入 `{path}/{timestamp}.ts`，关闭时追加 `#EXT-X-ENDLIST`。

**实现方案**:

新增 `recorder.rs` 模块：

```rust
pub struct HlsRecorder {
    output_dir: PathBuf,
    playlist_content: String,
    max_duration: f64,
    segment_count: u64,
}

impl HlsRecorder {
    pub fn write_segment(&mut self, name: &str, duration: f64, data: &[u8]) -> io::Result<()>;
    pub fn finalize(&mut self) -> io::Result<()>; // 写入 EXT-X-ENDLIST
}
```

**配置**:
```yaml
modules:
  hls:
    recording:
      enabled: false
      output_dir: "/var/hls/recordings"
      max_duration_secs: 3600
```

**改动点**:
- `cheetah-hls-module`: 新增 `recorder.rs`
- `run_subscriber`: segment 完成时同时写入 recorder
- 流结束时调用 `recorder.finalize()`

---

## 3.3 配置热更新 + 完整配置项

**问题**: 当前配置变更返回 `ModuleRestartRequired`，需要确保所有配置项完整。

**完整配置模型**:

```yaml
modules:
  hls:
    enabled: true
    listen: "0.0.0.0:8088"
    segment_duration_ms: 4000
    segment_count: 5
    ready_threshold: 1
    force_segment_after_ms: 12000
    session_timeout_secs: 10
    container: "ts"           # "ts" | "fmp4"
    ll_hls:
      enabled: false
      part_duration_ms: 500
    recording:
      enabled: false
      output_dir: ""
      max_duration_secs: 3600
    pull_jobs: []
```

**改动点**: 扩展 `HlsModuleConfig` struct，保持 `ModuleRestartRequired` 语义。

---

## 验证方法

1. Session 超时: 推流 → 拉流 → 停止拉流 → 验证 muxer 在 timeout 后被清理
2. 录制: 推流 → 停止 → 验证磁盘上 .m3u8 + .ts 文件可用 ffplay 播放
3. 配置: 修改配置 → 验证模块重启后新配置生效
