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
pub mod error;
pub mod facade;
pub mod idempotency;
pub mod resource_store;
pub mod sqlite;
pub mod store;

pub use blocking::blocking_call;
pub use error::ControlPlaneError;
pub use facade::ControlPlane;
pub use idempotency::{CanonicalDigest, CanonicalRequest, IdempotencyKey, IdempotencyState};
pub use sqlite::SqliteStore;
pub use store::{
    IdempotencyOutcome, IdempotencyRecord, IdempotencyStore, ResourceRecord, ResourceStore,
};
