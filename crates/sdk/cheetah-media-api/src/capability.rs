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
    WebRtc,
    ServerAdmin,
    Webhook,
    Playback,
}

/// Set of capabilities and versions advertised by a media provider.
///
/// 媒体 provider 声明的能力集与版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaCapabilitySet {
    pub capabilities: Vec<(MediaCapability, u32)>,
    pub version: String,
}

impl MediaCapabilitySet {
    /// Create an empty capability set.
    ///
    /// 创建空能力集。
    pub fn empty() -> Self {
        Self {
            capabilities: Vec::new(),
            version: "0.0.0".to_string(),
        }
    }

    /// Check whether a capability is available.
    ///
    /// 检查某能力是否可用。
    pub fn has(&self, capability: MediaCapability) -> bool {
        self.capabilities.iter().any(|(c, _)| *c == capability)
    }

    /// Add a capability with a version.
    ///
    /// 添加带版本的能力。
    pub fn add(&mut self, capability: MediaCapability, version: u32) {
        self.capabilities.push((capability, version));
    }

    /// Remove a capability.
    ///
    /// 移除能力。
    pub fn remove(&mut self, capability: MediaCapability) {
        self.capabilities.retain(|(c, _)| *c != capability);
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
    }
}
