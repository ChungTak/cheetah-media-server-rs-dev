//! Resource leak observer for the in-memory engine.
//!
//! Gathers snapshots of tasks, streams, modules and media sessions so tests and
//! operators can assert that cancellation, stop and restart paths do not leave
//! orphan runtime objects behind.
//!
//! 资源泄漏观测器。
//! 汇总任务、流、模块与媒体会话的快照，用于验证取消、停止与重启后没有遗留运行时对象。

use cheetah_media_api::command::SessionQuery;
use cheetah_media_api::model::SessionState;
use cheetah_media_api::port::MediaRequestContext;
use cheetah_media_api::processing::{ProcessingJobQuery, ProcessingJobState};
use cheetah_sdk::{
    MediaServices, MediaSessionDirectoryApi, ModuleManagerApi, ModuleState, StreamManagerApi,
    StreamSnapshot, TaskState, TaskSystemApi,
};

/// Summary of runtime objects that are still alive when they should have been
/// cleaned up.
///
/// 应清理但仍在运行的运行时对象摘要。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResourceLeakReport {
    pub active_task_ids: Vec<String>,
    pub active_stream_keys: Vec<String>,
    pub running_module_ids: Vec<String>,
    pub active_processing_job_ids: Vec<String>,
    pub active_session_ids: Vec<String>,
}

impl ResourceLeakReport {
    /// True when no tasks, streams or media sessions are still alive.
    ///
    /// Running modules are intentionally excluded: the report is meant to detect
    /// orphan runtime objects, and modules are expected to be `Running` while the
    /// engine is live. Use `running_module_ids` directly when you need to assert
    /// module shutdown.
    ///
    /// 当没有仍在运行的任务、流或媒体会话时返回 true。
    /// 运行中的模块被排除在外，因为引擎存活时模块本就应该运行；
    /// 如需验证模块已停止，请直接使用 `running_module_ids`。
    pub fn is_clean(&self) -> bool {
        self.active_task_ids.is_empty()
            && self.active_stream_keys.is_empty()
            && self.active_processing_job_ids.is_empty()
            && self.active_session_ids.is_empty()
    }
}

pub struct ResourceLeakObserver;

impl ResourceLeakObserver {
    pub async fn observe(
        task_system: &dyn TaskSystemApi,
        stream_manager: &dyn StreamManagerApi,
        module_manager: &dyn ModuleManagerApi,
        media_services: &MediaServices,
        session_directory: &dyn MediaSessionDirectoryApi,
    ) -> anyhow::Result<ResourceLeakReport> {
        let mut report = ResourceLeakReport::default();

        for task in task_system.snapshot() {
            if matches!(task.state, TaskState::Running | TaskState::Stopping) {
                report.active_task_ids.push(task.id.to_string());
            }
        }

        for (module_id, state) in module_manager.modules() {
            if state == ModuleState::Running {
                report.running_module_ids.push(module_id.0.clone());
            }
        }

        for stream in stream_manager.list_streams().await? {
            if is_stream_active(&stream) {
                report.active_stream_keys.push(stream.key.to_string());
            }
        }

        if let Some(processing) = media_services.processing() {
            let mut query = ProcessingJobQuery {
                page_size: ProcessingJobQuery::MAX_PAGE_SIZE,
                ..Default::default()
            };
            query.clamp_page_size();
            let ctx = MediaRequestContext::default();
            let page = processing.list_jobs(&ctx, query).await?;
            for job in page.items {
                if !matches!(
                    job.state,
                    ProcessingJobState::Stopped | ProcessingJobState::Failed
                ) {
                    report
                        .active_processing_job_ids
                        .push(job.job_id.to_string());
                }
            }
        }

        let ctx = MediaRequestContext::default();
        let session_query = SessionQuery {
            page: 1,
            page_size: SessionQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        let mut collected = 0u64;
        loop {
            let mut query = session_query.clone();
            query.page = (collected / session_query.page_size) + 1;
            let page = session_directory.list_sessions(&ctx, query).await?;
            let page_len = page.items.len() as u64;
            for session in page.items {
                if !matches!(session.state, SessionState::Closed | SessionState::Failed) {
                    report
                        .active_session_ids
                        .push(session.session_id.to_string());
                }
            }
            collected += page_len;
            if collected >= page.total || page_len == 0 {
                break;
            }
        }

        Ok(report)
    }
}

fn is_stream_active(stream: &StreamSnapshot) -> bool {
    stream.publisher_active || stream.subscriber_count > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_clean_requires_no_active_processing_jobs() {
        let mut report = ResourceLeakReport::default();
        assert!(report.is_clean());

        report.active_processing_job_ids.push("job-1".to_string());
        assert!(!report.is_clean());
    }
}
