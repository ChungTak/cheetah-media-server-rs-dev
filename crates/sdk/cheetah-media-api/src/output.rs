//! Output endpoint registry types.
//!
//! 输出端点注册表类型。

use serde::{Deserialize, Serialize};

use crate::ids::MediaSchema;

/// Runtime state of a registered output endpoint.
///
/// 已注册输出端点的运行时状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointState {
    Starting,
    Active,
    Draining,
    Stopped,
    Error,
}

/// A public-facing endpoint that can be used to access a media resource.
///
/// 可用于访问媒体资源的公网端点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaOutputEndpoint {
    /// Opaque registration id assigned by the registry.
    pub registration_id: String,
    /// Identifies the provider/module that owns this endpoint.
    pub provider: String,
    /// Output schema (rtmp, rtsp, hls, ...).
    pub schema: MediaSchema,
    /// Public host or IP advertised to clients.
    pub public_host: String,
    /// Public port advertised to clients.
    pub port: u16,
    /// Whether TLS is in use.
    pub tls: bool,
    /// URL path template, which may contain `{app}` and `{stream}` placeholders.
    pub path_template: String,
    /// Current endpoint state.
    pub state: EndpointState,
}

impl MediaOutputEndpoint {
    /// Create a new endpoint with `Starting` state.
    pub fn new(
        provider: impl Into<String>,
        schema: MediaSchema,
        public_host: impl Into<String>,
        port: u16,
        tls: bool,
        path_template: impl Into<String>,
    ) -> Self {
        Self {
            registration_id: String::new(),
            provider: provider.into(),
            schema,
            public_host: public_host.into(),
            port,
            tls,
            path_template: path_template.into(),
            state: EndpointState::Starting,
        }
    }

    /// Fill the registration id after the registry assigns one.
    pub fn with_registration_id(mut self, id: impl Into<String>) -> Self {
        self.registration_id = id.into();
        self
    }
}
