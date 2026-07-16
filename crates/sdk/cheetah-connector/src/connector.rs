use std::sync::Arc;

use async_trait::async_trait;
use cheetah_engine::Engine;

use crate::error::ConnectorError;
#[cfg(feature = "loopback")]
use crate::handles::LoopbackPair;
use crate::handles::{PullHandle, PushHandle};
#[cfg(feature = "loopback")]
use crate::options::LoopbackOptions;
use crate::options::{ConnectorPullOptions, ConnectorPushOptions};
use crate::protocol::{supports, Direction, Protocol};

/// High-level connector trait implemented by runtime-backed connectors.
///
/// 运行时支持的 connector 实现的高层 trait。
#[async_trait]
pub trait RuntimeConnector: Send + Sync {
    /// Open a pull handle for the given protocol and URL.
    ///
    /// 为给定协议和 URL 打开 pull 句柄。
    async fn open_pull(
        &self,
        protocol: Protocol,
        url: &str,
        options: ConnectorPullOptions,
    ) -> Result<PullHandle, ConnectorError>;

    /// Open a push handle for the given protocol and URL.
    ///
    /// 为给定协议和 URL 打开 push 句柄。
    async fn open_push(
        &self,
        protocol: Protocol,
        url: &str,
        options: ConnectorPushOptions,
    ) -> Result<PushHandle, ConnectorError>;
}

/// Default connector implementation backed by an embedded `Engine`.
///
/// 由嵌入式 `Engine` 支持的默认 connector 实现。
pub struct EngineConnector {
    engine: Arc<Engine>,
}

impl EngineConnector {
    /// Create a connector from an existing engine.
    ///
    /// 从已有引擎创建 connector。
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }

    /// Returns a reference to the underlying engine.
    ///
    /// 返回底层引擎的引用。
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Start the embedded engine.
    ///
    /// 启动嵌入式引擎。
    pub async fn start(&self) -> Result<(), ConnectorError> {
        self.engine
            .start()
            .await
            .map_err(|e| ConnectorError::Internal(format!("engine start failed: {e}")))
    }

    /// Stop the embedded engine.
    ///
    /// 停止嵌入式引擎。
    pub async fn stop(&self) {
        self.engine.stop().await;
    }

    /// Shuts down the connector and embedded engine.
    ///
    /// 关闭 connector 和嵌入式引擎。
    pub async fn shutdown(self) -> Result<(), ConnectorError> {
        self.engine.stop().await;
        Ok(())
    }

    /// Returns `true` if the connector capability matrix supports this pair.
    ///
    /// 返回 connector 能力矩阵是否支持该组合。
    pub fn supports(protocol: Protocol, direction: Direction) -> bool {
        supports(protocol, direction)
    }

    /// Open an in-memory loopback pair (RTMP push -> HTTP-FLV pull).
    ///
    /// 打开内存 loopback 对（RTMP push -> HTTP-FLV pull）。
    #[cfg(feature = "loopback")]
    pub async fn open_in_memory_loopback(
        &self,
        options: LoopbackOptions,
    ) -> Result<LoopbackPair, ConnectorError> {
        crate::loopback::open_in_memory_loopback(self.engine.clone(), options).await
    }
}

#[async_trait]
impl RuntimeConnector for EngineConnector {
    async fn open_pull(
        &self,
        protocol: Protocol,
        url: &str,
        options: ConnectorPullOptions,
    ) -> Result<PullHandle, ConnectorError> {
        crate::engine_bootstrap::validate_capability(protocol, Direction::Pull, Some(&options))?;

        #[allow(unreachable_patterns)]
        match protocol {
            #[cfg(feature = "http-flv")]
            Protocol::HttpFlv => {
                crate::pull::http_flv::open_http_flv_pull(self.engine.clone(), url, options).await
            }
            #[cfg(feature = "rtsp")]
            Protocol::Rtsp => {
                crate::pull::rtsp::open_rtsp_pull(self.engine.clone(), url, options).await
            }
            #[cfg(feature = "rtmp")]
            Protocol::Rtmp => Err(ConnectorError::UnsupportedProtocol {
                protocol,
                direction: Direction::Pull,
            }),
            #[cfg(feature = "webrtc")]
            Protocol::WebRtc => Err(ConnectorError::UnsupportedProtocol {
                protocol,
                direction: Direction::Pull,
            }),
            _ => Err(ConnectorError::UnsupportedProtocol {
                protocol,
                direction: Direction::Pull,
            }),
        }
    }

    async fn open_push(
        &self,
        protocol: Protocol,
        _url: &str,
        _options: ConnectorPushOptions,
    ) -> Result<PushHandle, ConnectorError> {
        crate::engine_bootstrap::validate_capability(protocol, Direction::Push, None)?;

        #[allow(unreachable_patterns)]
        match protocol {
            #[cfg(feature = "rtmp")]
            Protocol::Rtmp => {
                crate::push::rtmp::open_rtmp_push(self.engine.clone(), _url, _options).await
            }
            #[cfg(feature = "webrtc")]
            Protocol::WebRtc => {
                crate::push::webrtc::open_webrtc_push(self.engine.clone(), _url, _options).await
            }
            #[cfg(feature = "rtsp")]
            Protocol::Rtsp => Err(ConnectorError::UnsupportedProtocol {
                protocol,
                direction: Direction::Push,
            }),
            #[cfg(feature = "http-flv")]
            Protocol::HttpFlv => Err(ConnectorError::UnsupportedProtocol {
                protocol,
                direction: Direction::Push,
            }),
            _ => Err(ConnectorError::UnsupportedProtocol {
                protocol,
                direction: Direction::Push,
            }),
        }
    }
}
