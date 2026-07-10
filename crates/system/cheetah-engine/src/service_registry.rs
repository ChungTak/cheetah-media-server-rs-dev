use cheetah_sdk::{SdkError, ServiceDescriptor, ServiceRegistry};
use dashmap::DashMap;

/// `InMemoryServiceRegistry` data structure.
/// `InMemoryServiceRegistry` 数据结构。
#[derive(Default)]
pub struct InMemoryServiceRegistry {
    services: DashMap<String, ServiceDescriptor>,
}

impl ServiceRegistry for InMemoryServiceRegistry {
    fn register(&self, service: ServiceDescriptor) -> Result<(), SdkError> {
        if self.services.contains_key(&service.name) {
            return Err(SdkError::AlreadyExists(format!("service {}", service.name)));
        }
        self.services.insert(service.name.clone(), service);
        Ok(())
    }

    fn get(&self, name: &str) -> Option<ServiceDescriptor> {
        self.services.get(name).map(|v| v.value().clone())
    }

    fn unregister(&self, name: &str) -> Result<(), SdkError> {
        self.services
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| SdkError::NotFound(format!("service {name}")))
    }

    fn list_services(&self) -> Vec<ServiceDescriptor> {
        let mut out: Vec<_> = self
            .services
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}
