use serde::{Deserialize, Serialize};

/// Capability declaration for a media provider.
///
/// 媒体 provider 的能力声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    #[serde(default)]
    pub operations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl MediaCapabilityDescriptor {
    /// Create a descriptor with the given capability, version and provider.
    /// State defaults to `Available` and operations are filled from the
    /// well-known operation list for the capability.
    pub fn new(capability: MediaCapability, version: u32, provider_id: impl Into<String>) -> Self {
        Self {
            capability,
            version,
            provider_id: provider_id.into(),
            state: CapabilityState::Available,
            operations: default_operations(capability),
            reason: None,
        }
    }

    /// Mark the descriptor as degraded with a reason.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.state = CapabilityState::Degraded;
        self.reason = Some(reason.into());
        self
    }

    /// Override the default operations list.
    pub fn with_operations(mut self, operations: Vec<String>) -> Self {
        self.operations = operations;
        self
    }
}

/// Aggregate capability report across all registered providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaCapabilityReport {
    pub generation: u64,
    pub descriptors: Vec<MediaCapabilityDescriptor>,
}

impl MediaCapabilityReport {
    /// Create an empty report.
    pub fn empty() -> Self {
        Self {
            generation: 0,
            descriptors: Vec::new(),
        }
    }

    /// Build a report from a single provider's capability set.
    pub fn from_capability_set(set: &MediaCapabilitySet, provider_id: impl Into<String>) -> Self {
        let provider_id = provider_id.into();
        let mut descriptors: Vec<_> = set
            .capabilities
            .iter()
            .map(|(cap, version)| MediaCapabilityDescriptor::new(*cap, *version, &provider_id))
            .collect();
        descriptors.sort_by(|a, b| {
            a.capability
                .cmp(&b.capability)
                .then_with(|| a.provider_id.cmp(&b.provider_id))
        });
        Self {
            generation: set.version,
            descriptors,
        }
    }

    /// Merge another report into this one, replacing descriptors with the same
    /// capability and provider_id and advancing the generation if the result
    /// changes.
    pub fn merge(&mut self, other: &MediaCapabilityReport) {
        let before = self.descriptors.clone();
        for descriptor in &other.descriptors {
            if let Some(existing) = self.descriptors.iter_mut().find(|d| {
                d.capability == descriptor.capability && d.provider_id == descriptor.provider_id
            }) {
                *existing = descriptor.clone();
            } else {
                self.descriptors.push(descriptor.clone());
            }
        }
        self.descriptors.sort_by(|a, b| {
            a.capability
                .cmp(&b.capability)
                .then_with(|| a.provider_id.cmp(&b.provider_id))
        });
        if self.descriptors != before || self.generation < other.generation {
            self.generation = self.generation.max(other.generation) + 1;
        }
    }
}

/// Well-known operation names advertised for each capability.
pub fn default_operations(capability: MediaCapability) -> Vec<String> {
    match capability {
        MediaCapability::Query => vec!["list".to_string(), "get".to_string(), "online".to_string()],
        MediaCapability::SessionControl => vec![
            "list_sessions".to_string(),
            "kick_session".to_string(),
            "kick_stream".to_string(),
            "request_keyframe".to_string(),
        ],
        MediaCapability::Publish => vec!["acquire_publisher".to_string()],
        MediaCapability::Subscribe => vec!["open_subscriber".to_string()],
        MediaCapability::Record => vec![
            "start".to_string(),
            "stop".to_string(),
            "query_tasks".to_string(),
            "query_files".to_string(),
            "delete_file".to_string(),
        ],
        MediaCapability::Snapshot => vec![
            "take".to_string(),
            "query".to_string(),
            "delete_directory".to_string(),
        ],
        MediaCapability::Proxy => vec![
            "create_pull".to_string(),
            "delete_pull".to_string(),
            "list_pull".to_string(),
            "create_push".to_string(),
            "delete_push".to_string(),
            "create_ffmpeg".to_string(),
            "delete_ffmpeg".to_string(),
        ],
        MediaCapability::Rtp => vec![
            "open_receiver".to_string(),
            "connect_receiver".to_string(),
            "open_sender".to_string(),
            "stop_session".to_string(),
            "list_sessions".to_string(),
            "update_session".to_string(),
            "get_session".to_string(),
        ],
        MediaCapability::Webhook => vec!["request_decision".to_string()],
        MediaCapability::Playback => vec![
            "open".to_string(),
            "get".to_string(),
            "list".to_string(),
            "control".to_string(),
            "stop".to_string(),
        ],
    }
    .into_iter()
    .collect()
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

impl Default for MediaCapabilityReport {
    fn default() -> Self {
        Self::empty()
    }
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

    #[test]
    fn capability_descriptor_defaults_to_available_with_operations() {
        let d = MediaCapabilityDescriptor::new(MediaCapability::Record, 3, "record:1");
        assert_eq!(d.state, CapabilityState::Available);
        assert_eq!(d.version, 3);
        assert!(d.operations.contains(&"start".to_string()));
        assert!(d.reason.is_none());
    }

    #[test]
    fn capability_descriptor_with_reason_becomes_degraded() {
        let d = MediaCapabilityDescriptor::new(MediaCapability::Rtp, 1, "rtp:1")
            .with_reason("tcp active not configured");
        assert_eq!(d.state, CapabilityState::Degraded);
        assert_eq!(d.reason.as_deref(), Some("tcp active not configured"));
    }

    #[test]
    fn capability_report_from_set_is_sorted_by_capability_then_provider() {
        let mut set = MediaCapabilitySet::empty();
        set.add(MediaCapability::Rtp, 1);
        set.add(MediaCapability::Query, 1);
        let report = MediaCapabilityReport::from_capability_set(&set, "test");
        assert_eq!(report.descriptors.len(), 2);
        assert_eq!(report.descriptors[0].capability, MediaCapability::Query);
        assert_eq!(report.descriptors[1].capability, MediaCapability::Rtp);
    }

    #[test]
    fn capability_report_merge_replaces_same_provider_and_advances_generation() {
        let mut a = MediaCapabilityReport::from_capability_set(
            &{
                let mut s = MediaCapabilitySet::empty();
                s.add(MediaCapability::Query, 1);
                s
            },
            "p1",
        );
        let gen_before = a.generation;
        let b = MediaCapabilityReport::from_capability_set(
            &{
                let mut s = MediaCapabilitySet::empty();
                s.add(MediaCapability::Query, 2);
                s
            },
            "p1",
        );
        a.merge(&b);
        assert_eq!(a.generation, gen_before.max(b.generation) + 1);
        assert_eq!(a.descriptors[0].version, 2);
    }
}
