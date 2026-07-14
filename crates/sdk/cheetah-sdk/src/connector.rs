//! Runtime-neutral connector abstraction for pulling/pushing media streams.
//!
//! `ConnectorApi` lets feature modules open protocol handles without depending on
//! a concrete connector implementation. Implementations live in `cheetah-connector`
//! and are injected into `EngineContext` at server startup.
//!
//! 运行时无关的媒体流拉/推 connector 抽象。

use async_trait::async_trait;

use crate::error::SdkError;
use crate::stream::{PublisherSink, SubscriberSource};

/// Direction used when querying connector capability.
///
/// 查询 connector 能力时使用的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorDirection {
    Pull,
    Push,
}

/// Options for opening a pull session.
///
/// 打开拉流会话的选项。
#[derive(Debug, Clone, Default)]
pub struct ConnectorPullOptions {
    pub protocol: Option<String>,
}

/// Options for opening a push session.
///
/// 打开推流会话的选项。
#[derive(Debug, Clone, Default)]
pub struct ConnectorPushOptions {
    pub protocol: Option<String>,
}

/// Abstraction over a runtime connector that can pull/push media streams.
///
/// 可拉/推媒体流的运行时 connector 抽象。
#[async_trait]
pub trait ConnectorApi: Send + Sync {
    /// Open a pull source for the given URL.
    async fn open_pull(
        &self,
        url: &str,
        options: ConnectorPullOptions,
    ) -> Result<Box<dyn SubscriberSource>, SdkError>;

    /// Open a push sink for the given URL.
    async fn open_push(
        &self,
        url: &str,
        options: ConnectorPushOptions,
    ) -> Result<Box<dyn PublisherSink>, SdkError>;

    /// Return `true` if the connector can handle the protocol/direction pair.
    fn supports(&self, protocol: &str, direction: ConnectorDirection) -> bool;
}
