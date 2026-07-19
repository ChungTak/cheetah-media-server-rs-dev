/// Configuration schema for the HTTP-FLV module.
///
/// HTTP-FLV 模块的配置模式。
pub mod config;

/// HTTP-FLV module lifecycle and play/publish integration.
///
/// HTTP-FLV 模块生命周期以及播放/发布集成。
pub mod module;

/// HTTP-FLV pull client and stream supervisor.
///
/// HTTP-FLV 拉流客户端与监管器。
pub mod pull;

pub(crate) mod processing;
pub(crate) mod route;
pub(crate) mod session;

/// HTTP-FLV module configuration.
///
/// HTTP-FLV 模块配置。
pub use config::HttpFlvModuleConfig;

/// Factory for creating an HTTP-FLV module instance.
///
/// 用于创建 HTTP-FLV 模块实例的工厂。
pub use module::HttpFlvModuleFactory;
