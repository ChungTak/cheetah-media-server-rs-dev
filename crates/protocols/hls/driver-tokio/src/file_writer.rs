//! HLS file writer: writes TS/fMP4 segments and M3U8 playlists to disk.
//!
//! HLS 文件写入器：将 TS/fMP4 分片与 M3U8 播放列表写入磁盘。

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;

use bytes::Bytes;
use tokio::fs;

/// Writes HLS segments and playlists to disk with automatic cleanup.
///
/// 将 HLS 分片与播放列表写入磁盘并自动清理。
pub struct HlsFileWriter {
    output_dir: PathBuf,
    /// Per-stream tracking of written segment files for cleanup.
    ///
    /// 用于清理的每个流的分片文件跟踪。
    written_segments: VecDeque<PathBuf>,
    /// Maximum segments to retain on disk.
    ///
    /// 磁盘保留的最大分片数。
    max_segments: usize,
}

/// `HlsFileWriter` API.
///
/// `HlsFileWriter` API。
impl HlsFileWriter {
    /// Create a new writer for the given output directory and retention limit.
    ///
    /// 为指定输出目录与保留上限创建新的写入器。
    pub fn new(output_dir: PathBuf, max_segments: usize) -> Self {
        Self {
            output_dir,
            written_segments: VecDeque::new(),
            max_segments,
        }
    }

    /// Ensure the output directory exists.
    ///
    /// 确保输出目录存在。
    pub async fn init(&self) -> io::Result<()> {
        fs::create_dir_all(&self.output_dir).await
    }

    /// Ensure stream subdirectory exists and return its path.
    ///
    /// 确保流子目录存在并返回其路径。
    async fn ensure_stream_dir(&self, app: &str, stream: &str) -> io::Result<PathBuf> {
        let dir = self.output_dir.join(app).join(stream);
        fs::create_dir_all(&dir).await?;
        Ok(dir)
    }

    /// Write a segment file to disk and track it for cleanup.
    ///
    /// 将分片文件写入磁盘并加入清理跟踪。
    pub async fn write_segment(
        &mut self,
        app: &str,
        stream: &str,
        filename: &str,
        data: &Bytes,
    ) -> io::Result<()> {
        let dir = self.ensure_stream_dir(app, stream).await?;
        let path = dir.join(filename);
        fs::write(&path, data).await?;
        self.written_segments.push_back(path);
        self.cleanup_old_segments().await;
        Ok(())
    }

    /// Write the M3U8 playlist file atomically (write tmp + rename).
    ///
    /// 原子方式写入 M3U8 播放列表（先写临时文件再重命名）。
    pub async fn write_playlist(&self, app: &str, stream: &str, content: &str) -> io::Result<()> {
        let dir = self.ensure_stream_dir(app, stream).await?;
        let target = dir.join("index.m3u8");
        let tmp = dir.join("index.m3u8.tmp");
        fs::write(&tmp, content.as_bytes()).await?;
        fs::rename(&tmp, &target).await
    }

    /// Write the fMP4 init segment.
    ///
    /// 写入 fMP4 init 分片。
    pub async fn write_init_segment(
        &self,
        app: &str,
        stream: &str,
        data: &Bytes,
    ) -> io::Result<()> {
        let dir = self.ensure_stream_dir(app, stream).await?;
        let path = dir.join("init.mp4");
        fs::write(&path, data).await
    }

    /// Remove a segment file from disk.
    ///
    /// 从磁盘删除分片文件。
    pub async fn remove_segment(&self, filename: &str) -> io::Result<()> {
        let path = self.output_dir.join(filename);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Read a file from disk (for serving).
    ///
    /// 从磁盘读取文件（用于服务）。
    pub async fn read_file(&self, relative_path: &str) -> io::Result<Bytes> {
        let path = self.output_dir.join(relative_path);
        let data = fs::read(&path).await?;
        Ok(Bytes::from(data))
    }

    /// Check if a file exists.
    ///
    /// 检查文件是否存在。
    pub fn file_exists(&self, relative_path: &str) -> bool {
        self.output_dir.join(relative_path).exists()
    }

    /// Get the full path for a file.
    ///
    /// 获取文件的完整路径。
    pub fn file_path(&self, filename: &str) -> PathBuf {
        self.output_dir.join(filename)
    }

    /// Remove old segments exceeding max_segments limit.
    ///
    /// 删除超过 `max_segments` 限制的旧分片。
    async fn cleanup_old_segments(&mut self) {
        while self.written_segments.len() > self.max_segments {
            if let Some(old_path) = self.written_segments.pop_front() {
                let _ = fs::remove_file(&old_path).await;
            }
        }
    }

    /// Clean up entire stream directory (on unpublish).
    ///
    /// 清理整个流目录（取消发布时）。
    pub async fn cleanup_stream(&self, app: &str, stream: &str) {
        let dir = self.output_dir.join(app).join(stream);
        let _ = fs::remove_dir_all(&dir).await;
        // Try to remove parent app dir if empty
        let app_dir = self.output_dir.join(app);
        let _ = fs::remove_dir(&app_dir).await; // fails silently if not empty
    }
}
