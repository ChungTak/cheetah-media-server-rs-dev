use cheetah_sdk::{DatabaseApi, SdkError};
use dashmap::DashMap;

/// `InMemoryDatabase` data structure.
/// `InMemoryDatabase` 数据结构。
#[derive(Default)]
pub struct InMemoryDatabase {
    entries: DashMap<String, Vec<u8>>,
}

impl DatabaseApi for InMemoryDatabase {
    fn put(&self, key: &str, value: &[u8]) -> Result<(), SdkError> {
        self.entries.insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SdkError> {
        Ok(self.entries.get(key).map(|v| v.value().clone()))
    }

    fn delete(&self, key: &str) -> Result<(), SdkError> {
        self.entries.remove(key);
        Ok(())
    }

    fn list_prefix(&self, prefix: &str) -> Result<Vec<String>, SdkError> {
        let mut out: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.key().starts_with(prefix))
            .map(|entry| entry.key().clone())
            .collect();
        out.sort();
        Ok(out)
    }
}
