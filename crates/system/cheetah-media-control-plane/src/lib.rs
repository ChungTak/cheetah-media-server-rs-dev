//! Signaling control-plane runtime-neutral facade and durable store traits.
//!
//! `cheetah-media-control-plane` owns:
//!
//! - mutation validation, fencing and capacity orchestration;
//! - durable idempotency and the controlled resource index;
//! - replayable event journal, cursor and reconciliation hooks;
//! - runtime-neutral facade and store traits.
//!
//! Public interfaces do not expose `rusqlite` connections, Tokio tasks, or
//! tonic types. SQLite I/O is isolated behind `RuntimeApi::spawn_blocking` and
//! store implementations keep the connection internal.
//!
//! 信号控制面无运行时依赖的 facade 与持久化 store trait。

pub mod blocking;
pub mod capacity;
pub mod error;
pub mod event_store;
pub mod facade;
pub mod fault;
pub mod idempotency;
pub mod recovery;
pub mod resource_store;
pub mod rollback;
pub mod rollout;
pub mod side_effect;
pub mod sqlite;
pub mod store;

pub use blocking::blocking_call;
pub use capacity::CapacityOrchestrator;
pub use error::ControlPlaneError;
pub use event_store::{EventRecord, EventStore};
pub use facade::ControlPlane;
pub use fault::{
    DeterministicFaultInjector, FaultAction, FaultInjector, FaultPoint, NullFaultInjector,
};
pub use idempotency::{CanonicalDigest, CanonicalRequest, IdempotencyKey, IdempotencyState};
pub use recovery::{
    ConvergeOutcome, ProbeResult, RecoveryEngine, RecoveryLimits, RecoveryReport, ResourceProbe,
};
pub use rollback::{
    RollbackContext, RollbackOutcome, RollbackPolicy, RollbackRequest, RollbackViolation,
    SchemaVersion,
};
pub use side_effect::{RecoveryAction, SideEffectWindow};
pub use sqlite::SqliteStore;
pub use store::{
    IdempotencyOutcome, IdempotencyRecord, IdempotencyStore, ResourceRecord, ResourceStore,
};
