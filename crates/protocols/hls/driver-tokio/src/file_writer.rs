//! HLS file writer: writes TS/fMP4 segments and M3U8 playlists to disk.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;

use bytes::Bytes;
use tokio::fs;

/// Writes HLS segments and playlists to disk with automatic cleanup.
pub struct HlsFileWriter {
    output_dir: PathBuf,
    /// Per-stream tracking of written segment files for cleanup.
    written_segments: VecDeque<PathBuf>,
    /// Maximum segments to retain on disk.
    max_segments: usize,
}

impl HlsFileWriter {
    /// Creates a new `HlsFileWriter` instance.
    /// 创建新的 `HlsFileWriter` 实例。
    pub fn new(output_dir: PathBuf, max_segments: usize) -> Self {
        Self {
            output_dir,
            written_segments: VecDeque::new(),
            max_segments,
        }
    }

    /// Ensure the output directory exists.
    pub async fn init(&self) -> io::Result<()> {
        fs::create_dir_all(&self.output_dir).await
    }

    /// Ensure stream subdirectory exists and return its path.
    async fn ensure_stream_dir(&self, app: &str, stream: &str) -> io::Result<PathBuf> {
        let dir = self.output_dir.join(app).join(stream);
        fs::create_dir_all(&dir).await?;
        Ok(dir)
    }

    /// Write a segment file to disk and track it for cleanup.
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
    pub async fn write_playlist(&self, app: &str, stream: &str, content: &str) -> io::Result<()> {
        let dir = self.ensure_stream_dir(app, stream).await?;
        let target = dir.join("index.m3u8");
        let tmp = dir.join("index.m3u8.tmp");
        fs::write(&tmp, content.as_bytes()).await?;
        fs::rename(&tmp, &target).await
    }

    /// Write the fMP4 init segment.
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
    pub async fn remove_segment(&self, filename: &str) -> io::Result<()> {
        let path = self.output_dir.join(filename);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Read a file from disk (for serving).
    pub async fn read_file(&self, relative_path: &str) -> io::Result<Bytes> {
        let path = self.output_dir.join(relative_path);
        let data = fs::read(&path).await?;
        Ok(Bytes::from(data))
    }

    /// Check if a file exists.
    pub fn file_exists(&self, relative_path: &str) -> bool {
        self.output_dir.join(relative_path).exists()
    }

    /// Get the full path for a file.
    pub fn file_path(&self, filename: &str) -> PathBuf {
        self.output_dir.join(filename)
    }

    /// Remove old segments exceeding max_segments limit.
    async fn cleanup_old_segments(&mut self) {
        while self.written_segments.len() > self.max_segments {
            if let Some(old_path) = self.written_segments.pop_front() {
                let _ = fs::remove_file(&old_path).await;
            }
        }
    }

    /// Clean up entire stream directory (on unpublish).
    pub async fn cleanup_stream(&self, app: &str, stream: &str) {
        let dir = self.output_dir.join(app).join(stream);
        let _ = fs::remove_dir_all(&dir).await;
        // Try to remove parent app dir if empty
        let app_dir = self.output_dir.join(app);
        let _ = fs::remove_dir(&app_dir).await; // fails silently if not empty
    }
}
