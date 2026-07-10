use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::ConfigEffect;
use crate::module::ModuleState;
use crate::stream::DispatchResult;
use crate::task::{TaskState, TaskTerminalOutcome};

/// `ModuleEventKind` enumeration.
/// `ModuleEventKind` 枚举.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleEventKind {
    /// `Created` variant.
    /// `Created` 变体.
    Created,
    /// `Initialized` variant.
    /// `Initialized` 变体.
    Initialized,
    /// `Started` variant.
    /// `Started` 变体.
    Started,
    /// `Stopping` variant.
    /// `Stopping` 变体.
    Stopping,
    /// `Stopped` variant.
    /// `Stopped` 变体.
    Stopped,
    /// `Failed` variant.
    /// `Failed` 变体.
    Failed,
    /// `ConfigApplied` variant.
    /// `ConfigApplied` 变体.
    ConfigApplied,
}

/// `StreamEventKind` enumeration.
/// `StreamEventKind` 枚举.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamEventKind {
    /// `PublisherOpened` variant.
    /// `PublisherOpened` 变体.
    PublisherOpened,
    /// `SubscriberOpened` variant.
    /// `SubscriberOpened` 变体.
    SubscriberOpened,
    /// `SubscriberClosed` variant.
    /// `SubscriberClosed` 变体.
    SubscriberClosed,
    /// `StreamClosed` variant.
    /// `StreamClosed` 变体.
    StreamClosed,
    /// `FrameDropped` variant.
    /// `FrameDropped` 变体.
    FrameDropped,
}

/// `TaskEventKind` enumeration.
/// `TaskEventKind` 枚举.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskEventKind {
    /// `Created` variant.
    /// `Created` 变体.
    Created,
    /// `Cancelling` variant.
    /// `Cancelling` 变体.
    Cancelling,
    /// `Finished` variant.
    /// `Finished` 变体.
    Finished,
}

/// `ModuleEvent` data structure.
/// `ModuleEvent` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleEvent {
    /// `module_id` field of type `String`.
    /// `module_id` 字段，类型为 `String`.
    pub module_id: String,
    /// `kind` field of type `ModuleEventKind`.
    /// `kind` 字段，类型为 `ModuleEventKind`.
    pub kind: ModuleEventKind,
    /// `state` field.
    /// `state` 字段.
    pub state: Option<ModuleState>,
    /// `effect` field.
    /// `effect` 字段.
    pub effect: Option<ConfigEffect>,
    /// `error` field.
    /// `error` 字段.
    pub error: Option<String>,
}

/// `StreamEvent` data structure.
/// `StreamEvent` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamEvent {
    /// `stream_key` field of type `String`.
    /// `stream_key` 字段，类型为 `String`.
    pub stream_key: String,
    /// `kind` field of type `StreamEventKind`.
    /// `kind` 字段，类型为 `StreamEventKind`.
    pub kind: StreamEventKind,
    /// `stream_id` field.
    /// `stream_id` 字段.
    pub stream_id: Option<u64>,
    /// `subscriber_id` field.
    /// `subscriber_id` 字段.
    pub subscriber_id: Option<u64>,
    /// `dispatch_result` field.
    /// `dispatch_result` 字段.
    pub dispatch_result: Option<DispatchResult>,
    /// `message` field.
    /// `message` 字段.
    pub message: Option<String>,
}

/// `TaskEvent` data structure.
/// `TaskEvent` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    /// `task_id` field of type `u64`.
    /// `task_id` 字段，类型为 `u64`.
    pub task_id: u64,
    /// `kind` field of type `TaskEventKind`.
    /// `kind` 字段，类型为 `TaskEventKind`.
    pub kind: TaskEventKind,
    /// `state` field of type `TaskState`.
    /// `state` 字段，类型为 `TaskState`.
    pub state: TaskState,
    /// `terminal_outcome` field.
    /// `terminal_outcome` 字段.
    pub terminal_outcome: Option<TaskTerminalOutcome>,
    /// `message` field.
    /// `message` 字段.
    pub message: Option<String>,
}

/// `ConfigEvent` data structure.
/// `ConfigEvent` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEvent {
    /// `scope` field of type `String`.
    /// `scope` 字段，类型为 `String`.
    pub scope: String,
    /// `version` field of type `u64`.
    /// `version` 字段，类型为 `u64`.
    pub version: u64,
    /// `effect` field.
    /// `effect` 字段.
    pub effect: Option<ConfigEffect>,
    /// `rolled_back` field of type `bool`.
    /// `rolled_back` 字段，类型为 `bool`.
    pub rolled_back: bool,
}

/// `SystemLifecycleEvent` data structure.
/// `SystemLifecycleEvent` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemLifecycleEvent {
    /// `component` field of type `String`.
    /// `component` 字段，类型为 `String`.
    pub component: String,
    /// `phase` field of type `String`.
    /// `phase` 字段，类型为 `String`.
    pub phase: String,
    /// `message` field.
    /// `message` 字段.
    pub message: Option<String>,
}

/// `ProtocolEvent` data structure.
/// `ProtocolEvent` 数据结构.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolEvent {
    /// `protocol` field of type `String`.
    /// `protocol` 字段，类型为 `String`.
    pub protocol: String,
    /// `event_type` field of type `String`.
    /// `event_type` 字段，类型为 `String`.
    pub event_type: String,
    /// `payload` field.
    /// `payload` 字段.
    pub payload: serde_json::Value,
}

/// `SystemEvent` enumeration.
/// `SystemEvent` 枚举.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SystemEvent {
    /// `Module` variant.
    /// `Module` 变体.
    Module(ModuleEvent),
    /// `Stream` variant.
    /// `Stream` 变体.
    Stream(StreamEvent),
    /// `Task` variant.
    /// `Task` 变体.
    Task(TaskEvent),
    /// `Config` variant.
    /// `Config` 变体.
    Config(ConfigEvent),
    /// `System` variant.
    /// `System` 变体.
    System(SystemLifecycleEvent),
    /// Protocol-specific events published by feature modules.
    /// The event type is defined in the module crate; the SDK only
    /// provides the transport envelope.
    Protocol(ProtocolEvent),
}

/// `EventSubscriber` trait.
/// `EventSubscriber` trait.
#[async_trait]
pub trait EventSubscriber: Send {
    async fn recv(&mut self) -> Option<SystemEvent>;
}

/// `EventBus` trait.
/// `EventBus` trait.
pub trait EventBus: Send + Sync {
    fn publish(&self, event: SystemEvent);

    fn subscribe(&self, capacity: usize) -> Box<dyn EventSubscriber>;
}
