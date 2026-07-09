# Phase 02 — 磁盘切片模式

- **状态**: 未开始
- **目标**: 对标 ABLMediaServer 的磁盘切片模式（hlsCutType=1），实现 segment 文件写入磁盘、m3u8 文件生成、目录管理、自动清理、内存/磁盘混合模式
- **影响 crate**: `cheetah-hls-core`（切片策略）、`cheetah-hls-driver-tokio`（文件 I/O）、`cheetah-hls-module`（模式编排）
- **参考源**: `MediaStreamSource.cpp` (SaveTsMp4M3u8File / InitHlsResoure)、`NetServerHLS.cpp` (磁盘读取路径)

---

## 1. 磁盘切片模式概述

### ABL 实现分析

ABLMediaServer `hlsCutType=1` 模式：
- segment 文件写入 `{wwwPath}/{app}/{stream}/N.ts`（或 `N.mp4`）
- m3u8 文件写入 `{wwwPath}/{app}/{stream}/hls.m3u8`
- 超过 `nMaxTsFileCount`（默认 20）个文件时删除最旧的
- 媒体源销毁时清理整个目录
- HTTP 请求时直接从磁盘读取文件

### 本地现状

当前仅支持内存模式（`SegmentRing`），segment 数据全部在内存中。已有 `HlsFileWriter` 在 driver 层但未接入 module。

---

## 2. 目录结构设计

```
{hls_root_path}/
├── {app}/
│   └── {stream}/
│       ├── index.m3u8          # 媒体 playlist
│       ├── init.mp4            # fMP4 init segment（仅 fMP4 模式）
│       ├── seg_0.ts            # segment 文件
│       ├── seg_1.ts
│       ├── seg_2.ts
│       └── ...
```

**配置项：**
```yaml
hls:
  storage_mode: "memory"       # "memory" | "disk" | "hybrid"
  disk_root_path: "./hls_output"
  max_disk_segments: 20        # 磁盘最大保留 segment 数
  cleanup_on_unpublish: true   # 流结束时清理目录
```

---

## 3. 实现方案

### 3.1 Core 层：切片模式抽象

在 `cheetah-hls-core` 中定义存储策略接口：

```rust
pub enum StorageMode {
    Memory,
    Disk,
    Hybrid,  // 内存保留最近 N 个 + 磁盘保留全部
}

pub struct SegmentOutput {
    pub name: String,
    pub data: Bytes,
    pub duration_ms: u64,
    pub is_keyframe_start: bool,
}
```

Core 层不关心存储位置，只产出 `SegmentOutput`。

### 3.2 Driver 层：文件写入

扩展已有的 `HlsFileWriter`：

```rust
pub struct DiskSegmentWriter {
    root_path: PathBuf,
    max_segments: usize,
    written_segments: VecDeque<PathBuf>,
}

impl DiskSegmentWriter {
    /// 写入 segment 文件，返回文件路径
    pub async fn write_segment(&mut self, app: &str, stream: &str, seg: &SegmentOutput) -> io::Result<PathBuf>;
    
    /// 写入 m3u8 文件（原子写入：先写临时文件再 rename）
    pub async fn write_playlist(&self, app: &str, stream: &str, content: &str) -> io::Result<()>;
    
    /// 写入 init segment（fMP4 模式）
    pub async fn write_init_segment(&self, app: &str, stream: &str, data: &[u8]) -> io::Result<()>;
    
    /// 清理超出限制的旧 segment
    pub async fn cleanup_old_segments(&mut self) -> io::Result<Vec<PathBuf>>;
    
    /// 清理整个流目录
    pub async fn cleanup_stream_dir(&self, app: &str, stream: &str) -> io::Result<()>;
}
```

**关键实现细节：**
- m3u8 写入使用原子操作（write to `.tmp` + rename），避免客户端读到半写文件
- segment 写入使用 `tokio::fs::File` 异步 I/O
- 目录不存在时自动创建（`tokio::fs::create_dir_all`）

### 3.3 Module 层：模式编排

```rust
// module/muxer.rs 扩展
match config.storage_mode {
    StorageMode::Memory => {
        segment_ring.push(segment);
    }
    StorageMode::Disk => {
        disk_writer.write_segment(app, stream, &segment).await?;
        disk_writer.write_playlist(app, stream, &playlist_content).await?;
        disk_writer.cleanup_old_segments().await?;
    }
    StorageMode::Hybrid => {
        segment_ring.push(segment.clone());
        disk_writer.write_segment(app, stream, &segment).await?;
        disk_writer.write_playlist(app, stream, &playlist_content).await?;
        disk_writer.cleanup_old_segments().await?;
    }
}
```

### 3.4 HTTP 服务：磁盘读取路径

磁盘模式下，segment 请求直接从文件系统读取：

```rust
// driver/server.rs
async fn serve_segment_from_disk(path: &Path, stream: &mut TcpStream) -> io::Result<()> {
    let file = tokio::fs::File::open(path).await?;
    let metadata = file.metadata().await?;
    let size = metadata.len();
    
    // 写 HTTP 头
    write_response_headers(stream, 200, size, "video/mp2t").await?;
    
    // 分片读取并发送（128KB chunks）
    let mut reader = tokio::io::BufReader::new(file);
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 { break; }
        stream.write_all(&buf[..n]).await?;
    }
    Ok(())
}
```

---

## 4. 自动清理策略

### 4.1 Segment 数量限制

当磁盘 segment 数量超过 `max_disk_segments` 时，删除最旧的文件：

```rust
fn cleanup_old_segments(&mut self) {
    while self.written_segments.len() > self.max_segments {
        if let Some(old_path) = self.written_segments.pop_front() {
            tokio::fs::remove_file(old_path).await.ok();
        }
    }
}
```

### 4.2 流结束清理

当流 unpublish 时（`cleanup_on_unpublish=true`）：
- 删除所有 segment 文件
- 删除 m3u8 文件
- 删除 init.mp4（如有）
- 删除空目录

### 4.3 启动时清理

服务启动时扫描 `disk_root_path`，清理无对应活跃流的残留目录。

---

## 5. 混合模式 (Hybrid)

同时维护内存 ring 和磁盘文件：
- 内存 ring 用于快速响应实时请求（最近 5 个 segment）
- 磁盘用于长时间保留（最多 20 个 segment）
- HTTP 请求优先从内存 ring 获取，miss 时回退到磁盘读取

---

## 验收标准

- [ ] 磁盘模式：segment 文件正确写入指定目录
- [ ] 磁盘模式：m3u8 文件内容正确，路径指向正确的 segment 文件
- [ ] 磁盘模式：超过 20 个 segment 时自动删除最旧文件
- [ ] 磁盘模式：流结束时目录被清理
- [ ] 混合模式：内存 miss 时从磁盘读取成功
- [ ] 原子写入：m3u8 不会出现半写状态
- [ ] fMP4 模式：init.mp4 正确写入并可被请求

---

## 测试计划

```bash
# 单元测试
cargo test -p cheetah-hls-driver-tokio

# 集成测试
cargo test -p cheetah-hls-module

# 手动验证
ls -la ./hls_output/live/test/
cat ./hls_output/live/test/index.m3u8
ffplay http://127.0.0.1:8088/live/test.m3u8
```
