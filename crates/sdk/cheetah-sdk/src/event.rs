use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::ConfigEffect;
use crate::module::ModuleState;
use crate::stream::DispatchResult;
use crate::task::{TaskState, TaskTerminalOutcome};

/// Lifecycle event kinds for a module.
///
/// 模块生命周期事件类型。
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

/// Event kinds related to stream/subscriber lifecycle.
///
/// 流/订阅者生命周期相关事件类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamEventKind {
    PublisherOpened,
    SubscriberOpened,
    SubscriberClosed,
    StreamClosed,
    FrameDropped,
}

/// Event kinds for task lifecycle.
///
/// 任务生命周期事件类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskEventKind {
    Created,
    Cancelling,
    Finished,
}

/// Event emitted when a module changes state or applies config.
///
/// 模块状态变化或应用配置时发出的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleEvent {
    pub module_id: String,
    pub kind: ModuleEventKind,
    pub state: Option<ModuleState>,
    pub effect: Option<ConfigEffect>,
    pub error: Option<String>,
}

/// Event emitted when a stream or subscriber changes.
///
/// 流或订阅者变化时发出的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamEvent {
    pub stream_key: String,
    pub kind: StreamEventKind,
    pub stream_id: Option<u64>,
    pub subscriber_id: Option<u64>,
    pub dispatch_result: Option<DispatchResult>,
    pub message: Option<String>,
}

/// Event emitted when a task changes state.
///
/// 任务状态变化时发出的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub task_id: u64,
    pub kind: TaskEventKind,
    pub state: TaskState,
    pub terminal_outcome: Option<TaskTerminalOutcome>,
    pub message: Option<String>,
}

/// Event emitted when a config patch is applied or rolled back.
///
/// 配置补丁应用或回滚时发出的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEvent {
    pub scope: String,
    pub version: u64,
    pub effect: Option<ConfigEffect>,
    pub rolled_back: bool,
}

/// Event emitted during system startup/shutdown phases.
///
/// 系统启动/关闭阶段发出的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemLifecycleEvent {
    pub component: String,
    pub phase: String,
    pub message: Option<String>,
}

/// Protocol-specific event envelope forwarded by modules.
///
/// 模块转发的协议特定事件信封。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolEvent {
    pub protocol: String,
    pub event_type: String,
    pub payload: serde_json::Value,
}

/// Top-level event carried by the `EventBus`.
///
/// `EventBus` 传递的顶层事件。
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

/// Subscriber of events from an `EventBus`.
///
/// `EventBus` 的事件订阅者。
#[async_trait]
pub trait EventSubscriber: Send {
    async fn recv(&mut self) -> Option<SystemEvent>;
}

/// Bus for publishing and subscribing to system events.
///
/// 发布和订阅系统事件的总线。
pub trait EventBus: Send + Sync {
    fn publish(&self, event: SystemEvent);

    fn subscribe(&self, capacity: usize) -> Box<dyn EventSubscriber>;
}
