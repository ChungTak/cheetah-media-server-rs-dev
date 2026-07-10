use std::panic::Location;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use cheetah_sdk::{
    CancellationToken, EventBus, SdkError, SystemEvent, TaskEvent, TaskEventKind, TaskId, TaskKind,
    TaskOutcome, TaskSnapshot, TaskState, TaskSystemApi, TaskTerminalOutcome,
};
use dashmap::DashMap;
use parking_lot::RwLock;

/// Internal state and cancellation token for one task.
///
/// 单个任务的内部状态与取消 token。
struct TaskNode {
    snapshot: RwLock<TaskSnapshot>,
    token: CancellationToken,
}

/// In-memory task tree with lifecycle events and cancellation propagation.
///
/// 内存任务树，支持生命周期事件与取消传播。
#[derive(Default)]
pub struct TaskSystem {
    next_id: AtomicU64,
    tasks: DashMap<TaskId, Arc<TaskNode>>,
    event_bus: RwLock<Option<Arc<dyn EventBus>>>,
}

/// Summary of a task finishing, used to propagate completion to parent jobs.
///
/// 任务完成摘要，用于向父任务（job）传播完成状态。
struct TerminalTransition {
    parent_id: Option<TaskId>,
    state: TaskState,
    terminal_outcome: TaskTerminalOutcome,
    message: Option<String>,
}

/// Current timestamp in milliseconds for task lifecycle metadata.
///
/// 任务生命周期元数据使用的当前毫秒时间戳。
fn unix_millis_now() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis() as u64
}

/// Returns true for final task states.
///
/// 判断是否为任务终态。
fn is_terminal_state(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Succeeded | TaskState::Failed | TaskState::Stopped
    )
}

impl TaskSystem {
    /// Attach the event bus used for task lifecycle events.
    ///
    /// 附加用于任务生命周期事件的事件总线。
    pub fn set_event_bus(&self, event_bus: Arc<dyn EventBus>) {
        *self.event_bus.write() = Some(event_bus);
    }

    /// Publish a task event to the event bus if configured.
    ///
    /// 若已配置，则向事件总线发布任务事件。
    fn publish_task_event(&self, event: TaskEvent) {
        if let Some(bus) = self.event_bus.read().as_ref() {
            bus.publish(SystemEvent::Task(event));
        }
    }

    /// Look up a task node by ID.
    ///
    /// 按 ID 查找任务节点。
    fn get_node(&self, task_id: TaskId) -> Result<Arc<TaskNode>, SdkError> {
        self.tasks
            .get(&task_id)
            .map(|v| Arc::clone(v.value()))
            .ok_or_else(|| SdkError::NotFound(format!("task {task_id}")))
    }

    /// Apply a terminal outcome to a task and cancel its token.
    ///
    /// 对任务应用终态结果并取消其 token。
    fn apply_terminal(
        &self,
        task_id: TaskId,
        outcome: TaskOutcome,
    ) -> Result<Option<TerminalTransition>, SdkError> {
        let node = self.get_node(task_id)?;
        let mut snapshot = node.snapshot.write();
        if is_terminal_state(snapshot.state) {
            return Ok(None);
        }

        let now = unix_millis_now();
        snapshot.updated_unix_millis = now;
        snapshot.finished_unix_millis = Some(now);

        let (state, terminal_outcome, message, cancel_reason) = match outcome {
            TaskOutcome::Succeeded => (
                TaskState::Succeeded,
                TaskTerminalOutcome::Succeeded,
                None,
                None,
            ),
            TaskOutcome::Failed(message) => (
                TaskState::Failed,
                TaskTerminalOutcome::Failed,
                Some(message),
                None,
            ),
            TaskOutcome::Cancelled(reason) => (
                TaskState::Stopped,
                TaskTerminalOutcome::Cancelled,
                reason.clone(),
                reason,
            ),
        };

        snapshot.state = state;
        snapshot.terminal_outcome = Some(terminal_outcome);
        snapshot.finish_message = message.clone();
        snapshot.cancel_reason = cancel_reason;
        let parent_id = snapshot.parent_id;
        drop(snapshot);

        node.token.cancel();

        Ok(Some(TerminalTransition {
            parent_id,
            state,
            terminal_outcome,
            message,
        }))
    }

    /// Walk up the parent chain and finalize jobs once all children are terminal.
    ///
    /// 沿父链向上，当所有子任务都到达终态时完成 job。
    fn maybe_finalize_jobs_from(&self, mut parent_id: Option<TaskId>) {
        while let Some(task_id) = parent_id {
            let (next_parent, decision) = match self.tasks.get(&task_id) {
                Some(entry) => {
                    let snapshot = entry.snapshot.read();
                    let next_parent = snapshot.parent_id;
                    if snapshot.kind != TaskKind::Job
                        || is_terminal_state(snapshot.state)
                        || snapshot.child_ids.is_empty()
                    {
                        (next_parent, None)
                    } else {
                        let mut all_terminal = true;
                        let mut has_failed = false;
                        let mut has_cancelled = false;
                        for child_id in &snapshot.child_ids {
                            let Some(child_entry) = self.tasks.get(child_id) else {
                                all_terminal = false;
                                break;
                            };
                            let child_snapshot = child_entry.snapshot.read();
                            if !is_terminal_state(child_snapshot.state) {
                                all_terminal = false;
                                break;
                            }
                            match child_snapshot.terminal_outcome {
                                Some(TaskTerminalOutcome::Failed) => has_failed = true,
                                Some(TaskTerminalOutcome::Cancelled) => has_cancelled = true,
                                _ => {}
                            }
                        }

                        if !all_terminal {
                            (next_parent, None)
                        } else if snapshot.cancel_reason.is_some()
                            || snapshot.state == TaskState::Stopping
                        {
                            (
                                next_parent,
                                Some(TaskOutcome::Cancelled(snapshot.cancel_reason.clone())),
                            )
                        } else if has_failed {
                            (
                                next_parent,
                                Some(TaskOutcome::Failed("child failed".to_string())),
                            )
                        } else if has_cancelled {
                            (
                                next_parent,
                                Some(TaskOutcome::Cancelled(Some("child cancelled".to_string()))),
                            )
                        } else {
                            (next_parent, Some(TaskOutcome::Succeeded))
                        }
                    }
                }
                None => (None, None),
            };

            if let Some(outcome) = decision {
                if let Ok(Some(transition)) = self.apply_terminal(task_id, outcome) {
                    self.publish_task_event(TaskEvent {
                        task_id: task_id.0,
                        kind: TaskEventKind::Finished,
                        state: transition.state,
                        terminal_outcome: Some(transition.terminal_outcome),
                        message: transition.message,
                    });
                }
            }

            parent_id = next_parent;
        }
    }

    /// Recursively cancel a task and all descendants.
    ///
    /// 递归取消任务及其所有子任务。
    fn cancel_recursive(&self, task_id: TaskId, reason: Option<&str>) {
        let Some(entry) = self.tasks.get(&task_id) else {
            return;
        };

        let node = Arc::clone(entry.value());
        let children = {
            let mut snapshot = node.snapshot.write();
            if is_terminal_state(snapshot.state) {
                return;
            }
            snapshot.state = TaskState::Stopping;
            snapshot.updated_unix_millis = unix_millis_now();
            snapshot.cancel_reason = reason.map(ToString::to_string);
            snapshot.child_ids.clone()
        };
        drop(entry);
        node.token.cancel();

        self.publish_task_event(TaskEvent {
            task_id: task_id.0,
            kind: TaskEventKind::Cancelling,
            state: TaskState::Stopping,
            terminal_outcome: None,
            message: reason.map(ToString::to_string),
        });

        for child in children {
            self.cancel_recursive(child, reason);
        }

        if let Ok(Some(transition)) = self.apply_terminal(
            task_id,
            TaskOutcome::Cancelled(reason.map(ToString::to_string)),
        ) {
            self.publish_task_event(TaskEvent {
                task_id: task_id.0,
                kind: TaskEventKind::Finished,
                state: transition.state,
                terminal_outcome: Some(transition.terminal_outcome),
                message: transition.message,
            });
            self.maybe_finalize_jobs_from(transition.parent_id);
        }
    }
}

/// `TaskSystemApi` implementation: task tree lifecycle and cancellation.
///
/// `TaskSystemApi` 实现：任务树生命周期与取消。
impl TaskSystemApi for TaskSystem {
    #[track_caller]
    /// Create a new task with the given parent and initialize its token.
    ///
    /// 用指定父任务创建新任务并初始化其 token。
    fn create_task(
        &self,
        parent_id: Option<TaskId>,
        kind: TaskKind,
        owner: &str,
        label: &str,
    ) -> Result<TaskId, SdkError> {
        let task_id = TaskId(self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let (level, token) = if let Some(pid) = parent_id {
            let parent = self.get_node(pid)?;
            let parent_snapshot = parent.snapshot.read();
            if is_terminal_state(parent_snapshot.state) {
                return Err(SdkError::Conflict(format!(
                    "parent task {} already finished",
                    pid.0
                )));
            }
            let level = parent_snapshot.level.saturating_add(1);
            drop(parent_snapshot);
            (level, parent.token.child_token())
        } else {
            (0, CancellationToken::new())
        };

        let now = unix_millis_now();
        let caller = Location::caller();
        let snapshot = TaskSnapshot {
            id: task_id,
            parent_id,
            kind,
            state: TaskState::Running,
            terminal_outcome: None,
            owner: owner.to_string(),
            label: label.to_string(),
            level,
            child_ids: Vec::new(),
            started_unix_millis: now,
            updated_unix_millis: now,
            finished_unix_millis: None,
            cancel_reason: None,
            finish_message: None,
            spawn_site: format!("{}:{}:{}", caller.file(), caller.line(), caller.column()),
        };

        self.tasks.insert(
            task_id,
            Arc::new(TaskNode {
                snapshot: RwLock::new(snapshot),
                token,
            }),
        );
        self.publish_task_event(TaskEvent {
            task_id: task_id.0,
            kind: TaskEventKind::Created,
            state: TaskState::Running,
            terminal_outcome: None,
            message: Some(format!("{kind:?}:{owner}:{label}")),
        });

        if let Some(pid) = parent_id {
            let parent = self.get_node(pid)?;
            let mut parent_snapshot = parent.snapshot.write();
            parent_snapshot.child_ids.push(task_id);
            parent_snapshot.updated_unix_millis = unix_millis_now();
        }

        Ok(task_id)
    }

    /// Cancel a task and propagate to its descendants.
    ///
    /// 取消任务并传播到其后代。
    fn cancel(&self, task_id: TaskId, reason: Option<&str>) -> Result<(), SdkError> {
        self.get_node(task_id)?;
        self.cancel_recursive(task_id, reason);
        Ok(())
    }

    /// Finish a task and finalize parent jobs if all children are terminal.
    ///
    /// 完成任务；若所有子任务均已终态，则完成父 job。
    fn finish(&self, task_id: TaskId, outcome: TaskOutcome) -> Result<(), SdkError> {
        let transition = self.apply_terminal(task_id, outcome)?.ok_or_else(|| {
            SdkError::Conflict(format!("task {} already in terminal state", task_id.0))
        })?;

        self.publish_task_event(TaskEvent {
            task_id: task_id.0,
            kind: TaskEventKind::Finished,
            state: transition.state,
            terminal_outcome: Some(transition.terminal_outcome),
            message: transition.message.clone(),
        });
        self.maybe_finalize_jobs_from(transition.parent_id);
        Ok(())
    }

    /// Return the cancellation token for a task.
    ///
    /// 返回任务的取消 token。
    fn token(&self, task_id: TaskId) -> Result<CancellationToken, SdkError> {
        let node = self.get_node(task_id)?;
        Ok(node.token.clone())
    }

    /// Return snapshots of all active tasks.
    ///
    /// 返回所有活跃任务的快照。
    fn snapshot(&self) -> Vec<TaskSnapshot> {
        self.tasks
            .iter()
            .map(|entry| entry.value().snapshot.read().clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cheetah_sdk::{EventBus, SystemEvent, TaskEventKind, TaskOutcome, TaskTerminalOutcome};
    use tokio::time::{timeout, Duration};

    use crate::event::LocalEventBus;

    use super::*;

    #[test]
    fn cascades_to_children() {
        let tasks = TaskSystem::default();
        let root = tasks
            .create_task(None, TaskKind::Work, "engine", "root")
            .expect("root");
        let child = tasks
            .create_task(Some(root), TaskKind::Task, "module", "child")
            .expect("child");

        tasks.cancel(root, Some("shutdown")).expect("cancel");

        let snapshots = tasks.snapshot();
        let root_state = snapshots.iter().find(|s| s.id == root).expect("root state");
        let child_state = snapshots
            .iter()
            .find(|s| s.id == child)
            .expect("child state");
        assert_eq!(root_state.state, TaskState::Stopped);
        assert_eq!(child_state.state, TaskState::Stopped);
        assert_eq!(
            root_state.terminal_outcome,
            Some(TaskTerminalOutcome::Cancelled)
        );
        assert_eq!(root_state.finish_message.as_deref(), Some("shutdown"));
        assert!(root_state.finished_unix_millis.is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_emits_finished_event() {
        let tasks = TaskSystem::default();
        let bus = Arc::new(LocalEventBus::new(16));
        tasks.set_event_bus(bus.clone());

        let root = tasks
            .create_task(None, TaskKind::Work, "engine", "root")
            .expect("root");
        let mut sub = bus.subscribe(8);

        tasks.cancel(root, Some("shutdown")).expect("cancel");

        let mut saw_cancelling = false;
        let mut saw_finished = false;
        for _ in 0..6 {
            let event = timeout(Duration::from_millis(50), sub.recv())
                .await
                .expect("event wait")
                .expect("event");
            if let SystemEvent::Task(task_event) = event {
                if task_event.kind == TaskEventKind::Cancelling {
                    saw_cancelling = true;
                }
                if task_event.kind == TaskEventKind::Finished {
                    saw_finished = true;
                }
            }
            if saw_cancelling && saw_finished {
                break;
            }
        }

        assert!(saw_cancelling, "must emit cancelling event");
        assert!(saw_finished, "must emit finished(cancelled) event");
    }

    #[test]
    fn job_auto_finishes_when_children_complete() {
        let tasks = TaskSystem::default();
        let job = tasks
            .create_task(None, TaskKind::Job, "worker", "job")
            .expect("job");
        let c1 = tasks
            .create_task(Some(job), TaskKind::Task, "worker", "c1")
            .expect("c1");
        let c2 = tasks
            .create_task(Some(job), TaskKind::Task, "worker", "c2")
            .expect("c2");

        tasks.finish(c1, TaskOutcome::Succeeded).expect("finish c1");
        let mid = tasks.snapshot();
        let job_mid = mid.iter().find(|s| s.id == job).expect("job snapshot");
        assert_eq!(job_mid.state, TaskState::Running);

        tasks.finish(c2, TaskOutcome::Succeeded).expect("finish c2");
        let snapshots = tasks.snapshot();
        let job_final = snapshots
            .iter()
            .find(|s| s.id == job)
            .expect("job snapshot");
        assert_eq!(job_final.state, TaskState::Succeeded);
        assert_eq!(
            job_final.terminal_outcome,
            Some(TaskTerminalOutcome::Succeeded)
        );
    }
}
