use std::any::Any;
use std::sync::Arc;

use cheetah_config::ConfigStore;
use cheetah_engine::{DispatcherMode, Engine, EngineBuilder};
use cheetah_runtime_api::RuntimeApi;
use cheetah_sdk::{ConfigApplyApi, ConfigProvider, ModuleFactory};

use crate::error::ConnectorError;
use crate::options::ConnectorPullOptions;
use crate::protocol::{supports, Direction, Protocol};

/// Builder for assembling an `EngineConnector`.
///
/// 组装 `EngineConnector` 的构建器。
pub struct ConnectorBuilder {
    runtime: Arc<dyn RuntimeApi>,
    config_provider: Option<Arc<dyn ConfigProvider>>,
    config_apply: Option<Arc<dyn ConfigApplyApi>>,
    register_default_modules: bool,
    dispatcher_mode: DispatcherMode,
    register_rtmp: bool,
    register_rtsp: bool,
    register_http_flv: bool,
    register_webrtc: bool,
}

impl ConnectorBuilder {
    /// Create a builder with the required runtime.
    ///
    /// 使用必需的运行时创建构建器。
    pub fn new(runtime: Arc<dyn RuntimeApi>) -> Self {
        Self {
            runtime,
            config_provider: None,
            config_apply: None,
            register_default_modules: true,
            dispatcher_mode: DispatcherMode::PerStream,
            register_rtmp: cfg!(feature = "rtmp"),
            register_rtsp: cfg!(feature = "rtsp"),
            register_http_flv: cfg!(feature = "http-flv"),
            register_webrtc: cfg!(feature = "webrtc"),
        }
    }

    /// Use an explicit config provider. If not set, an empty `ConfigStore` is used.
    ///
    /// 使用显式配置提供方。若未设置，则使用空 `ConfigStore`。
    pub fn with_config_provider(mut self, provider: Arc<dyn ConfigProvider>) -> Self {
        self.config_provider = Some(provider);
        self
    }

    /// Use an explicit config apply API. If not set, the same `ConfigStore` is used.
    ///
    /// 使用显式配置应用 API。若未设置，则使用同一 `ConfigStore`。
    pub fn with_config_apply(mut self, apply: Arc<dyn ConfigApplyApi>) -> Self {
        self.config_apply = Some(apply);
        self
    }

    /// Register the default module factories matching enabled features.
    ///
    /// 注册与已启用特性匹配的默认模块工厂。
    pub fn with_default_modules(mut self) -> Self {
        self.register_default_modules = true;
        self
    }

    /// Disable default module registration.
    ///
    /// 禁用默认模块注册。
    pub fn without_default_modules(mut self) -> Self {
        self.register_default_modules = false;
        self
    }

    /// Configure the engine dispatcher mode.
    ///
    /// 配置引擎分发器模式。
    pub fn with_dispatcher_mode(mut self, mode: DispatcherMode) -> Self {
        self.dispatcher_mode = mode;
        self
    }

    /// Force enable/disable RTMP module registration.
    ///
    /// 强制启用/禁用 RTMP 模块注册。
    pub fn with_rtmp(mut self, enable: bool) -> Self {
        self.register_rtmp = enable;
        self
    }

    /// Force enable/disable RTSP module registration.
    ///
    /// 强制启用/禁用 RTSP 模块注册。
    pub fn with_rtsp(mut self, enable: bool) -> Self {
        self.register_rtsp = enable;
        self
    }

    /// Force enable/disable HTTP-FLV module registration.
    ///
    /// 强制启用/禁用 HTTP-FLV 模块注册。
    pub fn with_http_flv(mut self, enable: bool) -> Self {
        self.register_http_flv = enable;
        self
    }

    /// Force enable/disable WebRTC module registration.
    ///
    /// 强制启用/禁用 WebRTC 模块注册。
    pub fn with_webrtc(mut self, enable: bool) -> Self {
        self.register_webrtc = enable;
        self
    }

    /// Build the underlying engine.
    ///
    /// 构建底层引擎。
    pub fn build_engine(self) -> Result<Engine, ConnectorError> {
        let provider = self.config_provider;
        let apply = self.config_apply;

        // Try to extract a concrete ConfigStore from either side so that the
        // fallback side can share the same instance. This makes the promise in
        // `with_config_apply` ("if not set, the same ConfigStore is used") true
        // even when only one side is provided.
        let provider_store = provider.as_ref().and_then(|p| {
            let any: Arc<dyn Any + Send + Sync> = p.clone();
            any.downcast::<ConfigStore>().ok()
        });
        let apply_store = apply.as_ref().and_then(|a| {
            let any: Arc<dyn Any + Send + Sync> = a.clone();
            any.downcast::<ConfigStore>().ok()
        });

        let shared = match (provider_store, apply_store) {
            (Some(p), Some(a)) if Arc::ptr_eq(&p, &a) => Some(p),
            (Some(p), None) => Some(p),
            (None, Some(a)) => Some(a),
            (None, None) => Some(Arc::new(ConfigStore::new())),
            _ => None,
        };

        let (config_provider, config_apply) = if let Some(store) = shared {
            let provider = provider.unwrap_or_else(|| store.clone() as Arc<dyn ConfigProvider>);
            let apply = apply.unwrap_or_else(|| store.clone() as Arc<dyn ConfigApplyApi>);
            (provider, apply)
        } else {
            (provider.unwrap(), apply.unwrap())
        };

        let mut builder = EngineBuilder::new(config_provider, config_apply, self.runtime)
            .with_dispatcher_mode(self.dispatcher_mode);

        if self.register_default_modules {
            #[cfg(feature = "rtmp")]
            if self.register_rtmp {
                builder = builder
                    .register_module_factory(
                        Arc::new(self::bootstrap::RtmpModuleFactory) as Arc<dyn ModuleFactory>
                    );
            }
            #[cfg(feature = "rtsp")]
            if self.register_rtsp {
                builder = builder
                    .register_module_factory(
                        Arc::new(self::bootstrap::RtspModuleFactory) as Arc<dyn ModuleFactory>
                    );
            }
            #[cfg(feature = "http-flv")]
            if self.register_http_flv {
                builder = builder
                    .register_module_factory(
                        Arc::new(self::bootstrap::HttpFlvModuleFactory) as Arc<dyn ModuleFactory>
                    );
            }
            #[cfg(feature = "webrtc")]
            if self.register_webrtc {
                builder = builder
                    .register_module_factory(
                        Arc::new(self::bootstrap::WebRtcModuleFactory) as Arc<dyn ModuleFactory>
                    );
            }
        }

        builder
            .build()
            .map_err(|e| ConnectorError::Internal(format!("engine build failed: {e}")))
    }

    /// Build the engine and wrap it in an `EngineConnector`.
    ///
    /// 构建引擎并包装为 `EngineConnector`。
    pub fn build(self) -> Result<super::EngineConnector, ConnectorError> {
        let engine = Arc::new(self.build_engine()?);
        Ok(super::EngineConnector::new(engine))
    }
}

/// Validate that a requested protocol/direction pair is supported by the connector
/// capability matrix and that the corresponding feature is enabled.
///
/// 验证请求的协议/方向组合是否受 connector 能力矩阵支持，且对应特性已启用。
pub(crate) fn validate_capability(
    protocol: Protocol,
    direction: Direction,
    options: Option<&ConnectorPullOptions>,
) -> Result<(), ConnectorError> {
    if !supports(protocol, direction) {
        return Err(ConnectorError::UnsupportedProtocol {
            protocol,
            direction,
        });
    }

    if let Some(opts) = options {
        if opts.subscriber.queue_capacity == 0 {
            return Err(ConnectorError::InvalidArgument(
                "subscriber queue_capacity must be > 0".to_string(),
            ));
        }
    }

    Ok(())
}

mod bootstrap {
    #![allow(unused_imports)]

    #[cfg(feature = "http-flv")]
    pub(crate) use cheetah_http_flv_module::HttpFlvModuleFactory;
    #[cfg(feature = "rtmp")]
    pub(crate) use cheetah_rtmp_module::RtmpModuleFactory;
    #[cfg(feature = "rtsp")]
    pub(crate) use cheetah_rtsp_module::RtspModuleFactory;
    #[cfg(feature = "webrtc")]
    pub(crate) use cheetah_webrtc_module::WebRtcModuleFactory;
}
