use std::sync::Arc;

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_module::{NativeMediaModuleFactory, ZlmMediaModuleFactory};
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{ConfigApplyApi, ConfigEffect, ModuleConfigChange, ModuleId};
use serde_json::json;

fn make_engine_with_config(config: Arc<ConfigStore>) -> Arc<cheetah_engine::Engine> {
    let runtime = Arc::new(TokioRuntime::new());
    Arc::new(
        EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(Arc::new(NativeMediaModuleFactory))
            .register_module_factory(Arc::new(ZlmMediaModuleFactory))
            .build()
            .expect("engine build"),
    )
}

fn native_config_change(outcome: &cheetah_sdk::ConfigApplyOutcome) -> Option<ModuleConfigChange> {
    let global = outcome.global_change.as_ref()?;
    Some(ModuleConfigChange {
        module_id: ModuleId::new("media-http-native"),
        previous: json!({}),
        next: json!({}),
        previous_global: Some(global.previous.clone()),
        next_global: Some(global.next.clone()),
    })
}

#[tokio::test(flavor = "current_thread")]
async fn native_and_zlm_mount_prefixes_read_from_config() {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({
        "media": {
            "native": { "path_prefix": "/custom-native" },
            "zlm": { "path_prefix": "/custom-zlm" }
        }
    }));

    let engine = make_engine_with_config(config);
    engine.start().await.expect("engine start");

    let mounts = engine.module_manager_api().http_mounts();
    let prefixes: std::collections::HashSet<_> = mounts.iter().map(|m| m.prefix.as_str()).collect();
    assert!(
        prefixes.contains("/custom-native"),
        "native prefix should be configurable: {prefixes:?}"
    );
    assert!(
        prefixes.contains("/custom-zlm"),
        "zlm prefix should be configurable: {prefixes:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn disabled_adapter_does_not_mount() {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({
        "media": {
            "native": { "enabled": false }
        }
    }));

    let engine = make_engine_with_config(config);
    engine.start().await.expect("engine start");

    let mounts = engine.module_manager_api().http_mounts();
    let ids: std::collections::HashSet<_> = mounts.iter().map(|m| m.module_id.0.as_str()).collect();
    assert!(
        !ids.contains("media-http-native"),
        "disabled native adapter should not be mounted"
    );
    assert!(
        ids.contains("media-http-zlm"),
        "zlm adapter should still be mounted"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn path_prefix_change_requires_module_restart() {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({
        "media": {
            "native": { "path_prefix": "/api/v1" }
        }
    }));

    let engine = make_engine_with_config(config.clone());
    engine.start().await.expect("engine start");

    let outcome = config
        .apply_global_patch(
            json!({ "media": { "native": { "path_prefix": "/api/v2" } } }),
            ConfigEffect::ModuleRestartRequired,
        )
        .expect("patch config");
    let change = native_config_change(&outcome).expect("native config change");

    let report = engine
        .module_manager_api()
        .apply_module_config_change(change)
        .await
        .expect("apply config");
    assert_eq!(report.effect, ConfigEffect::ModuleRestartRequired);

    let mounts = engine.module_manager_api().http_mounts();
    let native_prefix = mounts
        .iter()
        .find(|m| m.module_id.0 == "media-http-native")
        .map(|m| m.prefix.as_str());
    assert_eq!(native_prefix, Some("/api/v2"));
}

#[tokio::test(flavor = "current_thread")]
async fn request_timeout_and_body_limit_changes_are_immediate() {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({
        "media": {
            "native": {
                "request_timeout_ms": 5000,
                "max_body_bytes": 1024
            }
        }
    }));

    let engine = make_engine_with_config(config.clone());
    engine.start().await.expect("engine start");

    let outcome = config
        .apply_global_patch(
            json!({
                "media": {
                    "native": {
                        "request_timeout_ms": 1000,
                        "max_body_bytes": 512
                    }
                }
            }),
            ConfigEffect::Immediate,
        )
        .expect("patch config");
    let change = native_config_change(&outcome).expect("native config change");

    let report = engine
        .module_manager_api()
        .apply_module_config_change(change)
        .await
        .expect("apply config");
    assert_eq!(report.effect, ConfigEffect::Immediate);

    let mounts = engine.module_manager_api().http_mounts();
    let native = mounts
        .iter()
        .find(|m| m.module_id.0 == "media-http-native")
        .expect("native mount");
    assert_eq!(native.request_timeout_ms, Some(1000));
    assert_eq!(native.max_body_bytes, 512);
}

#[tokio::test(flavor = "current_thread")]
async fn native_and_zlm_adapters_are_independent_config_namespaces() {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({
        "media": {
            "native": { "enabled": false, "path_prefix": "/native" },
            "zlm": { "path_prefix": "/zlm" }
        }
    }));

    let engine = make_engine_with_config(config);
    engine.start().await.expect("engine start");

    let mounts = engine.module_manager_api().http_mounts();
    let by_id: std::collections::HashMap<_, _> = mounts
        .iter()
        .map(|m| (m.module_id.0.as_str(), m.prefix.as_str()))
        .collect();
    assert_eq!(by_id.get("media-http-native"), None);
    assert_eq!(by_id.get("media-http-zlm"), Some(&"/zlm"));
}
