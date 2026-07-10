use cheetah_sdk::{ProxyManager, ProxyRoute, SdkError};
use dashmap::DashMap;

/// `LocalProxyManager` data structure.
/// `LocalProxyManager` 数据结构。
#[derive(Default)]
pub struct LocalProxyManager {
    routes: DashMap<String, ProxyRoute>,
}

impl ProxyManager for LocalProxyManager {
    fn register_route(&self, route: ProxyRoute) -> Result<(), SdkError> {
        if self.routes.contains_key(&route.path_prefix) {
            return Err(SdkError::AlreadyExists(format!(
                "proxy route {}",
                route.path_prefix
            )));
        }
        self.routes.insert(route.path_prefix.clone(), route);
        Ok(())
    }

    fn remove_route(&self, path_prefix: &str) -> Result<(), SdkError> {
        self.routes
            .remove(path_prefix)
            .map(|_| ())
            .ok_or_else(|| SdkError::NotFound(format!("proxy route {path_prefix}")))
    }

    fn list_routes(&self) -> Vec<ProxyRoute> {
        let mut out: Vec<_> = self
            .routes
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        out.sort_by(|a, b| a.path_prefix.cmp(&b.path_prefix));
        out
    }
}
