use serde::{Deserialize, Serialize};

/// Capability declaration for a media provider.
///
/// 媒体 provider 的能力声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaCapability {
    Query,
    SessionControl,
    Publish,
    Subscribe,
    Record,
    Snapshot,
    Proxy,
    Rtp,
    Webhook,
    Playback,
    UrlResolve,
}

/// Runtime state of a capability advertised by a provider.
///
/// 媒体 provider 宣告的能力运行时状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Starting,
    Available,
    Degraded,
    Stopping,
    Unavailable,
}

/// Descriptor for a single advertised capability, including its provider identity
/// and runtime state.
///
/// 单个宣告能力的描述符，包含其提供者与运行时状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaCapabilityDescriptor {
    pub capability: MediaCapability,
    pub version: u32,
    pub provider_id: String,
    pub state: CapabilityState,
    pub operations: Vec<String>,
}

/// Set of capabilities and versions advertised by a media provider.
///
/// 媒体 provider 声明的能力集与版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaCapabilitySet {
    pub capabilities: Vec<(MediaCapability, u32)>,
    /// Monotonic generation of the capability set. Incremented whenever the set
    /// changes so callers can detect stale snapshots.
    ///
    /// 能力集单调版本号。集合变化时递增，调用方可据此发现过期快照。
    pub version: u64,
}

impl MediaCapabilitySet {
    /// Create an empty capability set.
    ///
    /// 创建空能力集。
    pub fn empty() -> Self {
        Self {
            capabilities: Vec::new(),
            version: 0,
        }
    }

    /// Check whether a capability is available.
    ///
    /// 检查某能力是否可用。
    pub fn has(&self, capability: MediaCapability) -> bool {
        self.capabilities.iter().any(|(c, _)| *c == capability)
    }

    /// Add a capability with a version. If the capability already exists, its
    /// version is updated and the set generation is advanced only when the
    /// content changes.
    ///
    /// 添加带版本的能力。若已存在则更新版本，仅当内容变化时递增集合版本号。
    pub fn add(&mut self, capability: MediaCapability, version: u32) {
        if let Some(entry) = self.capabilities.iter_mut().find(|(c, _)| *c == capability) {
            if entry.1 == version {
                return;
            }
            entry.1 = version;
        } else {
            self.capabilities.push((capability, version));
        }
        self.version += 1;
    }

    /// Remove a capability and advance the set generation.
    ///
    /// 移除能力并递增集合版本号。
    pub fn remove(&mut self, capability: MediaCapability) {
        let before = self.capabilities.len();
        self.capabilities.retain(|(c, _)| *c != capability);
        if self.capabilities.len() != before {
            self.version += 1;
        }
    }

    /// Merge another capability set into this one, advancing the generation if
    /// the result changes.
    ///
    /// 合并另一个能力集；若结果变化则递增版本号。
    pub fn merge(&mut self, other: &MediaCapabilitySet) {
        let before_version = self.version;
        for (cap, version) in &other.capabilities {
            self.add(*cap, *version);
        }
        // `add` advances version on every structural change, so no further bump is needed.
        let _ = before_version;
    }
}

impl Default for MediaCapabilitySet {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_set_has_and_add() {
        let mut set = MediaCapabilitySet::empty();
        assert!(!set.has(MediaCapability::Record));
        set.add(MediaCapability::Record, 1);
        assert!(set.has(MediaCapability::Record));
        assert_eq!(set.version, 1);
    }

    #[test]
    fn capability_set_add_dedupes_and_updates_version() {
        let mut set = MediaCapabilitySet::empty();
        set.add(MediaCapability::Record, 1);
        set.add(MediaCapability::Record, 2);
        assert_eq!(set.capabilities.len(), 1);
        assert_eq!(set.capabilities[0].1, 2);
        assert_eq!(set.version, 2);

        set.add(MediaCapability::Record, 2);
        assert_eq!(set.version, 2);
    }

    #[test]
    fn capability_set_remove_advances_version() {
        let mut set = MediaCapabilitySet::empty();
        set.add(MediaCapability::Record, 1);
        set.remove(MediaCapability::Record);
        assert!(!set.has(MediaCapability::Record));
        assert_eq!(set.version, 2);

        set.remove(MediaCapability::Record);
        assert_eq!(set.version, 2);
    }

    #[test]
    fn capability_set_merge_unions_capabilities() {
        let mut a = MediaCapabilitySet::empty();
        a.add(MediaCapability::Query, 1);
        let mut b = MediaCapabilitySet::empty();
        b.add(MediaCapability::Record, 1);
        a.merge(&b);
        assert!(a.has(MediaCapability::Query));
        assert!(a.has(MediaCapability::Record));
    }
}
