//! Engine integration for SRT.

pub mod config;
mod http;
mod metrics;
pub mod module;

pub use config::SrtModuleConfig;
pub use metrics::{SrtModuleMetrics, SrtModuleMetricsSnapshot};
pub use module::SrtModuleFactory;
