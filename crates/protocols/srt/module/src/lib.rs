//! Engine integration for the SRT protocol.
//!
//! SRT 协议的引擎集成。

pub mod config;
mod http;
mod metrics;
pub mod module;

pub use config::SrtModuleConfig;
pub use metrics::{SrtModuleMetrics, SrtModuleMetricsSnapshot};
pub use module::SrtModuleFactory;
