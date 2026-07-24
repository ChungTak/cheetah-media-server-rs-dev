//! Output endpoint registry and helpers.
//!
//! 输出端点注册表与辅助类型。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use cheetah_media_api::error::{MediaError, Result};
pub use cheetah_media_api::output::{EndpointState, MediaOutputEndpoint};
use cheetah_media_api::port::MediaOutputRegistryApi;

/// Registration handle returned by `MediaServices::register_output_registry`.
///
/// `MediaServices` 注册输出注册表后返回的句柄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputRegistryRegistration {
    pub provider_id: String,
    pub generation: u64,
}

#[derive(Default)]
struct OutputRegistryState {
    generation: u64,
    next_id: u64,
    endpoints: HashMap<String, MediaOutputEndpoint>,
}

/// In-memory `MediaOutputRegistryApi` implementation.
///
/// 内存中的 `MediaOutputRegistryApi` 实现。
#[derive(Clone, Default)]
pub struct InMemoryMediaOutputRegistry {
    inner: Arc<RwLock<OutputRegistryState>>,
}

impl InMemoryMediaOutputRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl MediaOutputRegistryApi for InMemoryMediaOutputRegistry {
    async fn register_endpoint(&self, mut endpoint: MediaOutputEndpoint) -> Result<String> {
        let mut state = self.inner.write();
        state.generation += 1;
        state.next_id += 1;
        let id = format!("{}-{}", endpoint.provider, state.next_id);
        endpoint.registration_id = id.clone();
        endpoint.state = EndpointState::Active;
        state.endpoints.insert(id.clone(), endpoint);
        Ok(id)
    }

    async fn unregister_endpoint(&self, registration_id: &str) -> Result<()> {
        let mut state = self.inner.write();
        if state.endpoints.remove(registration_id).is_none() {
            return Err(MediaError::not_found(format!(
                "output endpoint {registration_id} not found"
            )));
        }
        state.generation += 1;
        Ok(())
    }

    async fn snapshot(&self) -> Result<Vec<MediaOutputEndpoint>> {
        let state = self.inner.read();
        let mut endpoints: Vec<_> = state.endpoints.values().cloned().collect();
        // Stable ordering makes resolver output deterministic.
        endpoints.sort_by(|a, b| a.registration_id.cmp(&b.registration_id));
        Ok(endpoints)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_media_api::ids::MediaSchema;

    fn test_endpoint() -> MediaOutputEndpoint {
        MediaOutputEndpoint::new(
            "rtmp",
            MediaSchema::Rtmp,
            "127.0.0.1",
            1935,
            false,
            "{app}/{stream}",
        )
    }

    #[tokio::test]
    async fn register_and_snapshot() {
        let registry = InMemoryMediaOutputRegistry::new();
        let id = registry.register_endpoint(test_endpoint()).await.unwrap();
        let snapshot = registry.snapshot().await.unwrap();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].registration_id, id);
        assert_eq!(snapshot[0].state, EndpointState::Active);
    }

    #[tokio::test]
    async fn unregister_removes_endpoint() {
        let registry = InMemoryMediaOutputRegistry::new();
        let id = registry.register_endpoint(test_endpoint()).await.unwrap();
        registry.unregister_endpoint(&id).await.unwrap();
        assert!(registry.snapshot().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn unregister_unknown_returns_not_found() {
        let registry = InMemoryMediaOutputRegistry::new();
        let err = registry.unregister_endpoint("missing").await.unwrap_err();
        assert_eq!(err.code, cheetah_media_api::error::MediaErrorCode::NotFound);
    }
}
