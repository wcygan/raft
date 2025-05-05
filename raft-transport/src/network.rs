use raft_core::NodeId;
use rand::{Rng, SeedableRng, rngs::SmallRng};
use std::collections::HashSet;
use std::time::Duration;
use tokio::time::sleep;

/// Options to configure simulated network behavior for `MockTransport`.
#[derive(Clone, Debug)]
pub struct NetworkOptions {
    /// Probability (0.0 to 1.0) of dropping a message.
    pub message_loss_probability: f64,
    /// Minimum delay to add to message delivery.
    pub min_delay: Duration,
    /// Maximum delay to add to message delivery (delay will be uniform random between min and max).
    pub max_delay: Duration,
    /// Set of pairs (A, B) indicating that node A cannot send messages to node B.
    /// Note: This simulates one-way partitions. For a full partition, add both (A, B) and (B, A).
    pub partitioned_links: HashSet<(NodeId, NodeId)>,
}

impl Default for NetworkOptions {
    fn default() -> Self {
        Self {
            message_loss_probability: 0.0,
            min_delay: Duration::from_millis(0),
            max_delay: Duration::from_millis(0),
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
        let delay = if self.min_delay == Duration::ZERO && self.max_delay == Duration::ZERO {
            // No delay configured
            None
        } else if self.min_delay == self.max_delay {
            // Fixed delay
            Some(self.min_delay)
        } else if self.min_delay > self.max_delay {
            // Configuration error, maybe default to min_delay or warn?
            tracing::warn!(min = ?self.min_delay, max = ?self.max_delay, "min_delay > max_delay, using min_delay");
            Some(self.min_delay)
        } else {
            // Try from_rng with deprecated thread_rng as a test
            let mut thread_rng = rand::thread_rng(); // Get mutable rng
            let mut rng = SmallRng::from_rng(&mut thread_rng); // Pass mutable reference
            Some(rng.gen_range(self.min_delay..=self.max_delay))
        };

        if let Some(d) = delay {
            if d > Duration::ZERO {
                tracing::trace!(delay = ?d, "Applying simulated network delay");
                sleep(d).await;
            }
        }
    }
}
