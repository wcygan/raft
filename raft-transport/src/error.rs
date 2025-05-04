use raft_core::NodeId;
use thiserror::Error;

/// Errors that can occur during mock transport operations.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    #[error("Peer node {0} not found in the transport registry")]
    PeerNotFound(NodeId),

    #[error("Communication channel to peer {0} is closed")]
    ChannelClosed(NodeId),

    #[error("Failed to send request to peer {0}: {1}")]
    SendError(NodeId, String),

    #[error("Failed to receive response from peer {0}")]
    RecvError(NodeId),

    #[error("Network simulation error: Message dropped to peer {0}")]
    MessageDropped(NodeId),

    #[error("Network simulation error: Node {0} is partitioned from peer {1}")]
    Partitioned(NodeId, NodeId),

    #[error("Node {0} is already registered in the transport")]
    NodeAlreadyExists(NodeId),

    #[error("Internal transport error: {0}")]
    Internal(String),
}
