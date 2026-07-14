//! HTTP adapter modules for the Cheetah media-domain API.
//!
//! This crate exposes two `cheetah_sdk::ModuleFactory` implementations:
//!
//! - `NativeMediaModuleFactory` mounts native `/api/v1/media/*`, `/api/v1/sessions/*`,
//!   `/api/v1/proxies/*`, `/api/v1/rtp/*` and `/api/v1/record/*` routes.
//! - `ZlmMediaModuleFactory` mounts ZLMediaKit-compatible `/index/api/*` and
//!   `/index/hook/*` routes.
//!
//! Both adapters call the same `cheetah_media_api` ports exposed through
//! `EngineContext::media_services`.
//!
//! Cheetah 媒体领域 API 的 HTTP adapter 模块。
//!
//! 本 crate 暴露两个 `cheetah_sdk::ModuleFactory` 实现：
//!
//! - `NativeMediaModuleFactory` 挂载 native `/api/v1/media/*`、`/api/v1/sessions/*`、
//!   `/api/v1/proxies/*`、`/api/v1/rtp/*` 与 `/api/v1/record/*` 路由。
//! - `ZlmMediaModuleFactory` 挂载 ZLMediaKit 兼容的 `/index/api/*` 与 `/index/hook/*` 路由。
//!
//! 两个 adapter 都通过 `EngineContext::media_services` 调用同一组 `cheetah_media_api` 端口。

mod error;
mod native;
mod util;
mod zlm;

pub use native::{NativeMediaModule, NativeMediaModuleFactory};
pub use zlm::{ZlmMediaModule, ZlmMediaModuleFactory};

#[cfg(test)]
mod tests {
    use cheetah_sdk::{Module, ModuleFactory};

    use crate::{
        NativeMediaModule, NativeMediaModuleFactory, ZlmMediaModule, ZlmMediaModuleFactory,
    };

    #[test]
    fn zlm_module_has_routes() {
        let module = ZlmMediaModule::new();
        let routes = module.http_routes();
        assert!(!routes.is_empty(), "ZLM module must expose routes");
        let paths: std::collections::HashSet<_> = routes.into_iter().map(|r| r.path).collect();
        assert!(paths.contains("/api/getMediaList"));
        assert!(paths.contains("/api/kick_session"));
    }

    #[test]
    fn native_module_has_empty_routes_for_fuzzy_prefix_matching() {
        let module = NativeMediaModule::new();
        let routes = module.http_routes();
        assert!(
            routes.is_empty(),
            "native module delegates routing to handle()"
        );
    }

    #[test]
    fn factory_manifests_match_module_ids() {
        let native = NativeMediaModuleFactory.manifest();
        let zlm = ZlmMediaModuleFactory.manifest();
        assert!(!native.module_id.0.is_empty());
        assert!(!zlm.module_id.0.is_empty());
        assert!(zlm.routes_prefix.starts_with('/'));
    }
}
