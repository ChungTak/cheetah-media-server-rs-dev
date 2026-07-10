use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::ConfigEffect;
use crate::module::ModuleState;
use crate::stream::DispatchResult;
use crate::task::{TaskState, TaskTerminalOutcome};

/// Kind of `Module Event`.
/// `Module Event` 的种类。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleEventKind {
    Created,
    Initialized,
    Started,
    Stopping,
    Stopped,
    Failed,
    ConfigApplied,
}

/// Kind of `Stream Event`.
/// `Stream Event` 的种类。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamEventKind {
    PublisherOpened,
    SubscriberOpened,
    SubscriberClosed,
    StreamClosed,
    FrameDropped,
}

/// Kind of `Task Event`.
/// `Task Event` 的种类。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskEventKind {
    Created,
    Cancelling,
    Finished,
}

/// Events produced by the `Module` subsystem.
/// `Module` 子系统产生的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleEvent {
    pub module_id: String,
    pub kind: ModuleEventKind,
    pub state: Option<ModuleState>,
    pub effect: Option<ConfigEffect>,
    pub error: Option<String>,
}

/// Events produced by the `Stream` subsystem.
/// `Stream` 子系统产生的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamEvent {
    pub stream_key: String,
    pub kind: StreamEventKind,
    pub stream_id: Option<u64>,
    pub subscriber_id: Option<u64>,
    pub dispatch_result: Option<DispatchResult>,
    pub message: Option<String>,
}

/// Events produced by the `Task` subsystem.
/// `Task` 子系统产生的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub task_id: u64,
    pub kind: TaskEventKind,
    pub state: TaskState,
    pub terminal_outcome: Option<TaskTerminalOutcome>,
    pub message: Option<String>,
}

/// Events produced by the `Config` subsystem.
/// `Config` 子系统产生的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEvent {
    pub scope: String,
    pub version: u64,
    pub effect: Option<ConfigEffect>,
    pub rolled_back: bool,
}

/// Events produced by the `System Lifecycle` subsystem.
/// `System Lifecycle` 子系统产生的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemLifecycleEvent {
    pub component: String,
    pub phase: String,
    pub message: Option<String>,
}

/// Events produced by the `Protocol` subsystem.
/// `Protocol` 子系统产生的事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolEvent {
    pub protocol: String,
    pub event_type: String,
    pub payload: serde_json::Value,
}

/// Events produced by the `System` subsystem.
/// `System` 子系统产生的事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SystemEvent {
    Module(ModuleEvent),
    Stream(StreamEvent),
    Task(TaskEvent),
    Config(ConfigEvent),
    System(SystemLifecycleEvent),
    /// Protocol-specific events published by feature modules.
    /// The event type is defined in the module crate; the SDK only
    /// provides the transport envelope.
    Protocol(ProtocolEvent),
}

/// `EventSubscriber` trait.
/// `EventSubscriber` trait。
#[async_trait]
pub trait EventSubscriber: Send {
    async fn recv(&mut self) -> Option<SystemEvent>;
}

/// `EventBus` trait.
/// `EventBus` trait。
pub trait EventBus: Send + Sync {
    fn publish(&self, event: SystemEvent);

    fn subscribe(&self, capacity: usize) -> Box<dyn EventSubscriber>;
}
