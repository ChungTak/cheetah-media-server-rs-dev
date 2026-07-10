//! In-memory record task & file registry.
//!
//! Holds task metadata, file inventory, and per-task command channels. The
//! registry is intentionally `Send + Sync` so the HTTP service and background
//! workers can both query and mutate it.

use std::collections::HashMap;

use parking_lot::RwLock;

use crate::metadata::{RecordFileMetadata, RecordFileQuery, RecordTaskMetadata, RecordTaskState};

/// `RegistryError` enumeration.
/// `RegistryError` 枚举.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// `TaskNotFound` variant.
    /// `TaskNotFound` 变体.
    #[error("task not found: {0}")]
    TaskNotFound(String),
    /// `DuplicateTask` variant.
    /// `DuplicateTask` 变体.
    #[error("task already exists: {0}")]
    DuplicateTask(String),
    /// `FileNotFound` variant.
    /// `FileNotFound` 变体.
    #[error("file not found: {0}")]
    FileNotFound(String),
    /// `CapacityExceeded` variant.
    /// `CapacityExceeded` 变体.
    #[error("registry capacity exceeded ({0})")]
    CapacityExceeded(usize),
}

/// Default in-memory registry. Disk-backed metadata persistence is implemented
/// by `RecordModule` using `metadata_flush_interval_ms`.
#[derive(Default)]
pub struct RecordRegistry {
    /// `tasks` field.
    /// `tasks` 字段.
    tasks: RwLock<HashMap<String, RecordTaskMetadata>>,
    /// `files` field.
    /// `files` 字段.
    files: RwLock<HashMap<String, RecordFileMetadata>>,
    /// `capacity` field of type `usize`.
    /// `capacity` 字段，类型为 `usize`.
    capacity: usize,
}

impl RecordRegistry {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(capacity: usize) -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            files: RwLock::new(HashMap::new()),
            capacity,
        }
    }

    /// `capacity` function.
    /// `capacity` 函数.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// `task_count` function.
    /// `task_count` 函数.
    pub fn task_count(&self) -> usize {
        self.tasks.read().len()
    }

    /// `file_count` function.
    /// `file_count` 函数.
    pub fn file_count(&self) -> usize {
        self.files.read().len()
    }

    /// `insert_task` function.
    /// `insert_task` 函数.
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

    /// `update_task_state` function.
    /// `update_task_state` 函数.
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

    /// `remove_task` function.
    /// `remove_task` 函数.
    pub fn remove_task(&self, task_id: &str) -> Result<RecordTaskMetadata, RegistryError> {
        self.tasks
            .write()
            .remove(task_id)
            .ok_or_else(|| RegistryError::TaskNotFound(task_id.to_string()))
    }

    /// `list_tasks` function.
    /// `list_tasks` 函数.
    pub fn list_tasks(&self) -> Vec<RecordTaskMetadata> {
        self.tasks.read().values().cloned().collect()
    }

    /// Returns the `task` value.
    /// 返回 `task` 值.
    pub fn get_task(&self, task_id: &str) -> Option<RecordTaskMetadata> {
        self.tasks.read().get(task_id).cloned()
    }

    /// `insert_file` function.
    /// `insert_file` 函数.
    pub fn insert_file(&self, file: RecordFileMetadata) -> Result<(), RegistryError> {
        self.files.write().insert(file.file_id.clone(), file);
        Ok(())
    }

    /// `remove_file` function.
    /// `remove_file` 函数.
    pub fn remove_file(&self, file_id: &str) -> Result<RecordFileMetadata, RegistryError> {
        self.files
            .write()
            .remove(file_id)
            .ok_or_else(|| RegistryError::FileNotFound(file_id.to_string()))
    }

    /// `query_files` function.
    /// `query_files` 函数.
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
