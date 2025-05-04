use raft_core::NodeId;
use std::collections::HashSet;
use std::time::Duration;

/// Options for simulating network conditions in the MockTransport.
#[derive(Debug, Clone)]
pub struct NetworkOptions {
    /// Optional delay applied to every message sent.
    pub message_delay: Option<Duration>,
    /// Probability (0.0 to 1.0) that a message will be dropped.
    pub message_loss_probability: f64,
    /// Set of pairs (A, B) indicating that node A cannot send messages to node B.
    /// Note: This simulates one-way partitions. For a full partition, add both (A, B) and (B, A).
    pub partitioned_links: HashSet<(NodeId, NodeId)>,
}

impl Default for NetworkOptions {
    fn default() -> Self {
        NetworkOptions {
            message_delay: None,
            message_loss_probability: 0.0,
            partitioned_links: HashSet::new(),
        }
    }
}

impl NetworkOptions {
    /// Checks if a message from `sender` to `receiver` should be dropped due to loss probability.
    pub fn should_drop_message(&self) -> bool {
        self.message_loss_probability > 0.0 && rand::random::<f64>() < self.message_loss_probability
    }

    /// Checks if the link from `sender` to `receiver` is partitioned.
    pub fn is_partitioned(&self, sender: NodeId, receiver: NodeId) -> bool {
        self.partitioned_links.contains(&(sender, receiver))
    }

    /// Simulates the message delay, if configured.
    pub async fn simulate_delay(&self) {
        if let Some(delay) = self.message_delay {
            tokio::time::sleep(delay).await;
        }
    }
}
