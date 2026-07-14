//! In-memory record task & file registry.
//!
//! Holds task metadata, file inventory, and per-task command channels. The
//! registry is intentionally `Send + Sync` so the HTTP service and background
//! workers can both query and mutate it.
//!
//! 内存中的录制任务与文件注册表。
//!
//! 保存任务元数据、文件清单以及每个任务的命令通道。注册表刻意实现为
//! `Send + Sync`，使 HTTP 服务与后台工作线程均可查询和修改。

use std::collections::HashMap;

use parking_lot::RwLock;

use crate::metadata::{RecordFileMetadata, RecordFileQuery, RecordTaskMetadata, RecordTaskState};

/// Errors the registry can return for task/file operations.
///
/// 注册表在任务/文件操作中可能返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RegistryError {
    #[error("task not found: {0}")]
    TaskNotFound(String),
    #[error("task already exists: {0}")]
    DuplicateTask(String),
    #[error("file not found: {0}")]
    FileNotFound(String),
    #[error("registry capacity exceeded ({0})")]
    CapacityExceeded(usize),
}

/// Default in-memory registry. Disk-backed metadata persistence is implemented
/// by `RecordModule` using `metadata_flush_interval_ms`.
///
/// 默认的内存注册表。磁盘持久化元数据由 `RecordModule` 使用
/// `metadata_flush_interval_ms` 实现。
#[derive(Default)]
pub struct RecordRegistry {
    tasks: RwLock<HashMap<String, RecordTaskMetadata>>,
    files: RwLock<HashMap<String, RecordFileMetadata>>,
    capacity: usize,
}

impl RecordRegistry {
    /// Create a new registry with the given task capacity.
    ///
    /// 使用指定的任务容量创建新注册表。
    pub fn new(capacity: usize) -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            files: RwLock::new(HashMap::new()),
            capacity,
        }
    }

    /// Return the configured maximum number of tasks.
    ///
    /// 返回配置的最大任务数。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Return the number of currently tracked tasks.
    ///
    /// 返回当前跟踪的任务数量。
    pub fn task_count(&self) -> usize {
        self.tasks.read().len()
    }

    /// Return the number of currently tracked files.
    ///
    /// 返回当前跟踪的文件数量。
    pub fn file_count(&self) -> usize {
        self.files.read().len()
    }

    /// Insert a new task. Fails if the task id already exists or capacity is full.
    ///
    /// 插入一个新任务。若任务 ID 已存在或容量已满则失败。
    pub fn insert_task(&self, task: RecordTaskMetadata) -> Result<(), RegistryError> {
        let mut tasks = self.tasks.write();
        if tasks.contains_key(&task.task_id) {
            return Err(RegistryError::DuplicateTask(task.task_id.clone()));
        }
        if tasks.len() >= self.capacity {
            return Err(RegistryError::CapacityExceeded(self.capacity));
        }
        tasks.insert(task.task_id.clone(), task);
        Ok(())
    }

    /// Update the lifecycle state of an existing task.
    ///
    /// 更新已有任务的生命周期状态。
    pub fn update_task_state(
        &self,
        task_id: &str,
        state: RecordTaskState,
    ) -> Result<(), RegistryError> {
        let mut tasks = self.tasks.write();
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| RegistryError::TaskNotFound(task_id.to_string()))?;
        task.state = state;
        Ok(())
    }

    /// Remove a task from the registry and return its metadata.
    ///
    /// 从注册表中移除任务并返回其元数据。
    pub fn remove_task(&self, task_id: &str) -> Result<RecordTaskMetadata, RegistryError> {
        self.tasks
            .write()
            .remove(task_id)
            .ok_or_else(|| RegistryError::TaskNotFound(task_id.to_string()))
    }

    /// List all tracked tasks.
    ///
    /// 列出所有已跟踪任务。
    pub fn list_tasks(&self) -> Vec<RecordTaskMetadata> {
        self.tasks.read().values().cloned().collect()
    }

    /// Return a copy of a task's metadata by id.
    ///
    /// 按 ID 返回任务元数据的副本。
    pub fn get_task(&self, task_id: &str) -> Option<RecordTaskMetadata> {
        self.tasks.read().get(task_id).cloned()
    }

    /// Insert a file into the inventory.
    ///
    /// 将文件插入清单。
    pub fn insert_file(&self, file: RecordFileMetadata) -> Result<(), RegistryError> {
        self.files.write().insert(file.file_id.clone(), file);
        Ok(())
    }

    /// Remove a file from the inventory and return its metadata.
    ///
    /// 从清单中移除文件并返回其元数据。
    pub fn remove_file(&self, file_id: &str) -> Result<RecordFileMetadata, RegistryError> {
        self.files
            .write()
            .remove(file_id)
            .ok_or_else(|| RegistryError::FileNotFound(file_id.to_string()))
    }

    /// Query the file inventory with optional filters, sorted by start time.
    ///
    /// 使用可选过滤条件查询文件清单，按开始时间排序。
    pub fn query_files(&self, query: &RecordFileQuery) -> Vec<RecordFileMetadata> {
        let files = self.files.read();
        let mut filtered: Vec<RecordFileMetadata> = files
            .values()
            .filter(|f| filter_file(f, query))
            .cloned()
            .collect();
        filtered.sort_by_key(|f| f.start_time_ms);
        if let Some(limit) = query.limit {
            filtered.truncate(limit as usize);
        }
        filtered
    }
}

/// Apply the query filters to a single file record.
///
/// Substring matching is used for `app`/`stream` so clients can query by prefix
/// without separate indexing.
///
/// 对单条文件记录应用查询过滤。
///
/// `app`/`stream` 使用子串匹配，方便客户端按前缀查询而无需额外索引。
fn filter_file(f: &RecordFileMetadata, query: &RecordFileQuery) -> bool {
    if let Some(app) = &query.app {
        if !f.path.contains(app.as_str()) {
            return false;
        }
    }
    if let Some(stream) = &query.stream {
        if !f.path.contains(stream.as_str()) {
            return false;
        }
    }
    if let Some(format) = query.format {
        if f.format != format {
            return false;
        }
    }
    if let Some(s) = query.start_time_ms {
        if f.end_time_ms < s {
            return false;
        }
    }
    if let Some(e) = query.end_time_ms {
        if f.start_time_ms > e {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::RecordFormatStr;

    fn task(id: &str) -> RecordTaskMetadata {
        RecordTaskMetadata {
            task_id: id.to_string(),
            format: RecordFormatStr::Mp4,
            vhost: cheetah_media_api::ids::DEFAULT_VHOST.to_string(),
            app: "live".to_string(),
            stream: "test".to_string(),
            source_stream_key: "live/test".to_string(),
            state: RecordTaskState::Pending,
            create_time_ms: 0,
            duration_limit_ms: 0,
            segment_duration_ms: 0,
            segment_count_limit: 0,
        }
    }

    #[test]
    fn registry_inserts_and_lists_tasks() {
        let r = RecordRegistry::new(8);
        r.insert_task(task("a")).unwrap();
        r.insert_task(task("b")).unwrap();
        assert_eq!(r.task_count(), 2);
        let mut ids: Vec<_> = r.list_tasks().into_iter().map(|t| t.task_id).collect();
        ids.sort();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn registry_rejects_duplicates_and_overflow() {
        let r = RecordRegistry::new(1);
        r.insert_task(task("x")).unwrap();
        let dup = r.insert_task(task("x")).unwrap_err();
        assert!(matches!(dup, RegistryError::DuplicateTask(_)));
        let cap = r.insert_task(task("y")).unwrap_err();
        assert!(matches!(cap, RegistryError::CapacityExceeded(_)));
    }

    #[test]
    fn file_query_filters_by_format_and_time() {
        let r = RecordRegistry::new(8);
        r.insert_file(RecordFileMetadata {
            file_id: "f1".to_string(),
            task_id: "t1".to_string(),
            format: RecordFormatStr::Mp4,
            vhost: cheetah_media_api::ids::DEFAULT_VHOST.to_string(),
            app: "live".to_string(),
            stream: "test".to_string(),
            path: "/rec/live/test/2024/01/01.mp4".to_string(),
            duration_ms: 1_000,
            size_bytes: 100,
            start_time_ms: 1_000,
            end_time_ms: 2_000,
            track_summary: vec![],
        })
        .unwrap();
        r.insert_file(RecordFileMetadata {
            file_id: "f2".to_string(),
            task_id: "t1".to_string(),
            format: RecordFormatStr::Hls,
            vhost: cheetah_media_api::ids::DEFAULT_VHOST.to_string(),
            app: "live".to_string(),
            stream: "test".to_string(),
            path: "/rec/live/test/2024/01/01.m3u8".to_string(),
            duration_ms: 5_000,
            size_bytes: 500,
            start_time_ms: 4_000,
            end_time_ms: 9_000,
            track_summary: vec![],
        })
        .unwrap();
        let q = RecordFileQuery {
            format: Some(RecordFormatStr::Mp4),
            ..Default::default()
        };
        let results = r.query_files(&q);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_id, "f1");
    }
}
