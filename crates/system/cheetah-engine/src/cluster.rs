use cheetah_sdk::{ClusterApi, ClusterNode, SdkError};
use dashmap::DashMap;
use parking_lot::RwLock;

/// `LocalCluster` data structure.
/// `LocalCluster` 数据结构.
#[derive(Default)]
pub struct LocalCluster {
    /// `local` field.
    /// `local` 字段.
    local: RwLock<Option<ClusterNode>>,
    /// `peers` field.
    /// `peers` 字段.
    peers: DashMap<String, ClusterNode>,
}

impl ClusterApi for LocalCluster {
    fn set_local_node(&self, node: ClusterNode) -> Result<(), SdkError> {
        self.peers.remove(&node.node_id);
        *self.local.write() = Some(node);
        Ok(())
    }

    fn upsert_peer(&self, node: ClusterNode) -> Result<(), SdkError> {
        if self
            .local
            .read()
            .as_ref()
            .is_some_and(|local| local.node_id == node.node_id)
        {
            return Err(SdkError::Conflict(format!(
                "peer id {} conflicts with local node",
                node.node_id
            )));
        }
        self.peers.insert(node.node_id.clone(), node);
        Ok(())
    }

    fn remove_peer(&self, node_id: &str) -> Result<(), SdkError> {
        self.peers
            .remove(node_id)
            .map(|_| ())
            .ok_or_else(|| SdkError::NotFound(format!("peer {node_id}")))
    }

    fn list_nodes(&self) -> Vec<ClusterNode> {
        let mut out = Vec::new();
        if let Some(local) = self.local.read().as_ref() {
            out.push(local.clone());
        }
        let mut peers: Vec<_> = self
            .peers
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        peers.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        out.extend(peers);
        out
    }
}
