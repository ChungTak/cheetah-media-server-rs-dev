//! `cheetah-gb28181-module`: Engine integration for GB28181 protocol.
//!
//! `cheetah-gb28181-module`：GB28181 协议引擎集成模块。

/// GB28181 module configuration.
///
/// GB28181 模块配置。
pub mod config;
/// GB28181 module lifecycle and HTTP media API.
///
/// GB28181 模块生命周期与 HTTP 媒体 API。
pub mod module;

/// GB28181 HTTP media request handlers and REST aliases.
///
/// GB28181 HTTP 媒体请求处理器与 REST 别名。
pub mod http_service;

/// Structured media events emitted by the GB28181 module.
///
/// GB28181 模块发出的结构化媒体事件。
pub mod event;

/// Typed GB28181 REST media request DTOs and field aliases.
///
/// GB28181 REST 媒体请求 DTO 与字段别名。
pub mod request;

pub use config::Gb28181ModuleConfig;
pub use module::{Gb28181Module, Gb28181ModuleFactory};
