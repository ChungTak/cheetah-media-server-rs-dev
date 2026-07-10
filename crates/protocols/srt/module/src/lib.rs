//! Engine integration for SRT.

/// `config` module.
/// `config` 模块.
pub mod config;
mod http;
mod metrics;
/// `module` module.
/// `模块` 模块.
pub mod module;

pub use config::SrtModuleConfig;
pub use metrics::{SrtModuleMetrics, SrtModuleMetricsSnapshot};
pub use module::SrtModuleFactory;
